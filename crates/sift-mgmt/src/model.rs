//! Entity models for the management API. Naming follows .NET's
//! `ServiceBusAdministrationClient` (`XxxProperties` for user-settable fields,
//! `XxxRuntimeInfo` for server counters, `XxxInfo` for both).

use std::fmt;
use std::time::Duration;

use time::OffsetDateTime;

/// The `.NET TimeSpan.MaxValue` sentinel the service uses for "unlimited"
/// durations. Round-tripped verbatim: anything at or above it formats back to
/// this exact literal.
pub const TIMESPAN_MAX: &str = "P10675199DT2H48M5.4775807S";
const TIMESPAN_MAX_SECS: u64 = 10_675_199 * 86_400 + 2 * 3_600 + 48 * 60 + 5;

/// Duration used in entity descriptions, serialized as ISO-8601.
#[must_use]
pub fn unlimited() -> Duration {
    Duration::new(TIMESPAN_MAX_SECS, 477_580_700)
}

/// True when a duration represents the service's "unlimited" sentinel.
#[must_use]
pub fn is_unlimited(d: Duration) -> bool {
    d.as_secs() >= TIMESPAN_MAX_SECS
}

/// Format a duration as the ISO-8601 subset the service emits
/// (`P{d}DT{h}H{m}M{s(.f)}S`, e.g. `PT1M`, `P14D`, `PT16S`).
#[must_use]
pub fn format_iso8601(d: Duration) -> String {
    use std::fmt::Write as _;

    if is_unlimited(d) {
        return TIMESPAN_MAX.to_owned();
    }
    let total = d.as_secs();
    let days = total / 86_400;
    let hours = (total % 86_400) / 3_600;
    let minutes = (total % 3_600) / 60;
    let seconds = total % 60;
    let nanos = d.subsec_nanos();

    let mut out = String::from("P");
    if days > 0 {
        let _ = write!(out, "{days}D");
    }
    if hours > 0 || minutes > 0 || seconds > 0 || nanos > 0 || days == 0 {
        out.push('T');
        if hours > 0 {
            let _ = write!(out, "{hours}H");
        }
        if minutes > 0 {
            let _ = write!(out, "{minutes}M");
        }
        if seconds > 0 || nanos > 0 || (hours == 0 && minutes == 0) {
            if nanos > 0 {
                let frac = format!("{nanos:09}");
                let _ = write!(out, "{seconds}.{}S", frac.trim_end_matches('0'));
            } else {
                let _ = write!(out, "{seconds}S");
            }
        }
    }
    out
}

/// Parse the ISO-8601 duration subset the service emits. Years/months are not
/// produced by Service Bus and are rejected.
// Fractional seconds are always 0 ≤ f < 1, so the f64→int casts cannot
// truncate meaningfully or go negative.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#[must_use]
pub fn parse_iso8601(s: &str) -> Option<Duration> {
    let rest = s.strip_prefix('P')?;
    let (date_part, time_part) = match rest.split_once('T') {
        Some((d, t)) => (d, t),
        None => (rest, ""),
    };

    let mut secs = 0u64;
    let mut nanos = 0u32;

    let mut parse_segments = |part: &str, is_time: bool| -> Option<()> {
        let mut number = String::new();
        for c in part.chars() {
            if c.is_ascii_digit() || c == '.' {
                number.push(c);
            } else {
                let unit_secs: u64 = match (c, is_time) {
                    ('D', false) => 86_400,
                    ('H', true) => 3_600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None, // years/months/unknown units
                };
                if c == 'S' && number.contains('.') {
                    let value: f64 = number.parse().ok()?;
                    secs += value.trunc() as u64;
                    nanos += (value.fract() * 1e9).round() as u32;
                } else {
                    let value: u64 = number.parse().ok()?;
                    secs += value * unit_secs;
                }
                number.clear();
            }
        }
        number.is_empty().then_some(())
    };

    parse_segments(date_part, false)?;
    parse_segments(time_part, true)?;
    Some(Duration::new(secs, nanos))
}

/// Entity status, matching the service's `EntityStatus` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum EntityStatus {
    #[default]
    Active,
    Disabled,
    SendDisabled,
    ReceiveDisabled,
}

