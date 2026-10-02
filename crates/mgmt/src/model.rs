//! Entity models for the management API. Naming follows .NET's
//! `ServiceBusAdministrationClient` (`XxxProperties` for user-settable fields,
//! `XxxRuntimeInfo` for server counters, `XxxInfo` for both).

use std::fmt;
use std::time::Duration;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

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

/// XML Schema primitive types supported by correlation property values.
/// Values retain their original XML text so edits and rollback do not change
/// numeric precision, boolean spelling, or timestamp offsets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CorrelationPropertyType {
    #[default]
    String,
    Boolean,
    Byte,
    UnsignedByte,
    Short,
    UnsignedShort,
    Int,
    UnsignedInt,
    Long,
    UnsignedLong,
    Decimal,
    Float,
    Double,
    DateTime,
    Base64Binary,
    Duration,
    #[serde(rename = "anyURI")]
    AnyUri,
}

impl CorrelationPropertyType {
    pub const ALL: [Self; 17] = [
        Self::String,
        Self::Boolean,
        Self::Byte,
        Self::UnsignedByte,
        Self::Short,
        Self::UnsignedShort,
        Self::Int,
        Self::UnsignedInt,
        Self::Long,
        Self::UnsignedLong,
        Self::Decimal,
        Self::Float,
        Self::Double,
        Self::DateTime,
        Self::Base64Binary,
        Self::Duration,
        Self::AnyUri,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Boolean => "boolean",
            Self::Byte => "byte",
            Self::UnsignedByte => "unsignedByte",
            Self::Short => "short",
            Self::UnsignedShort => "unsignedShort",
            Self::Int => "int",
            Self::UnsignedInt => "unsignedInt",
            Self::Long => "long",
            Self::UnsignedLong => "unsignedLong",
            Self::Decimal => "decimal",
            Self::Float => "float",
            Self::Double => "double",
            Self::DateTime => "dateTime",
            Self::Base64Binary => "base64Binary",
            Self::Duration => "duration",
            Self::AnyUri => "anyURI",
        }
    }

    #[must_use]
    pub fn from_xml_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// Validate supported XML Schema lexical forms without normalizing the
    /// stored value. Precision and original spelling remain unchanged.
    #[must_use]
    pub fn accepts(self, value: &str) -> bool {
        use base64::Engine as _;
        let lexical = value.trim();
        match self {
            Self::String => true,
            Self::Boolean => matches!(lexical, "true" | "false" | "1" | "0"),
            Self::Byte => lexical.parse::<i8>().is_ok(),
            Self::UnsignedByte => lexical.parse::<u8>().is_ok(),
            Self::Short => lexical.parse::<i16>().is_ok(),
            Self::UnsignedShort => lexical.parse::<u16>().is_ok(),
            Self::Int => lexical.parse::<i32>().is_ok(),
            Self::UnsignedInt => lexical.parse::<u32>().is_ok(),
            Self::Long => lexical.parse::<i64>().is_ok(),
            Self::UnsignedLong => lexical.parse::<u64>().is_ok(),
            Self::Decimal => {
                let number = lexical.strip_prefix(['+', '-']).unwrap_or(lexical);
                !number.is_empty()
                    && number.bytes().any(|b| b.is_ascii_digit())
                    && number.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                    && number.bytes().filter(|b| *b == b'.').count() <= 1
            }
            Self::Float | Self::Double => {
                matches!(lexical, "INF" | "-INF" | "NaN")
                    || (lexical.bytes().any(|b| b.is_ascii_digit())
                        && lexical.bytes().all(|b| {
                            b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E')
                        })
                        && lexical.parse::<f64>().is_ok())
            }
            Self::DateTime => {
                let utc = format!("{lexical}Z");
                OffsetDateTime::parse(lexical, &Rfc3339).is_ok()
                    || OffsetDateTime::parse(&utc, &Rfc3339).is_ok()
            }
            Self::Base64Binary => {
                let compact: String = value.chars().filter(|c| !c.is_ascii_whitespace()).collect();
                base64::engine::general_purpose::STANDARD
                    .decode(compact)
                    .is_ok()
            }
            Self::Duration => valid_xml_duration(lexical),
            Self::AnyUri => !value.chars().any(char::is_control),
        }
    }
}

