//! UI-side application state: plain data owned by the app, mutated only on
//! the UI thread.

use sift_backend::RequestId;
use sift_mgmt::NamespaceInfo;
use uuid::Uuid;

/// Where we are with the (single, for now) namespace connection.
#[derive(Debug, Clone, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting {
        profile_id: Uuid,
        name: String,
    },
    Connected {
        profile_id: Uuid,
        name: String,
        info: NamespaceInfo,
    },
}

/// An in-flight connect request, used to match the response event.
#[derive(Debug)]
pub struct PendingConnect {
    pub req: RequestId,
    pub name: String,
}

/// Intents emitted by UI widgets during draw and executed by the app
/// afterwards, so widgets never mutate app state mid-frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    OpenConnectDialog,
    Disconnect,
    ImportLegacyProfiles,
}
