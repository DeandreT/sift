//! Opt-in broker verification of retained session receivers and dispositions.
//! Uses a unique scratch queue; no existing entity is read or changed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sift_backend::{
    BackendError, Command, Disposition, EntityPath, Event, MessageSource, RequestId,
    SessionSnapshot,
};
use sift_core::config::NamespaceProfile;
use sift_core::connection::NamespaceConnection;
use sift_core::message::OutboundMessage;
use sift_core::secrets::SecretString;
use sift_mgmt::{ManagementClient, QueueProperties};

fn wait<T>(rx: &crossbeam_channel::Receiver<Event>, mut pick: impl FnMut(Event) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("backend timed out");
        if let Some(value) = pick(rx.recv_timeout(remaining).expect("backend event")) {
            return value;
        }
    }
}

fn session(
    rx: &crossbeam_channel::Receiver<Event>,
    request: RequestId,
) -> Result<SessionSnapshot, BackendError> {
    wait(rx, |event| match event {
        Event::Session { req, result, .. } if req == request => Some(result),
        _ => None,
    })
}

struct ScratchQueue {
    client: ManagementClient,
    runtime: tokio::runtime::Runtime,
    name: String,
}

impl Drop for ScratchQueue {
    fn drop(&mut self) {
        if let Err(error) = self.runtime.block_on(self.client.delete_queue(&self.name)) {
            eprintln!("could not clean up scratch queue {}: {error}", self.name);
        }
    }
}

