//! Reusable state and egui views for sift.
//!
//! The desktop application adds native Azure Service Bus, keyring, and file
//! dialog integrations. Browser builds reuse the same views with a local
//! controller.

#[cfg(not(target_arch = "wasm32"))]
pub mod app;
pub mod icons;
#[cfg(not(target_arch = "wasm32"))]
pub mod logging;
#[cfg(not(target_arch = "wasm32"))]
mod message_files;
pub mod state;
pub mod ui;
