//! Live end-to-end messaging test against a real Service Bus namespace,
//! driving the backend through its real command/event channels.
//!
//! Self-skips unless `SIFT_TEST_SB_CONNECTION_STRING` is set and
//! `SIFT_TEST_SB_MUTATE=1`. Only touches a uniquely-named `sift-test-*`
//! queue, which is created and deleted by the test itself.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sift_backend::{
    Command, Disposition, EntityPath, Event, MessageSource, OpSummary, ReceiveMode,
};
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

#[allow(clippy::too_many_lines)] // a linear end-to-end scenario reads best in one function
#[test]
fn live_purge_and_resubmit() {
    let Ok(connection_string) = std::env::var("SIFT_TEST_SB_CONNECTION_STRING") else {
        eprintln!("skipped: SIFT_TEST_SB_CONNECTION_STRING not set");
        return;
    };
    if std::env::var("SIFT_TEST_SB_MUTATE").as_deref() != Ok("1") {
        eprintln!("skipped: SIFT_TEST_SB_MUTATE != 1");
        return;
    }

    let conn = NamespaceConnection::parse(&connection_string).expect("valid connection string");
    let mgmt = ManagementClient::new(&conn).expect("management client");
    let queue_name = format!("sift-test-op-{}", uuid::Uuid::new_v4());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime for mgmt calls");
    rt.block_on(mgmt.create_queue(&QueueProperties {
        name: queue_name.clone(),
        ..QueueProperties::default()
    }))
    .expect("create scratch queue");

    let (backend, events) = sift_backend::spawn(Arc::new(|| {}));
    let profile = NamespaceProfile::new_connection_string("live-op-test".into());
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
    let main = MessageSource {
        entity: queue_path.clone(),
        dead_letter: false,
    };
    let dlq = MessageSource {
        entity: queue_path.clone(),
        dead_letter: true,
    };
    let send_n = |n: usize| {
        backend.send(Command::SendMessages {
            req: backend.next_request(),
            ns,
            target: queue_path.clone(),
            messages: (0..n)
                .map(|i| OutboundMessage {
                    body: format!("msg {i}"),
                    ..OutboundMessage::default()
                })
                .collect(),
        });
        recv_until(&events, |e| match e {
            Event::Sent { result, .. } => {
                result.expect("send");
                Some(())
            }
            _ => None,
        });
    };
    let op_summary = |events: &crossbeam_channel::Receiver<Event>| -> Result<OpSummary, String> {
        recv_until(events, |e| match e {
            Event::OpFinished { result, .. } => Some(result.map_err(|e| e.message)),
            _ => None,
        })
    };

    // Purge: send 5, drain them all.
    send_n(5);
    backend.send(Command::StartPurge {
        op: backend.next_op(),
        ns,
        source: main.clone(),
    });
    let purged = op_summary(&events).expect("purge");
    assert_eq!(purged.processed, 5);

    // Resubmit: send 3, dead-letter each, then move them back to the queue.
    send_n(3);
    let mut dead_lettered = 0;
    while dead_lettered < 3 {
        backend.send(Command::ReceiveMessages {
            req: backend.next_request(),
            ns,
            source: main.clone(),
            mode: ReceiveMode::PeekLock,
            count: 3,
        });
        let batch = recv_until(&events, |e| match e {
            Event::Messages {
                received: true,
                result,
                ..
            } => Some(result.expect("receive")),
            _ => None,
        });
        for message in batch {
            backend.send(Command::SettleMessage {
                req: backend.next_request(),
                ns,
                source: main.clone(),
                lock_token: message.lock_token.clone().expect("lock token"),
                disposition: Disposition::DeadLetter {
                    reason: Some("sift-op-test".into()),
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
            dead_lettered += 1;
        }
    }

    backend.send(Command::StartResubmit {
        op: backend.next_op(),
        ns,
        source: dlq,
        target: queue_path.clone(),
    });
    let resubmitted = op_summary(&events).expect("resubmit");
    assert_eq!(resubmitted.processed, 3);

    // The 3 resubmitted messages are back on the main queue (fresh, no DLQ mark).
    backend.send(Command::PeekMessages {
        req: backend.next_request(),
        ns,
        source: main,
        from_seq: None,
        count: 10,
    });
    let back = recv_until(&events, |e| match e {
        Event::Messages {
            received: false,
            result,
            ..
        } => Some(result.expect("peek after resubmit")),
        _ => None,
    });
    assert_eq!(back.len(), 3);
    assert!(back.iter().all(|m| m.dead_letter_reason.is_none()));

    backend.send(Command::Disconnect { ns });
    rt.block_on(mgmt.delete_queue(&queue_name))
        .expect("delete scratch queue");
    eprintln!("purge + resubmit OK on '{queue_name}'");
}

#[allow(clippy::too_many_lines)] // a linear end-to-end scenario reads best in one function
#[test]
fn live_scheduled_deferred_export() {
    let Ok(connection_string) = std::env::var("SIFT_TEST_SB_CONNECTION_STRING") else {
        eprintln!("skipped: SIFT_TEST_SB_CONNECTION_STRING not set");
        return;
    };
    if std::env::var("SIFT_TEST_SB_MUTATE").as_deref() != Ok("1") {
        eprintln!("skipped: SIFT_TEST_SB_MUTATE != 1");
        return;
    }

    let conn = NamespaceConnection::parse(&connection_string).expect("valid connection string");
    let mgmt = ManagementClient::new(&conn).expect("management client");
    let queue_name = format!("sift-test-sd-{}", uuid::Uuid::new_v4());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime for mgmt calls");
    rt.block_on(mgmt.create_queue(&QueueProperties {
        name: queue_name.clone(),
        ..QueueProperties::default()
    }))
    .expect("create scratch queue");

    let (backend, events) = sift_backend::spawn(Arc::new(|| {}));
    let profile = NamespaceProfile::new_connection_string("live-sd-test".into());
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

    // Schedule a message an hour out; it should come back as a sequence number.
    backend.send(Command::ScheduleMessages {
        req: backend.next_request(),
        ns,
        target: queue_path.clone(),
        messages: vec![OutboundMessage {
            body: "scheduled".into(),
            ..OutboundMessage::default()
        }],
        enqueue_at: time::OffsetDateTime::now_utc() + time::Duration::hours(1),
    });
    let seqs = recv_until(&events, |e| match e {
        Event::Sent { result, .. } => Some(result.expect("schedule")),
        _ => None,
    });
    assert_eq!(seqs.len(), 1);

    // Peek sees it in the scheduled state.
    backend.send(Command::PeekMessages {
        req: backend.next_request(),
        ns,
        source: source.clone(),
        from_seq: None,
        count: 10,
    });
    let scheduled = recv_until(&events, |e| match e {
        Event::Messages {
            received: false,
            result,
            ..
        } => Some(result.expect("peek scheduled")),
        _ => None,
    });
    assert!(
        scheduled
            .iter()
            .any(|m| m.state == sift_core::message::MessageState::Scheduled)
    );

    // Cancel it by sequence number.
    backend.send(Command::CancelScheduled {
        req: backend.next_request(),
        ns,
        target: queue_path.clone(),
        sequence_number: seqs[0],
    });
    recv_until(&events, |e| match e {
        Event::ScheduledCancelled { result, .. } => {
            let _: () = result.expect("cancel scheduled");
            Some(())
        }
        _ => None,
    });

    // Defer round-trip: send, lock, defer, retrieve by sequence number.
    backend.send(Command::SendMessages {
        req: backend.next_request(),
        ns,
        target: queue_path.clone(),
        messages: vec![OutboundMessage {
            body: "deferred".into(),
            ..OutboundMessage::default()
        }],
    });
    recv_until(&events, |e| match e {
        Event::Sent { result, .. } => Some(result.expect("send")),
        _ => None,
    });
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
    let deferred_seq = locked[0].sequence_number;
    backend.send(Command::SettleMessage {
        req: backend.next_request(),
        ns,
        source: source.clone(),
        lock_token: locked[0].lock_token.clone().expect("lock token"),
        disposition: Disposition::Defer,
    });
    recv_until(&events, |e| match e {
        Event::Settled { result, .. } => {
            let _: () = result.expect("defer");
            Some(())
        }
        _ => None,
    });

    backend.send(Command::ReceiveDeferred {
        req: backend.next_request(),
        ns,
        source: source.clone(),
        sequence_numbers: vec![deferred_seq],
    });
    let retrieved = recv_until(&events, |e| match e {
        Event::Messages {
            received: true,
            result,
            ..
        } => Some(result.expect("receive deferred")),
        _ => None,
    });
    assert_eq!(retrieved.len(), 1);
    assert_eq!(retrieved[0].sequence_number, deferred_seq);
    // Complete it so nothing lingers.
    backend.send(Command::SettleMessage {
        req: backend.next_request(),
        ns,
        source,
        lock_token: retrieved[0]
            .lock_token
            .clone()
            .expect("deferred lock token"),
        disposition: Disposition::Complete,
    });
    recv_until(&events, |e| match e {
        Event::Settled { result, .. } => {
            let _: () = result.expect("complete deferred");
            Some(())
        }
        _ => None,
    });

    // Export the namespace and confirm the scratch queue is in the file.
    let export_path =
        std::env::temp_dir().join(format!("sift-export-{}.json", uuid::Uuid::new_v4()));
    backend.send(Command::ExportNamespace {
        req: backend.next_request(),
        ns,
        path: export_path.clone(),
    });
    recv_until(&events, |e| match e {
        Event::NamespaceTransfer { result, .. } => Some(result.expect("export")),
        _ => None,
    });
    let exported = std::fs::read_to_string(&export_path).expect("read export file");
    assert!(exported.contains(&queue_name));
    let _ = std::fs::remove_file(&export_path);

    backend.send(Command::Disconnect { ns });
    rt.block_on(mgmt.delete_queue(&queue_name))
        .expect("delete scratch queue");
    eprintln!("scheduled + deferred + export OK on '{queue_name}'");
}
