//! Client for the Azure Service Bus ATOM-XML management REST API — the same
//! namespace-level API that .NET's `ServiceBusAdministrationClient` wraps.
//! Rust has no official equivalent, so sift implements it in-house.
//!
//! Phase 0 covers authentication plumbing and `GET /$namespaceinfo`; entity
//! CRUD arrives in Phase 1.

mod atom;
pub mod client;
pub mod error;
pub mod model;
pub mod transfer;
mod write;

pub use client::{Authorizer, ManagementClient};
pub use error::MgmtError;
pub use model::{
    EntityRuntimeInfo, EntityStatus, MessageCountDetails, NamespaceInfo, QueueInfo,
    QueueProperties, RuleFilter, RuleInfo, RuleProperties, SubscriptionInfo,
    SubscriptionProperties, TopicInfo, TopicProperties, format_iso8601, is_unlimited,
    parse_iso8601, unlimited,
};
pub use transfer::{ImportOutcome, ImportPolicy, NamespaceExport};