#[allow(clippy::too_many_lines)]
#[test]
fn live_session_lifecycle() {
    let Ok(connection_string) = std::env::var("SIFT_TEST_SB_CONNECTION_STRING") else {
        eprintln!("skipped: SIFT_TEST_SB_CONNECTION_STRING not set");
        return;
    };
    if std::env::var("SIFT_TEST_SB_MUTATE").as_deref() != Ok("1") {
        eprintln!("skipped: SIFT_TEST_SB_MUTATE != 1");
        return;
    }
    let connection = NamespaceConnection::parse(&connection_string).expect("connection string");
    let client = ManagementClient::new(&connection).expect("management client");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let name = format!("sift-test-session-{}", uuid::Uuid::new_v4());
    runtime
        .block_on(client.create_queue(&QueueProperties {
            name: name.clone(),
            requires_session: true,
            ..QueueProperties::default()
        }))
        .expect("scratch session queue");
    let _scratch = ScratchQueue {
        client,
        runtime,
        name: name.clone(),
    };

    let (backend, events) = sift_backend::spawn(Arc::new(|| {}));
    let profile = NamespaceProfile::new_connection_string("session integration".into());
    let ns = profile.id;
    backend.send(Command::Connect {
        req: backend.next_request(),
        profile,
        secret: SecretString::from(connection_string.as_str()),
    });
    wait(&events, |event| match event {
        Event::Connected { result, .. } => Some(result.expect("connect")),
        _ => None,
    });
    let entity = EntityPath::Queue(name);
    let source = MessageSource {
        entity: entity.clone(),
        dead_letter: false,
    };
    let messages = (0..4)
        .map(|index| OutboundMessage {
            body: format!("session message {index}"),
            session_id: Some("integration-session".into()),
            ..OutboundMessage::default()
        })
        .collect();
    backend.send(Command::SendMessages {
        req: backend.next_request(),
        ns,
        target: entity,
        messages,
    });
    wait(&events, |event| match event {
        Event::Sent { result, .. } => Some(result.expect("send")),
        _ => None,
    });

    let req = backend.next_request();
    backend.send(Command::BrowseSession {
        req,
        ns,
        source: source.clone(),
        session_id: Some("integration-session".into()),
        count: 10,
    });
    let snapshot = session(&events, req).expect("accept named session");
    let lease_id = snapshot.lease_id;
    assert_eq!(snapshot.messages.len(), 4);
    assert!(snapshot.messages.iter().all(|row| row.lock_token.is_none()));
    assert!(!snapshot.lock_expired(time::OffsetDateTime::now_utc()));

    let req = backend.next_request();
    backend.send(Command::ReceiveSession {
        req,
        ns,
        source: source.clone(),
        lease_id,
        count: 4,
        sequence_numbers: Vec::new(),
    });
    let received = session(&events, req).expect("session receive");
    assert_eq!(received.messages.len(), 4);
    assert!(received.messages.iter().all(|row| row.lock_token.is_some()));
    let req = backend.next_request();
    backend.send(Command::RenewSession {
        req,
        ns,
        source: source.clone(),
        lease_id,
        lock_token: None,
    });
    let renewed = session(&events, req).expect("session renewal");
    assert!(renewed.locked_until >= snapshot.locked_until);
    assert!(
        renewed
            .messages
            .iter()
            .all(|row| row.locked_until == Some(renewed.locked_until))
    );
    let req = backend.next_request();
    backend.send(Command::RenewSession {
        req,
        ns,
        source: source.clone(),
        lease_id,
        lock_token: received.messages[0].lock_token.clone(),
    });
    let renewed = session(&events, req).expect("renew session through a received delivery");
    assert!(
        renewed
            .messages
            .iter()
            .all(|row| row.locked_until == Some(renewed.locked_until))
    );

    for (row, disposition) in received.messages.iter().zip([
        Disposition::Complete,
        Disposition::Abandon,
        Disposition::Defer,
        Disposition::DeadLetter {
            reason: Some("integration test".into()),
            description: Some("session disposition".into()),
        },
    ]) {
        let req = backend.next_request();
        backend.send(Command::SettleSessionMessage {
            req,
            ns,
            source: source.clone(),
            lease_id,
            lock_token: row.lock_token.clone().expect("delivery lock"),
            disposition,
        });
        wait(&events, |event| match event {
            Event::SessionSettled {
                req: response,
                result,
                ..
            } if response == req => {
                result.expect("session settlement");
                Some(())
            }
            _ => None,
        });
    }
    let req = backend.next_request();
    backend.send(Command::ReceiveSession {
        req,
        ns,
        source: source.clone(),
        lease_id,
        count: 1,
        sequence_numbers: vec![received.messages[2].sequence_number],
    });
    let deferred = session(&events, req).expect("receive deferred session message");
    let row = deferred
        .messages
        .iter()
        .find(|row| row.sequence_number == received.messages[2].sequence_number)
        .expect("deferred delivery");
    let req = backend.next_request();
    backend.send(Command::SettleSessionMessage {
        req,
        ns,
        source: source.clone(),
        lease_id,
        lock_token: row.lock_token.clone().expect("deferred lock"),
        disposition: Disposition::Complete,
    });
    wait(&events, |event| match event {
        Event::SessionSettled {
            req: response,
            result,
            ..
        } if response == req => {
            result.expect("complete deferred");
            Some(())
        }
        _ => None,
    });
    backend.send(Command::ReleaseSession {
        ns,
        source: source.clone(),
        lease_id,
    });

    let req = backend.next_request();
    backend.send(Command::BrowseSession {
        req,
        ns,
        source: source.clone(),
        session_id: None,
        count: 10,
    });
    let next = session(&events, req).expect("accept next available session");
    assert_eq!(next.session_id, "integration-session");
    assert_ne!(next.lease_id, lease_id);
    let req = backend.next_request();
    backend.send(Command::ReceiveSession {
        req,
        ns,
        source: source.clone(),
        lease_id,
        count: 1,
        sequence_numbers: Vec::new(),
    });
    assert!(
        session(&events, req)
            .expect_err("stale lease rejected")
            .session_lock_lost()
    );
    let req = backend.next_request();
    backend.send(Command::ReceiveSession {
        req,
        ns,
        source: source.clone(),
        lease_id: next.lease_id,
        count: 1,
        sequence_numbers: Vec::new(),
    });
    assert_eq!(
        session(&events, req)
            .expect("replacement receiver survived stale command")
            .messages
            .len(),
        1
    );

    let req = backend.next_request();
    backend.send(Command::PeekMessages {
        req,
        ns,
        source: MessageSource {
            dead_letter: true,
            ..source.clone()
        },
        from_seq: None,
        count: 10,
    });
    let dead_letter = wait(&events, |event| match event {
        Event::Messages {
            req: response,
            result,
            ..
        } if response == req => Some(result.expect("dead-letter peek")),
        _ => None,
    });
    assert_eq!(dead_letter.len(), 1);
    assert_eq!(
        dead_letter[0].dead_letter_reason.as_deref(),
        Some("integration test")
    );
    backend.send(Command::Disconnect { ns });
    wait(&events, |event| match event {
        Event::Disconnected { .. } => Some(()),
        _ => None,
    });
    backend.send(Command::Shutdown);
}