impl EntityStatus {
    pub const ALL: [Self; 4] = [
        Self::Active,
        Self::Disabled,
        Self::SendDisabled,
        Self::ReceiveDisabled,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Disabled => "Disabled",
            Self::SendDisabled => "SendDisabled",
            Self::ReceiveDisabled => "ReceiveDisabled",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "Disabled" => Self::Disabled,
            "SendDisabled" => Self::SendDisabled,
            "ReceiveDisabled" => Self::ReceiveDisabled,
            _ => Self::Active,
        }
    }
}

impl fmt::Display for EntityStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Per-state message counts (`<CountDetails>`), the dashboard's data source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MessageCountDetails {
    pub active: i64,
    pub dead_letter: i64,
    pub scheduled: i64,
    pub transfer: i64,
    pub transfer_dead_letter: i64,
}

impl MessageCountDetails {
    #[must_use]
    pub fn total(&self) -> i64 {
        self.active + self.dead_letter + self.scheduled + self.transfer + self.transfer_dead_letter
    }
}

/// Server-maintained fields common to queues and subscriptions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntityRuntimeInfo {
    pub message_count: i64,
    pub size_in_bytes: i64,
    pub count_details: MessageCountDetails,
    pub created_at: Option<OffsetDateTime>,
    pub updated_at: Option<OffsetDateTime>,
    pub accessed_at: Option<OffsetDateTime>,
}

// ---------------------------------------------------------------------------
// Queue

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct QueueProperties {
    pub name: String,
    pub lock_duration: Duration,
    pub max_size_in_megabytes: i64,
    pub requires_duplicate_detection: bool,
    pub requires_session: bool,
    pub default_message_time_to_live: Duration,
    pub dead_lettering_on_message_expiration: bool,
    pub duplicate_detection_history_time_window: Duration,
    pub max_delivery_count: i32,
    pub enable_batched_operations: bool,
    pub status: EntityStatus,
    pub forward_to: Option<String>,
    pub user_metadata: Option<String>,
    pub auto_delete_on_idle: Duration,
    pub enable_partitioning: bool,
    pub enable_express: bool,
    pub forward_dead_lettered_messages_to: Option<String>,
    pub max_message_size_in_kilobytes: Option<i64>,
}

