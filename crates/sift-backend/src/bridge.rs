//! The Command/Event vocabulary shared between the UI and the backend, plus
//! the handle the UI uses to talk to the backend.
//!
//! Phase 0 carries only connection commands; the enums grow with each phase
//! (entity CRUD, messaging, long-running operations, streams).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sift_core::config::NamespaceProfile;
use sift_core::secrets::SecretString;
use sift_mgmt::NamespaceInfo;
use uuid::Uuid;

/// Correlates a one-shot request with its response event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

/// Namespaces are identified by their profile id.
pub type NamespaceId = Uuid;

/// UI → backend.
#[derive(Debug)]
pub enum Command {
    /// Resolve the profile's secret, build a management client, and validate
    /// the connection with `GET /$namespaceinfo`.
    Connect {
        req: RequestId,
        profile: NamespaceProfile,
        secret: SecretString,
    },
    Disconnect {
        ns: NamespaceId,
    },
    Shutdown,
}

/// Backend → UI.
#[derive(Debug)]
pub enum Event {
    Connected {
        req: RequestId,
        ns: NamespaceId,
        result: Result<NamespaceInfo, BackendError>,
    },
    Disconnected {
        ns: NamespaceId,
    },
}

/// A user-presentable error from a backend operation.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct BackendError {
    pub message: String,
    /// Raw server/library detail for a "show details" expander.
    pub detail: Option<String>,
}

impl BackendError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: None,
        }
    }

    #[must_use]
    pub fn with_detail(message: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: Some(detail.into()),
        }
    }
}

/// Cloneable handle the UI uses to mint request ids and send commands.
#[derive(Debug, Clone)]
pub struct BackendHandle {
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>,
    next_id: Arc<AtomicU64>,
}

impl BackendHandle {
    pub(crate) fn new(cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>) -> Self {
        Self {
            cmd_tx,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    #[must_use]
    pub fn next_request(&self) -> RequestId {
        RequestId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Send a command. The backend outlives the UI, so a closed channel only
    /// happens during shutdown and is logged rather than surfaced.
    pub fn send(&self, cmd: Command) {
        if self.cmd_tx.send(cmd).is_err() {
            tracing::warn!("backend command channel is closed; command dropped");
        }
    }
}