fn valid_xml_duration(value: &str) -> bool {
    let value = value.strip_prefix('-').unwrap_or(value);
    let Some(value) = value.strip_prefix('P') else {
        return false;
    };
    let (date, time) = value
        .split_once('T')
        .map_or((value, None), |(date, time)| (date, Some(time)));
    let components = |part: &str, units: &str| {
        let mut previous = None;
        let mut number = String::new();
        for ch in part.chars() {
            if ch.is_ascii_digit() || ch == '.' {
                number.push(ch);
                continue;
            }
            let Some(position) = units.find(ch) else {
                return false;
            };
            if previous.is_some_and(|p| p >= position)
                || !number.bytes().any(|b| b.is_ascii_digit())
                || number.starts_with('.')
                || number.ends_with('.')
                || number.bytes().filter(|b| *b == b'.').count() > usize::from(ch == 'S')
            {
                return false;
            }
            previous = Some(position);
            number.clear();
        }
        number.is_empty()
    };
    !value.is_empty()
        && components(date, "YMD")
        && time.is_none_or(|part| !part.is_empty() && components(part, "HMS"))
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[allow(clippy::large_enum_variant)] // correlation fields mirror the export schema and edit form
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
        /// Missing entries represent strings, preserving earlier JSON exports.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        property_types: std::collections::BTreeMap<String, CorrelationPropertyType>,
    },
    True,
    False,
}

impl RuleFilter {
    /// Refuse ambiguous or unsupported typed correlation values before a
    /// destructive replacement or namespace import begins.
    pub fn validate(&self) -> Result<(), String> {
        let Self::Correlation {
            properties,
            property_types,
            ..
        } = self
        else {
            return Ok(());
        };
        let mut names = std::collections::BTreeSet::new();
        for (name, value) in properties {
            if name.is_empty() || !names.insert(name.as_str()) {
                return Err(format!(
                    "correlation property '{name}' has an empty or duplicate name"
                ));
            }
            let kind = property_types.get(name).copied().unwrap_or_default();
            if !kind.accepts(value) {
                return Err(format!(
                    "correlation property '{name}' has an invalid or unsupported {} value",
                    kind.as_str()
                ));
            }
        }
        for name in property_types.keys() {
            if !names.contains(name.as_str()) {
                return Err(format!(
                    "correlation property type metadata references missing property '{name}'"
                ));
            }
        }
        Ok(())
    }

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

    #[test]
    fn typed_correlation_values_validate_without_numeric_loss() {
        use CorrelationPropertyType as Kind;
        assert!(Kind::Decimal.accepts("12345678901234567890.123456789"));
        assert!(Kind::Boolean.accepts("0"));
        assert!(Kind::DateTime.accepts("2026-10-02T12:34:56"));
        assert!(Kind::Duration.accepts("-P1Y2M3DT4H5M6.123S"));
        for (kind, value) in [
            (Kind::Boolean, "yes"),
            (Kind::Byte, "128"),
            (Kind::Int, "2147483648"),
            (Kind::UnsignedLong, "-1"),
            (Kind::Decimal, "1e2"),
            (Kind::Double, "infinity"),
            (Kind::DateTime, "yesterday"),
            (Kind::Base64Binary, "%%%=="),
            (Kind::Duration, "P"),
            (Kind::Duration, "P1M2Y"),
            (Kind::Duration, "PT1.5H"),
            (Kind::Duration, "PT"),
        ] {
            assert!(!kind.accepts(value), "{}: {value}", kind.as_str());
        }
    }
}
