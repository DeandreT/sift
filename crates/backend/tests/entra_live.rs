//! Opt-in Entra management and AMQP verification using an Azure CLI account.
//! Set `SIFT_TEST_ENTRA_NAMESPACE`; mutations additionally require
//! `SIFT_TEST_SB_MUTATE=1` and use a uniquely named queue with cleanup on panic.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sift_backend::{Command, Disposition, EntityPath, Event, MessageSource, ReceiveMode};
use sift_core::config::{AuthMethod, NamespaceProfile};
use sift_core::connection::namespace_endpoint;
use sift_core::message::OutboundMessage;
use sift_core::secrets::SecretString;
use sift_mgmt::{ManagementClient, QueueProperties};

struct Scratch {
    client: ManagementClient,
    runtime: tokio::runtime::Runtime,
    queue: Option<String>,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(queue) = &self.queue
            && let Err(error) = self.runtime.block_on(self.client.delete_queue(queue))
        {
            eprintln!("could not clean up {queue}: {error}");
        }
    }
}

fn wait<T>(
    events: &crossbeam_channel::Receiver<Event>,
    mut pick: impl FnMut(Event) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("backend response before timeout");
        if let Some(value) = pick(events.recv_timeout(remaining).expect("backend event")) {
            return value;
        }
    }
}

#[allow(clippy::too_many_lines)]
#[test]
fn live_entra_management_and_messaging() {
    let Ok(namespace) = std::env::var("SIFT_TEST_ENTRA_NAMESPACE") else {
        eprintln!("skipped: SIFT_TEST_ENTRA_NAMESPACE not set");
        return;
    };
    let endpoint = namespace_endpoint(&namespace).expect("namespace endpoint");
    let tenant_id = std::env::var("SIFT_TEST_ENTRA_TENANT").ok();
    let credential = sift_mgmt::auth::azure_cli(tenant_id.clone()).expect("Entra credential");
    let mut scratch = Scratch {
        client: ManagementClient::new_with_credential(&endpoint, credential)
            .expect("management client"),
        runtime: tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime"),
        queue: None,
    };
    scratch
        .runtime
        .block_on(scratch.client.get_namespace_info())
        .expect("Entra management authorization");
    let mut profile = NamespaceProfile::new_connection_string("Entra integration".into());
    profile.auth = AuthMethod::AzureAd { tenant_id };
    profile.endpoint = Some(endpoint);
    let ns = profile.id;
    let (backend, events) = sift_backend::spawn(Arc::new(|| {}));
    backend.send(Command::Connect {
        req: backend.next_request(),
        profile,
        secret: SecretString::default(),
    });
    wait(&events, |event| match event {
        Event::Connected { result, .. } => Some(result.expect("Entra backend connection")),
        _ => None,
    });
    if std::env::var("SIFT_TEST_SB_MUTATE").as_deref() != Ok("1") {
        backend.send(Command::Shutdown);
        eprintln!(
            "read-only Entra management passed; AMQP mutation skipped: SIFT_TEST_SB_MUTATE != 1"
        );
        return;
    }
    let queue = format!("sift-test-entra-{}", uuid::Uuid::new_v4());
    scratch.queue = Some(queue.clone());
    scratch
        .runtime
        .block_on(scratch.client.create_queue(&QueueProperties {
            name: queue.clone(),
            ..Default::default()
        }))
        .expect("scratch queue");
    let target = EntityPath::Queue(queue);
    let source = MessageSource {
        entity: target.clone(),
        dead_letter: false,
    };
    backend.send(Command::SendMessages {
        req: backend.next_request(),
        ns,
        target,
        messages: vec![OutboundMessage {
            body: "Entra round trip".into(),
            ..Default::default()
        }],
    });
    wait(&events, |event| match event {
        Event::Sent { result, .. } => Some(result.expect("Entra AMQP send")),
        _ => None,
    });
    backend.send(Command::ReceiveMessages {
        req: backend.next_request(),
        ns,
        source: source.clone(),
        mode: ReceiveMode::PeekLock,
        count: 1,
    });
    let messages = wait(&events, |event| match event {
        Event::Messages {
            received: true,
            result,
            ..
        } => Some(result.expect("Entra AMQP receive")),
        _ => None,
    });
    assert_eq!(messages.len(), 1);
    let lock_token = messages[0].lock_token.clone().expect("lock token");
    backend.send(Command::SettleMessage {
        req: backend.next_request(),
        ns,
        source,
        lock_token,
        disposition: Disposition::Complete,
    });
    wait(&events, |event| match event {
        Event::Settled { result, .. } => {
            result.expect("Entra AMQP settlement");
            Some(())
        }
        _ => None,
    });
    backend.send(Command::Shutdown);
}
