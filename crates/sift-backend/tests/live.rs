//! Live end-to-end messaging test against a real Service Bus namespace,
//! driving the backend through its real command/event channels.
//!
//! Self-skips unless `SIFT_TEST_SB_CONNECTION_STRING` is set and
//! `SIFT_TEST_SB_MUTATE=1`. Only touches a uniquely-named `sift-test-*`
//! queue, which is created and deleted by the test itself.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sift_backend::{Command, Disposition, EntityPath, Event, MessageSource, ReceiveMode};
use sift_core::config::NamespaceProfile;
use sift_core::connection::NamespaceConnection;
use sift_core::message::OutboundMessage;
use sift_core::secrets::SecretString;
use sift_mgmt::{ManagementClient, QueueProperties};

const WAIT: Duration = Duration::from_secs(30);

fn recv_until<T>(
    rx: &crossbeam_channel::Receiver<Event>,
    mut pick: impl FnMut(Event) -> Option<T>,
) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("timed out waiting for a backend event");
        let event = rx
            .recv_timeout(remaining)
            .expect("backend event channel closed or timed out");
        if let Some(value) = pick(event) {
            return value;
        }
    }
}

#[allow(clippy::too_many_lines)] // a linear end-to-end scenario reads best in one function
#[test]
fn live_messaging_round_trip() {
    let Ok(connection_string) = std::env::var("SIFT_TEST_SB_CONNECTION_STRING") else {
        eprintln!("skipped: SIFT_TEST_SB_CONNECTION_STRING not set");
        return;
    };
    if std::env::var("SIFT_TEST_SB_MUTATE").as_deref() != Ok("1") {
        eprintln!("skipped: SIFT_TEST_SB_MUTATE != 1");
        return;
    }

    // A scratch queue, created directly via the management client.
    let conn = NamespaceConnection::parse(&connection_string).expect("valid connection string");
    let mgmt = ManagementClient::new(&conn).expect("management client");
    let queue_name = format!("sift-test-msg-{}", uuid::Uuid::new_v4());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime for mgmt calls");
    rt.block_on(mgmt.create_queue(&QueueProperties {
        name: queue_name.clone(),
        ..QueueProperties::default()
    }))
    .expect("create scratch queue");

    // Drive the real backend.
    let (backend, events) = sift_backend::spawn(Arc::new(|| {}));
    let profile = NamespaceProfile::new_connection_string("live-test".into());
    let ns = profile.id;
    backend.send(Command::Connect {
        req: backend.next_request(),
        profile,
        secret: SecretString::from(connection_string.as_str()),
    });
    recv_until(&events, |e| match e {
        Event::Connected { result, .. } => Some(result.expect("connect")),
        _ => None,
    });

    let queue_path = EntityPath::Queue(queue_name.clone());
    let source = MessageSource {
        entity: queue_path.clone(),
        dead_letter: false,
    };
    let dlq_source = MessageSource {
        entity: queue_path.clone(),
        dead_letter: true,
    };

    // Send three messages with different shapes.
    backend.send(Command::SendMessages {
        req: backend.next_request(),
        ns,
        target: queue_path.clone(),
        messages: vec![
            OutboundMessage {
                body: r#"{"kind":"json","n":1}"#.into(),
                content_type: Some("application/json".into()),
                subject: Some("first".into()),
                application_properties: vec![("origin".into(), "sift-live-test".into())],
                ..OutboundMessage::default()
            },
            OutboundMessage {
                body: "plain text body".into(),
                subject: Some("second".into()),
                ..OutboundMessage::default()
            },
            OutboundMessage {
                body: "third".into(),
                ..OutboundMessage::default()
            },
        ],
    });
    recv_until(&events, |e| match e {
        Event::Sent { result, count, .. } => {
            result.expect("send");
            assert_eq!(count, 3);
            Some(())
        }
        _ => None,
    });

    // Peek: all three visible, JSON body decoded and pretty-printed.
    backend.send(Command::PeekMessages {
        req: backend.next_request(),
        ns,
        source: source.clone(),
        from_seq: None,
        count: 10,
    });
    let peeked = recv_until(&events, |e| match e {
        Event::Messages {
            received: false,
            result,
            ..
        } => Some(result.expect("peek")),
        _ => None,
    });
    assert_eq!(peeked.len(), 3);
    let json_msg = peeked
        .iter()
        .find(|m| m.subject.as_deref() == Some("first"))
        .expect("json message present");
    assert_eq!(json_msg.body.format, sift_core::body::BodyFormat::Json);
    assert!(json_msg.body.text.as_deref().unwrap().contains("\"n\": 1"));
    assert_eq!(
        json_msg.application_properties,
        vec![("origin".to_owned(), "sift-live-test".to_owned())]
    );

    // Receive one with a lock and dead-letter it.
    backend.send(Command::ReceiveMessages {
        req: backend.next_request(),
        ns,
        source: source.clone(),
        mode: ReceiveMode::PeekLock,
        count: 1,
    });
    let locked = recv_until(&events, |e| match e {
        Event::Messages {
            received: true,
            result,
            ..
        } => Some(result.expect("receive")),
        _ => None,
    });
    let token = locked[0].lock_token.clone().expect("lock token");
    backend.send(Command::SettleMessage {
        req: backend.next_request(),
        ns,
        source: source.clone(),
        lock_token: token,
        disposition: Disposition::DeadLetter {
            reason: Some("sift-live-test".into()),
            description: None,
        },
    });
    recv_until(&events, |e| match e {
        Event::Settled { result, .. } => {
            let _: () = result.expect("dead-letter");
            Some(())
        }
        _ => None,
    });

    // The DLQ now holds it, with the reason we set.
    backend.send(Command::PeekMessages {
        req: backend.next_request(),
        ns,
        source: dlq_source.clone(),
        from_seq: None,
        count: 10,
    });
    let dlq = recv_until(&events, |e| match e {
        Event::Messages {
            received: false,
            result,
            ..
        } => Some(result.expect("peek DLQ")),
        _ => None,
    });
    assert_eq!(dlq.len(), 1);
    assert_eq!(dlq[0].dead_letter_reason.as_deref(), Some("sift-live-test"));

    // Receive it from the DLQ and complete (permanently remove) it.
    backend.send(Command::ReceiveMessages {
        req: backend.next_request(),
        ns,
        source: dlq_source.clone(),
        mode: ReceiveMode::PeekLock,
        count: 1,
    });
    let dlq_locked = recv_until(&events, |e| match e {
        Event::Messages {
            received: true,
            result,
            ..
        } => Some(result.expect("receive DLQ")),
        _ => None,
    });
    backend.send(Command::SettleMessage {
        req: backend.next_request(),
        ns,
        source: dlq_source,
        lock_token: dlq_locked[0].lock_token.clone().expect("dlq lock token"),
        disposition: Disposition::Complete,
    });
    recv_until(&events, |e| match e {
        Event::Settled { result, .. } => {
            let _: () = result.expect("complete from DLQ");
            Some(())
        }
        _ => None,
    });

    // The main queue now holds the two messages we neither locked nor
    // dead-lettered. Peek sees through locks, so this is a stable assertion.
    backend.send(Command::PeekMessages {
        req: backend.next_request(),
        ns,
        source: source.clone(),
        from_seq: None,
        count: 10,
    });
    let remaining = recv_until(&events, |e| match e {
        Event::Messages {
            received: false,
            result,
            ..
        } => Some(result.expect("peek remaining")),
        _ => None,
    });
    assert_eq!(remaining.len(), 2);
    assert!(
        remaining.iter().all(|m| m.dead_letter_reason.is_none()),
        "dead-lettered message must not remain on the main queue"
    );

    backend.send(Command::Disconnect { ns });
    rt.block_on(mgmt.delete_queue(&queue_name))
        .expect("delete scratch queue");
    eprintln!("messaging round trip OK on '{queue_name}'");
}
