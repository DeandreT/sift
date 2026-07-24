//! The backend runtime: a dedicated thread running tokio, processing
//! [`Command`]s and emitting [`Event`]s.

use std::collections::HashMap;
use std::sync::Arc;

use sift_core::config::AuthMethod;
use sift_core::connection::NamespaceConnection;
use sift_mgmt::ManagementClient;
use tokio::sync::Mutex;

use crate::bridge::{BackendError, BackendHandle, Command, Event, NamespaceId};

/// Called after every event so the UI repaints promptly; the GUI passes
/// `egui::Context::request_repaint` without this crate depending on egui.
pub type RepaintFn = Arc<dyn Fn() + Send + Sync>;

/// Start the backend thread. Returns the command handle and the event
/// receiver the UI drains each frame.
#[must_use]
pub fn spawn(repaint: RepaintFn) -> (BackendHandle, crossbeam_channel::Receiver<Event>) {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (evt_tx, evt_rx) = crossbeam_channel::unbounded();

    std::thread::Builder::new()
        .name("sift-backend".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("failed to build the tokio runtime");
            runtime.block_on(run(cmd_rx, EventSink { evt_tx, repaint }));
        })
        .expect("failed to spawn the backend thread");

    (BackendHandle::new(cmd_tx), evt_rx)
}

/// Sends events to the UI and wakes it up.
#[derive(Clone)]
struct EventSink {
    evt_tx: crossbeam_channel::Sender<Event>,
    repaint: RepaintFn,
}

impl EventSink {
    fn send(&self, event: Event) {
        if self.evt_tx.send(event).is_ok() {
            (self.repaint)();
        }
    }
}

#[derive(Default)]
struct State {
    namespaces: HashMap<NamespaceId, Arc<ManagementClient>>,
}

async fn run(mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Command>, sink: EventSink) {
    let state = Arc::new(Mutex::new(State::default()));
    tracing::debug!("backend runtime started");

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Command::Connect {
                req,
                profile,
                secret,
            } => {
                let sink = sink.clone();
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let ns = profile.id;
                    let result = connect(&profile.auth, &secret, &state, ns).await;
                    match &result {
                        Ok(info) => tracing::info!(
                            namespace = %info.name,
                            profile = %profile.name,
                            "connected"
                        ),
                        Err(e) => tracing::error!(
                            profile = %profile.name,
                            error = %e,
                            "connection failed"
                        ),
                    }
                    sink.send(Event::Connected { req, ns, result });
                });
            }
            Command::Disconnect { ns } => {
                state.lock().await.namespaces.remove(&ns);
                tracing::info!(%ns, "disconnected");
                sink.send(Event::Disconnected { ns });
            }
            Command::Shutdown => break,
        }
    }
    tracing::debug!("backend runtime stopped");
}

async fn connect(
    auth: &AuthMethod,
    secret: &sift_core::secrets::SecretString,
    state: &Mutex<State>,
    ns: NamespaceId,
) -> Result<sift_mgmt::NamespaceInfo, BackendError> {
    let AuthMethod::ConnectionString = auth else {
        return Err(BackendError::new(
            "Microsoft Entra ID authentication is not implemented yet",
        ));
    };

    let conn = NamespaceConnection::parse(secret.expose())
        .map_err(|e| BackendError::new(e.to_string()))?;
    for warning in &conn.warnings {
        tracing::warn!("{warning}");
    }

    let client = ManagementClient::new(&conn).map_err(|e| BackendError::new(e.to_string()))?;
    let info = client.get_namespace_info().await.map_err(|e| {
        let detail = match &e {
            sift_mgmt::MgmtError::Unauthorized { detail }
            | sift_mgmt::MgmtError::Forbidden { detail }
            | sift_mgmt::MgmtError::Conflict { detail }
            | sift_mgmt::MgmtError::BadRequest { detail }
            | sift_mgmt::MgmtError::Throttled { detail }
            | sift_mgmt::MgmtError::Server { detail, .. } => Some(detail.clone()),
            _ => None,
        };
        match detail {
            Some(detail) if !detail.is_empty() => BackendError::with_detail(e.to_string(), detail),
            _ => BackendError::new(e.to_string()),
        }
    })?;

    state.lock().await.namespaces.insert(ns, Arc::new(client));
    Ok(info)
}
