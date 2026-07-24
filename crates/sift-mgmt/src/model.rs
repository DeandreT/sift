//! Entity models for the management API. Naming follows .NET's
//! `ServiceBusAdministrationClient` (`XxxProperties` for user-settable fields,
//! `XxxRuntimeInfo` for server counters). Phase 0 only needs namespace info.

use time::OffsetDateTime;

/// Result of `GET /$namespaceinfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamespaceInfo {
    pub name: String,
    pub alias: Option<String>,
    /// `Messaging`, `EventHub`, `NotificationHub`, `Relay`, or `Mixed`.
    pub namespace_type: Option<String>,
    /// `Basic`, `Standard`, or `Premium`.
    pub messaging_sku: Option<String>,
    pub messaging_units: Option<u32>,
    pub created_time: Option<OffsetDateTime>,
    pub modified_time: Option<OffsetDateTime>,
}
