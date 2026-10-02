//! Serialization of entity descriptions into the ATOM `<entry>` bodies the
//! management API expects on PUT.
//!
//! The service validates element order against its XSD — a misplaced element
//! yields HTTP 400 — so fields are emitted by hand in schema order instead of
//! deriving serde serializers.

use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesStart, BytesText, Event};

use crate::model::{
    QueueProperties, RuleFilter, RuleProperties, SubscriptionProperties, TopicProperties,
    format_iso8601,
};

const SB_NS: &str = "http://schemas.microsoft.com/netservices/2010/10/servicebus/connect";
const XSI_NS: &str = "http://www.w3.org/2001/XMLSchema-instance";
const ATOM_NS: &str = "http://www.w3.org/2005/Atom";

struct Xml {
    writer: Writer<Vec<u8>>,
}

impl Xml {
    fn new() -> Self {
        let mut writer = Writer::new(Vec::new());
        let _ = writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)));
        Self { writer }
    }

    fn open(&mut self, tag: &str, attrs: &[(&str, &str)]) {
        let mut start = BytesStart::new(tag);
        for (key, value) in attrs {
            start.push_attribute((*key, *value));
        }
        let _ = self.writer.write_event(Event::Start(start));
    }

    fn close(&mut self, tag: &str) {
        let _ = self
            .writer
            .write_event(Event::End(BytesStart::new(tag).to_end()));
    }

    fn leaf(&mut self, tag: &str, value: &str) {
        self.open(tag, &[]);
        let _ = self.writer.write_event(Event::Text(BytesText::new(value)));
        self.close(tag);
    }

    fn leaf_bool(&mut self, tag: &str, value: bool) {
        self.leaf(tag, if value { "true" } else { "false" });
    }

    fn leaf_opt(&mut self, tag: &str, value: Option<&str>) {
        if let Some(value) = value
            && !value.is_empty()
        {
            self.leaf(tag, value);
        }
    }

    fn finish(self) -> String {
        String::from_utf8(self.writer.into_inner()).expect("the XML writer only produces UTF-8")
    }
}

/// Wrap a description in the ATOM `<entry>` envelope the API expects.
fn entry(description_tag: &str, write_fields: impl FnOnce(&mut Xml)) -> String {
    let mut xml = Xml::new();
    xml.open("entry", &[("xmlns", ATOM_NS)]);
    xml.open("content", &[("type", "application/xml")]);
    xml.open(description_tag, &[("xmlns", SB_NS), ("xmlns:i", XSI_NS)]);
    write_fields(&mut xml);
    xml.close(description_tag);
    xml.close("content");
    xml.close("entry");
    xml.finish()
}

/// `QueueDescription` in XSD order.
#[must_use]
pub(crate) fn queue_body(p: &QueueProperties) -> String {
    entry("QueueDescription", |xml| {
        xml.leaf("LockDuration", &format_iso8601(p.lock_duration));
        xml.leaf("MaxSizeInMegabytes", &p.max_size_in_megabytes.to_string());
        xml.leaf_bool("RequiresDuplicateDetection", p.requires_duplicate_detection);
        xml.leaf_bool("RequiresSession", p.requires_session);
        xml.leaf(
            "DefaultMessageTimeToLive",
            &format_iso8601(p.default_message_time_to_live),
        );
        xml.leaf_bool(
            "DeadLetteringOnMessageExpiration",
            p.dead_lettering_on_message_expiration,
        );
        xml.leaf(
            "DuplicateDetectionHistoryTimeWindow",
            &format_iso8601(p.duplicate_detection_history_time_window),
        );
        xml.leaf("MaxDeliveryCount", &p.max_delivery_count.to_string());
        xml.leaf_bool("EnableBatchedOperations", p.enable_batched_operations);
        xml.leaf("Status", p.status.as_str());
        xml.leaf_opt("ForwardTo", p.forward_to.as_deref());
        xml.leaf("AutoDeleteOnIdle", &format_iso8601(p.auto_delete_on_idle));
        xml.leaf_bool("EnablePartitioning", p.enable_partitioning);
        xml.leaf_bool("EnableExpress", p.enable_express);
        xml.leaf_opt("UserMetadata", p.user_metadata.as_deref());
        xml.leaf_opt(
            "ForwardDeadLetteredMessagesTo",
            p.forward_dead_lettered_messages_to.as_deref(),
        );
        if let Some(kb) = p.max_message_size_in_kilobytes {
            xml.leaf("MaxMessageSizeInKilobytes", &kb.to_string());
        }
    })
}

