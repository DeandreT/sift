//! The async half of sift: a tokio runtime on a dedicated thread, driven by
//! [`bridge::Command`] messages from the UI and answering with
//! [`bridge::Event`] messages.
//!
//! The contract with the GUI: the UI thread never blocks, and this crate
//! never touches UI state. All communication crosses one pair of channels.

#[cfg(not(target_arch = "wasm32"))]
pub mod backend;
pub mod bridge;
#[cfg(not(target_arch = "wasm32"))]
mod sb_runtime;

#[cfg(not(target_arch = "wasm32"))]
pub use backend::{RepaintFn, spawn};
#[cfg(not(target_arch = "wasm32"))]
pub use bridge::BackendHandle;
pub use bridge::{
    BackendError, Command, Disposition, EntityDescription, EntityInfo, EntityPath, Event,
    MessageSource, MutationOp, NamespaceId, OpId, OpKind, OpSummary, ReceiveMode, RequestId,
    SessionSnapshot,
};
pub use sift_mgmt::NamespaceExport;