impl Default for QueueProperties {
    fn default() -> Self {
        Self {
            name: String::new(),
            lock_duration: Duration::from_mins(1),
            max_size_in_megabytes: 1024,
            requires_duplicate_detection: false,
            requires_session: false,
            default_message_time_to_live: unlimited(),
            dead_lettering_on_message_expiration: false,
            duplicate_detection_history_time_window: Duration::from_mins(10),
            max_delivery_count: 10,
            enable_batched_operations: true,
            status: EntityStatus::Active,
            forward_to: None,
            user_metadata: None,
            auto_delete_on_idle: unlimited(),
            enable_partitioning: false,
            enable_express: false,
            forward_dead_lettered_messages_to: None,
            max_message_size_in_kilobytes: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueueInfo {
    pub properties: QueueProperties,
    pub runtime: EntityRuntimeInfo,
}

// ---------------------------------------------------------------------------
// Topic

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TopicProperties {
    pub name: String,
    pub default_message_time_to_live: Duration,
    pub max_size_in_megabytes: i64,
    pub requires_duplicate_detection: bool,
    pub duplicate_detection_history_time_window: Duration,
    pub enable_batched_operations: bool,
    pub status: EntityStatus,
    pub support_ordering: bool,
    pub auto_delete_on_idle: Duration,
    pub enable_partitioning: bool,
    pub enable_express: bool,
    pub user_metadata: Option<String>,
    pub max_message_size_in_kilobytes: Option<i64>,
}

impl Default for TopicProperties {
    fn default() -> Self {
        Self {
            name: String::new(),
            default_message_time_to_live: unlimited(),
            max_size_in_megabytes: 1024,
            requires_duplicate_detection: false,
            duplicate_detection_history_time_window: Duration::from_mins(10),
            enable_batched_operations: true,
            status: EntityStatus::Active,
            support_ordering: false,
            auto_delete_on_idle: unlimited(),
            enable_partitioning: false,
            enable_express: false,
            user_metadata: None,
            max_message_size_in_kilobytes: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TopicInfo {
    pub properties: TopicProperties,
    pub subscription_count: i64,
    pub size_in_bytes: i64,
    pub scheduled_message_count: i64,
    pub created_at: Option<OffsetDateTime>,
    pub updated_at: Option<OffsetDateTime>,
    pub accessed_at: Option<OffsetDateTime>,
}

// ---------------------------------------------------------------------------
// Subscription

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SubscriptionProperties {
    /// Parent topic path.
    pub topic: String,
    pub name: String,
    pub lock_duration: Duration,
    pub requires_session: bool,
    pub default_message_time_to_live: Duration,
    pub dead_lettering_on_message_expiration: bool,
    pub dead_lettering_on_filter_evaluation_exceptions: bool,
    pub max_delivery_count: i32,
    pub enable_batched_operations: bool,
    pub status: EntityStatus,
    pub forward_to: Option<String>,
    pub user_metadata: Option<String>,
    pub auto_delete_on_idle: Duration,
    pub forward_dead_lettered_messages_to: Option<String>,
}

impl Default for SubscriptionProperties {
    fn default() -> Self {
        Self {
            topic: String::new(),
            name: String::new(),
            lock_duration: Duration::from_mins(1),
            requires_session: false,
            default_message_time_to_live: unlimited(),
            dead_lettering_on_message_expiration: false,
            dead_lettering_on_filter_evaluation_exceptions: true,
            max_delivery_count: 10,
            enable_batched_operations: true,
            status: EntityStatus::Active,
            forward_to: None,
            user_metadata: None,
            auto_delete_on_idle: unlimited(),
            forward_dead_lettered_messages_to: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubscriptionInfo {
    pub properties: SubscriptionProperties,
    pub runtime: EntityRuntimeInfo,
}

// ---------------------------------------------------------------------------
// Rule

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RuleFilter {
    Sql {
        expression: String,
    },
    Correlation {
        correlation_id: Option<String>,
        message_id: Option<String>,
        to: Option<String>,
        reply_to: Option<String>,
        subject: Option<String>,
        session_id: Option<String>,
        reply_to_session_id: Option<String>,
        content_type: Option<String>,
        properties: Vec<(String, String)>,
    },
    True,
    False,
}

impl RuleFilter {
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Sql { expression } => expression.clone(),
            Self::Correlation { .. } => "correlation filter".to_owned(),
            Self::True => "1=1 (true)".to_owned(),
            Self::False => "1=0 (false)".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuleProperties {
    pub topic: String,
    pub subscription: String,
    pub name: String,
    pub filter: RuleFilter,
    /// SQL rule action expression, if any.
    pub action: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleInfo {
    pub properties: RuleProperties,
    pub created_at: Option<OffsetDateTime>,
}

// ---------------------------------------------------------------------------
// Namespace

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

#[cfg(test)]
mod tests {
    use super::*;

    // `Duration::from_days` is still unstable; seconds are fine in tests.
    #[allow(clippy::duration_suboptimal_units)]
    #[test]
    fn formats_common_durations() {
        assert_eq!(format_iso8601(Duration::from_mins(1)), "PT1M");
        assert_eq!(format_iso8601(Duration::from_mins(10)), "PT10M");
        assert_eq!(format_iso8601(Duration::from_secs(16)), "PT16S");
        assert_eq!(format_iso8601(Duration::from_secs(14 * 86_400)), "P14D");
        assert_eq!(format_iso8601(Duration::from_secs(0)), "PT0S");
        assert_eq!(format_iso8601(Duration::from_secs(90_061)), "P1DT1H1M1S");
    }

    #[test]
    fn unlimited_round_trips_verbatim() {
        let parsed = parse_iso8601(TIMESPAN_MAX).unwrap();
        assert!(is_unlimited(parsed));
        assert_eq!(format_iso8601(parsed), TIMESPAN_MAX);
    }

    #[allow(clippy::duration_suboptimal_units)]
    #[test]
    fn parses_what_it_formats() {
        for d in [
            Duration::from_mins(1),
            Duration::from_secs(16),
            Duration::from_secs(14 * 86_400),
            Duration::from_secs(90_061),
            Duration::new(5, 477_580_700),
        ] {
            assert_eq!(parse_iso8601(&format_iso8601(d)).unwrap(), d, "{d:?}");
        }
    }

    #[test]
    fn rejects_year_month_durations() {
        assert!(parse_iso8601("P1Y").is_none());
        assert!(parse_iso8601("P1M").is_none()); // month in date part
        assert!(parse_iso8601("PT1M").is_some()); // minute in time part
    }
}