/// `TopicDescription` in XSD order.
#[must_use]
pub(crate) fn topic_body(p: &TopicProperties) -> String {
    entry("TopicDescription", |xml| {
        xml.leaf(
            "DefaultMessageTimeToLive",
            &format_iso8601(p.default_message_time_to_live),
        );
        xml.leaf("MaxSizeInMegabytes", &p.max_size_in_megabytes.to_string());
        xml.leaf_bool("RequiresDuplicateDetection", p.requires_duplicate_detection);
        xml.leaf(
            "DuplicateDetectionHistoryTimeWindow",
            &format_iso8601(p.duplicate_detection_history_time_window),
        );
        xml.leaf_bool("EnableBatchedOperations", p.enable_batched_operations);
        xml.leaf("Status", p.status.as_str());
        xml.leaf_bool("SupportOrdering", p.support_ordering);
        xml.leaf("AutoDeleteOnIdle", &format_iso8601(p.auto_delete_on_idle));
        xml.leaf_bool("EnablePartitioning", p.enable_partitioning);
        xml.leaf_bool("EnableExpress", p.enable_express);
        xml.leaf_opt("UserMetadata", p.user_metadata.as_deref());
        if let Some(kb) = p.max_message_size_in_kilobytes {
            xml.leaf("MaxMessageSizeInKilobytes", &kb.to_string());
        }
    })
}

/// `SubscriptionDescription` in XSD order.
#[must_use]
pub(crate) fn subscription_body(p: &SubscriptionProperties) -> String {
    entry("SubscriptionDescription", |xml| {
        xml.leaf("LockDuration", &format_iso8601(p.lock_duration));
        xml.leaf_bool("RequiresSession", p.requires_session);
        xml.leaf(
            "DefaultMessageTimeToLive",
            &format_iso8601(p.default_message_time_to_live),
        );
        xml.leaf_bool(
            "DeadLetteringOnMessageExpiration",
            p.dead_lettering_on_message_expiration,
        );
        xml.leaf_bool(
            "DeadLetteringOnFilterEvaluationExceptions",
            p.dead_lettering_on_filter_evaluation_exceptions,
        );
        xml.leaf("MaxDeliveryCount", &p.max_delivery_count.to_string());
        xml.leaf_bool("EnableBatchedOperations", p.enable_batched_operations);
        xml.leaf("Status", p.status.as_str());
        xml.leaf_opt("ForwardTo", p.forward_to.as_deref());
        xml.leaf_opt("UserMetadata", p.user_metadata.as_deref());
        xml.leaf("AutoDeleteOnIdle", &format_iso8601(p.auto_delete_on_idle));
        xml.leaf_opt(
            "ForwardDeadLetteredMessagesTo",
            p.forward_dead_lettered_messages_to.as_deref(),
        );
    })
}

