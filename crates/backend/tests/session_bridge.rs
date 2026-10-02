use std::sync::Arc;
use std::time::Duration;

use sift_backend::{Command, Disposition, EntityPath, Event, MessageSource, RequestId};

#[test]
fn disconnected_session_commands_fail_and_cleanup_stops_backend() {
    let (backend, events) = sift_backend::spawn(Arc::new(|| {}));
    let ns = uuid::Uuid::new_v4();
    let lease_id = uuid::Uuid::new_v4();
    let source = MessageSource {
        entity: EntityPath::Queue("orders".into()),
        dead_letter: false,
    };
    for command in [
        Command::BrowseSession {
            req: RequestId(1),
            ns,
            source: source.clone(),
            session_id: None,
            count: 10,
        },
        Command::ReceiveSession {
            req: RequestId(2),
            ns,
            source: source.clone(),
            lease_id,
            count: 10,
            sequence_numbers: Vec::new(),
        },
        Command::RenewSession {
            req: RequestId(3),
            ns,
            source: source.clone(),
            lease_id,
            lock_token: None,
        },
        Command::SettleSessionMessage {
            req: RequestId(4),
            ns,
            source: source.clone(),
            lease_id,
            lock_token: "expired".into(),
            disposition: Disposition::Complete,
        },
    ] {
        backend.send(command);
    }
    let mut requests = Vec::new();
    for _ in 0..4 {
        let event = events
            .recv_timeout(Duration::from_secs(5))
            .expect("session command response");
        let (request, response_ns, response_source, error) = match event {
            Event::Session {
                req,
                ns,
                source,
                result,
            } => (req, ns, source, result.expect_err("disconnected")),
            Event::SessionSettled {
                req,
                ns,
                source,
                result,
                ..
            } => (req, ns, source, result.expect_err("disconnected")),
            other => panic!("unexpected response: {other:?}"),
        };
        assert_eq!(response_ns, ns);
        assert_eq!(response_source, source);
        assert_eq!(error.message, "not connected to this namespace");
        requests.push(request.0);
    }
    requests.sort_unstable();
    assert_eq!(requests, vec![1, 2, 3, 4]);
    backend.send(Command::ReleaseSession {
        ns,
        source,
        lease_id,
    });
    backend.send(Command::Shutdown);
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(5)),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected)
    ));
}
