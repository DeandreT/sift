//! Core domain logic for sift: connection-string parsing, SAS token generation,
//! application configuration, and secret storage.
//!
//! This crate is deliberately free of async runtimes, Azure SDKs, and GUI
//! dependencies so that every piece of it is unit-testable in isolation.

pub mod body;
pub mod config;
pub mod connection;
pub mod legacy_import;
pub mod message;
pub mod sas;
pub mod secrets;

pub use config::{AppConfig, AuthMethod, NamespaceProfile};
pub use connection::{Credential, NamespaceConnection, TransportType};
pub use sas::{SasToken, SasTokenProvider};
pub use secrets::{SecretKind, SecretRef, SecretStore, SecretString};
