//! The async half of sift: a tokio runtime on a dedicated thread, driven by
//! [`bridge::Command`] messages from the UI and answering with
//! [`bridge::Event`] messages.
//!
//! The contract with the GUI: the UI thread never blocks, and this crate
//! never touches UI state. All communication crosses one pair of channels.

pub mod backend;
pub mod bridge;

pub use backend::{RepaintFn, spawn};
pub use bridge::{
    BackendError, BackendHandle, Command, EntityDescription, EntityInfo, EntityPath, Event,
    MutationOp, NamespaceId, RequestId,
};