/// `RuleDescription`: Filter, Action, Name — in that order.
#[must_use]
pub(crate) fn rule_body(p: &RuleProperties) -> String {
    entry("RuleDescription", |xml| {
        match &p.filter {
            RuleFilter::Sql { expression } => {
                xml.open("Filter", &[("i:type", "SqlFilter")]);
                xml.leaf("SqlExpression", expression);
                xml.close("Filter");
            }
            RuleFilter::Correlation {
                correlation_id,
                message_id,
                to,
                reply_to,
                subject,
                session_id,
                reply_to_session_id,
                content_type,
                properties,
                property_types,
            } => {
                xml.open("Filter", &[("i:type", "CorrelationFilter")]);
                xml.leaf_opt("CorrelationId", correlation_id.as_deref());
                xml.leaf_opt("MessageId", message_id.as_deref());
                xml.leaf_opt("To", to.as_deref());
                xml.leaf_opt("ReplyTo", reply_to.as_deref());
                xml.leaf_opt("Label", subject.as_deref());
                xml.leaf_opt("SessionId", session_id.as_deref());
                xml.leaf_opt("ReplyToSessionId", reply_to_session_id.as_deref());
                xml.leaf_opt("ContentType", content_type.as_deref());
                if !properties.is_empty() {
                    xml.open("Properties", &[]);
                    for (key, value) in properties {
                        xml.open("KeyValueOfstringanyType", &[]);
                        xml.leaf("Key", key);
                        let schema_type = format!(
                            "d6p1:{}",
                            property_types
                                .get(key)
                                .copied()
                                .unwrap_or_default()
                                .as_str()
                        );
                        xml.open(
                            "Value",
                            &[
                                ("i:type", &schema_type),
                                ("xmlns:d6p1", "http://www.w3.org/2001/XMLSchema"),
                            ],
                        );
                        let _ = xml.writer.write_event(Event::Text(BytesText::new(value)));
                        xml.close("Value");
                        xml.close("KeyValueOfstringanyType");
                    }
                    xml.close("Properties");
                }
                xml.close("Filter");
            }
            RuleFilter::True => {
                xml.open("Filter", &[("i:type", "TrueFilter")]);
                xml.leaf("SqlExpression", "1=1");
                xml.close("Filter");
            }
            RuleFilter::False => {
                xml.open("Filter", &[("i:type", "FalseFilter")]);
                xml.leaf("SqlExpression", "1=0");
                xml.close("Filter");
            }
        }
        match &p.action {
            Some(expression) if !expression.trim().is_empty() => {
                xml.open("Action", &[("i:type", "SqlRuleAction")]);
                xml.leaf("SqlExpression", expression);
                xml.close("Action");
            }
            _ => {
                xml.open("Action", &[("i:type", "EmptyRuleAction")]);
                xml.close("Action");
            }
        }
        xml.leaf("Name", &p.name);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom;
    use crate::model::EntityStatus;

    #[test]
    fn queue_body_round_trips_through_the_parser() {
        let mut props = QueueProperties {
            name: "orders".into(),
            requires_session: true,
            dead_lettering_on_message_expiration: true,
            forward_dead_lettered_messages_to: Some("orders-dlq-archive".into()),
            status: EntityStatus::SendDisabled,
            ..QueueProperties::default()
        };
        let body = queue_body(&props);
        assert!(body.starts_with(r#"<?xml version="1.0" encoding="utf-8"?>"#));

        // The parser reads the name from the ATOM title, which PUT bodies
        // don't carry — inject it for comparison.
        let parsed = atom::parse_queue(&with_title(&body, "orders"))
            .unwrap()
            .unwrap();
        props.name = "orders".into();
        assert_eq!(parsed.properties, props);
    }

    #[test]
    fn field_order_matches_the_xsd() {
        let body = queue_body(&QueueProperties::default());
        let positions: Vec<usize> = [
            "<LockDuration>",
            "<MaxSizeInMegabytes>",
            "<RequiresDuplicateDetection>",
            "<RequiresSession>",
            "<DefaultMessageTimeToLive>",
            "<DeadLetteringOnMessageExpiration>",
            "<DuplicateDetectionHistoryTimeWindow>",
            "<MaxDeliveryCount>",
            "<EnableBatchedOperations>",
            "<Status>",
            "<AutoDeleteOnIdle>",
            "<EnablePartitioning>",
            "<EnableExpress>",
        ]
        .iter()
        .map(|tag| body.find(tag).unwrap_or_else(|| panic!("missing {tag}")))
        .collect();
        assert!(positions.is_sorted(), "queue fields out of XSD order");
    }

    #[test]
    fn sql_rule_round_trips() {
        let props = RuleProperties {
            topic: "events".into(),
            subscription: "audit".into(),
            name: "high-priority".into(),
            filter: RuleFilter::Sql {
                expression: "priority > 3".into(),
            },
            action: Some("SET processed = 'true'".into()),
        };
        let parsed = atom::parse_rule(&rule_body(&props), "events", "audit")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.properties, props);
    }

    #[test]
    fn every_supported_correlation_type_round_trips_without_normalizing_values() {
        use crate::model::CorrelationPropertyType as Kind;
        let cases = [
            (Kind::String, "  literal <value> & text  "),
            (Kind::Boolean, "1"),
            (Kind::Byte, "-128"),
            (Kind::UnsignedByte, "255"),
            (Kind::Short, "-32768"),
            (Kind::UnsignedShort, "65535"),
            (Kind::Int, "-2147483648"),
            (Kind::UnsignedInt, "4294967295"),
            (Kind::Long, "-9223372036854775808"),
            (Kind::UnsignedLong, "18446744073709551615"),
            (Kind::Decimal, "12345678901234567890.123456789"),
            (Kind::Float, "1.23E-5"),
            (Kind::Double, "-INF"),
            (Kind::DateTime, "2026-10-02T12:34:56.1234567-07:00"),
            (Kind::Base64Binary, "AAEC/w=="),
            (Kind::Duration, "-P1Y2M3DT4H5M6.123S"),
            (Kind::AnyUri, "relative/path?name=value"),
        ];
        let properties: Vec<_> = cases
            .iter()
            .map(|(kind, value)| (kind.as_str().to_owned(), (*value).to_owned()))
            .collect();
        let property_types = cases
            .iter()
            .filter(|(kind, _)| *kind != Kind::String)
            .map(|(kind, _)| (kind.as_str().to_owned(), *kind))
            .collect();
        let original = RuleProperties {
            topic: "events".into(),
            subscription: "audit".into(),
            name: "typed-routing".into(),
            filter: RuleFilter::Correlation {
                correlation_id: None,
                message_id: None,
                to: None,
                reply_to: None,
                subject: Some("order".into()),
                session_id: None,
                reply_to_session_id: None,
                content_type: None,
                properties,
                property_types,
            },
            action: Some("SET origin = 'original'".into()),
        };
        original.filter.validate().unwrap();
        let parsed = atom::parse_rule(&rule_body(&original), "events", "audit")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.properties, original);
        let mut edited = parsed.properties;
        edited.action = Some("SET origin = 'edited'".into());
        let rewritten = atom::parse_rule(&rule_body(&edited), "events", "audit")
            .unwrap()
            .unwrap();
        assert_eq!(rewritten.properties.filter, original.filter);
        // A replacement failure restores the same original type metadata.
        let restored = atom::parse_rule(&rule_body(&original), "events", "audit")
            .unwrap()
            .unwrap();
        assert_eq!(restored.properties, original);
    }

    #[test]
    fn subscription_body_round_trips() {
        let mut props = SubscriptionProperties {
            topic: "events".into(),
            name: "audit".into(),
            requires_session: true,
            max_delivery_count: 3,
            ..SubscriptionProperties::default()
        };
        let body = subscription_body(&props);
        let parsed = atom::parse_subscription(&with_title(&body, "audit"), "events")
            .unwrap()
            .unwrap();
        props.name = "audit".into();
        assert_eq!(parsed.properties, props);
    }

    #[test]
    fn topic_body_round_trips() {
        let props = TopicProperties {
            name: "events".into(),
            support_ordering: true,
            ..TopicProperties::default()
        };
        let parsed = atom::parse_topic(&with_title(&topic_body(&props), "events"))
            .unwrap()
            .unwrap();
        assert_eq!(parsed.properties, props);
    }

    /// PUT bodies have no `<title>`; splice one in so parse tests can verify
    /// the round trip including the name.
    fn with_title(body: &str, title: &str) -> String {
        body.replace(
            "<content type=\"application/xml\">",
            &format!("<title type=\"text\">{title}</title><content type=\"application/xml\">"),
        )
    }
}
