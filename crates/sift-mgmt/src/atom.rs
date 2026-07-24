//! Parsing of the ATOM envelopes returned by the management API.
//!
//! The service emits XML namespaces inconsistently (`d2p1:` prefixes, default
//! namespaces, `i:nil` markers), so all lookups match on *local* element names
//! against a read-only DOM.

use roxmltree::{Document, Node};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::MgmtError;
use crate::model::{
    EntityRuntimeInfo, EntityStatus, MessageCountDetails, NamespaceInfo, QueueInfo,
    QueueProperties, RuleFilter, RuleInfo, RuleProperties, SubscriptionInfo,
    SubscriptionProperties, TopicInfo, TopicProperties, parse_iso8601,
};

const XSI_NS: &str = "http://www.w3.org/2001/XMLSchema-instance";

fn parse_doc(xml: &str) -> Result<Document<'_>, MgmtError> {
    Document::parse(xml).map_err(|e| MgmtError::Xml(e.to_string()))
}

/// Find the description node (e.g. `QueueDescription`) inside one `<entry>`,
/// together with the entry's `<title>` (the entity name).
fn entry_description<'a, 'input>(
    entry: Node<'a, 'input>,
    description_tag: &str,
) -> Option<(String, Node<'a, 'input>)> {
    let title = entry
        .descendants()
        .find(|n| n.has_tag_name("title"))
        .and_then(|n| n.text())
        .unwrap_or_default()
        .to_owned();
    let description = entry
        .descendants()
        .find(|n| n.tag_name().name() == description_tag)?;
    Some((title, description))
}

/// Iterate the `<entry>` elements of a `<feed>`; a single `<entry>` document
/// yields itself, so entity GET and LIST responses share one code path.
fn entries<'a, 'input>(doc: &'a Document<'input>) -> Vec<Node<'a, 'input>> {
    let root = doc.root_element();
    if root.tag_name().name() == "entry" {
        vec![root]
    } else {
        root.children()
            .filter(|n| n.tag_name().name() == "entry")
            .collect()
    }
}

// ---- field readers --------------------------------------------------------

fn is_nil(node: Node<'_, '_>) -> bool {
    node.attribute((XSI_NS, "nil")) == Some("true")
}

fn child_text<'a>(parent: Node<'a, '_>, name: &str) -> Option<&'a str> {
    parent
        .children()
        .find(|n| n.tag_name().name() == name && !is_nil(*n))
        .and_then(|n| n.text())
}

fn text(parent: Node<'_, '_>, name: &str) -> Option<String> {
    child_text(parent, name).map(str::to_owned)
}

fn boolean(parent: Node<'_, '_>, name: &str) -> Option<bool> {
    child_text(parent, name).map(|t| t.eq_ignore_ascii_case("true"))
}

fn integer<T: std::str::FromStr>(parent: Node<'_, '_>, name: &str) -> Option<T> {
    child_text(parent, name).and_then(|t| t.parse().ok())
}

fn duration(parent: Node<'_, '_>, name: &str) -> Option<std::time::Duration> {
    child_text(parent, name).and_then(parse_iso8601)
}

fn timestamp(parent: Node<'_, '_>, name: &str) -> Option<OffsetDateTime> {
    child_text(parent, name).and_then(|t| OffsetDateTime::parse(t, &Rfc3339).ok())
}

fn status(parent: Node<'_, '_>) -> EntityStatus {
    child_text(parent, "Status").map_or(EntityStatus::Active, EntityStatus::parse)
}

fn count_details(parent: Node<'_, '_>) -> MessageCountDetails {
    let Some(details) = parent
        .children()
        .find(|n| n.tag_name().name() == "CountDetails")
    else {
        return MessageCountDetails::default();
    };
    MessageCountDetails {
        active: integer(details, "ActiveMessageCount").unwrap_or(0),
        dead_letter: integer(details, "DeadLetterMessageCount").unwrap_or(0),
        scheduled: integer(details, "ScheduledMessageCount").unwrap_or(0),
        transfer: integer(details, "TransferMessageCount").unwrap_or(0),
        transfer_dead_letter: integer(details, "TransferDeadLetterMessageCount").unwrap_or(0),
    }
}

fn runtime_info(node: Node<'_, '_>) -> EntityRuntimeInfo {
    EntityRuntimeInfo {
        message_count: integer(node, "MessageCount").unwrap_or(0),
        size_in_bytes: integer(node, "SizeInBytes").unwrap_or(0),
        count_details: count_details(node),
        created_at: timestamp(node, "CreatedAt"),
        updated_at: timestamp(node, "UpdatedAt"),
        accessed_at: timestamp(node, "AccessedAt"),
    }
}

// ---- queues ----------------------------------------------------------------

fn queue_from_entry(entry: Node<'_, '_>) -> Option<QueueInfo> {
    let (name, node) = entry_description(entry, "QueueDescription")?;
    let defaults = QueueProperties::default();
    let properties = QueueProperties {
        name,
        lock_duration: duration(node, "LockDuration").unwrap_or(defaults.lock_duration),
        max_size_in_megabytes: integer(node, "MaxSizeInMegabytes")
            .unwrap_or(defaults.max_size_in_megabytes),
        requires_duplicate_detection: boolean(node, "RequiresDuplicateDetection")
            .unwrap_or_default(),
        requires_session: boolean(node, "RequiresSession").unwrap_or_default(),
        default_message_time_to_live: duration(node, "DefaultMessageTimeToLive")
            .unwrap_or(defaults.default_message_time_to_live),
        dead_lettering_on_message_expiration: boolean(node, "DeadLetteringOnMessageExpiration")
            .unwrap_or_default(),
        duplicate_detection_history_time_window: duration(
            node,
            "DuplicateDetectionHistoryTimeWindow",
        )
        .unwrap_or(defaults.duplicate_detection_history_time_window),
        max_delivery_count: integer(node, "MaxDeliveryCount")
            .unwrap_or(defaults.max_delivery_count),
        enable_batched_operations: boolean(node, "EnableBatchedOperations").unwrap_or(true),
        status: status(node),
        forward_to: text(node, "ForwardTo"),
        user_metadata: text(node, "UserMetadata"),
        auto_delete_on_idle: duration(node, "AutoDeleteOnIdle")
            .unwrap_or(defaults.auto_delete_on_idle),
        enable_partitioning: boolean(node, "EnablePartitioning").unwrap_or_default(),
        enable_express: boolean(node, "EnableExpress").unwrap_or_default(),
        forward_dead_lettered_messages_to: text(node, "ForwardDeadLetteredMessagesTo"),
        max_message_size_in_kilobytes: integer(node, "MaxMessageSizeInKilobytes"),
    };
    Some(QueueInfo {
        runtime: runtime_info(node),
        properties,
    })
}

/// `Ok(None)` when the response is valid ATOM but contains no description —
/// which is how the service answers a GET for a non-existent entity (it
/// returns 200 with an empty feed, not 404).
pub(crate) fn parse_queue(xml: &str) -> Result<Option<QueueInfo>, MgmtError> {
    let doc = parse_doc(xml)?;
    Ok(entries(&doc).into_iter().find_map(queue_from_entry))
}

pub(crate) fn parse_queue_feed(xml: &str) -> Result<Vec<QueueInfo>, MgmtError> {
    let doc = parse_doc(xml)?;
    Ok(entries(&doc)
        .into_iter()
        .filter_map(queue_from_entry)
        .collect())
}

// ---- topics ----------------------------------------------------------------

fn topic_from_entry(entry: Node<'_, '_>) -> Option<TopicInfo> {
    let (name, node) = entry_description(entry, "TopicDescription")?;
    let defaults = TopicProperties::default();
    let properties = TopicProperties {
        name,
        default_message_time_to_live: duration(node, "DefaultMessageTimeToLive")
            .unwrap_or(defaults.default_message_time_to_live),
        max_size_in_megabytes: integer(node, "MaxSizeInMegabytes")
            .unwrap_or(defaults.max_size_in_megabytes),
        requires_duplicate_detection: boolean(node, "RequiresDuplicateDetection")
            .unwrap_or_default(),
        duplicate_detection_history_time_window: duration(
            node,
            "DuplicateDetectionHistoryTimeWindow",
        )
        .unwrap_or(defaults.duplicate_detection_history_time_window),
        enable_batched_operations: boolean(node, "EnableBatchedOperations").unwrap_or(true),
        status: status(node),
        support_ordering: boolean(node, "SupportOrdering").unwrap_or_default(),
        auto_delete_on_idle: duration(node, "AutoDeleteOnIdle")
            .unwrap_or(defaults.auto_delete_on_idle),
        enable_partitioning: boolean(node, "EnablePartitioning").unwrap_or_default(),
        enable_express: boolean(node, "EnableExpress").unwrap_or_default(),
        user_metadata: text(node, "UserMetadata"),
        max_message_size_in_kilobytes: integer(node, "MaxMessageSizeInKilobytes"),
    };
    Some(TopicInfo {
        properties,
        subscription_count: integer(node, "SubscriptionCount").unwrap_or(0),
        size_in_bytes: integer(node, "SizeInBytes").unwrap_or(0),
        scheduled_message_count: count_details(node).scheduled,
        created_at: timestamp(node, "CreatedAt"),
        updated_at: timestamp(node, "UpdatedAt"),
        accessed_at: timestamp(node, "AccessedAt"),
    })
}

pub(crate) fn parse_topic(xml: &str) -> Result<Option<TopicInfo>, MgmtError> {
    let doc = parse_doc(xml)?;
    Ok(entries(&doc).into_iter().find_map(topic_from_entry))
}

pub(crate) fn parse_topic_feed(xml: &str) -> Result<Vec<TopicInfo>, MgmtError> {
    let doc = parse_doc(xml)?;
    Ok(entries(&doc)
        .into_iter()
        .filter_map(topic_from_entry)
        .collect())
}

// ---- subscriptions ---------------------------------------------------------

fn subscription_from_entry(entry: Node<'_, '_>, topic: &str) -> Option<SubscriptionInfo> {
    let (name, node) = entry_description(entry, "SubscriptionDescription")?;
    let defaults = SubscriptionProperties::default();
    let properties = SubscriptionProperties {
        topic: topic.to_owned(),
        name,
        lock_duration: duration(node, "LockDuration").unwrap_or(defaults.lock_duration),
        requires_session: boolean(node, "RequiresSession").unwrap_or_default(),
        default_message_time_to_live: duration(node, "DefaultMessageTimeToLive")
            .unwrap_or(defaults.default_message_time_to_live),
        dead_lettering_on_message_expiration: boolean(node, "DeadLetteringOnMessageExpiration")
            .unwrap_or_default(),
        dead_lettering_on_filter_evaluation_exceptions: boolean(
            node,
            "DeadLetteringOnFilterEvaluationExceptions",
        )
        .unwrap_or(true),
        max_delivery_count: integer(node, "MaxDeliveryCount")
            .unwrap_or(defaults.max_delivery_count),
        enable_batched_operations: boolean(node, "EnableBatchedOperations").unwrap_or(true),
        status: status(node),
        forward_to: text(node, "ForwardTo"),
        user_metadata: text(node, "UserMetadata"),
        auto_delete_on_idle: duration(node, "AutoDeleteOnIdle")
            .unwrap_or(defaults.auto_delete_on_idle),
        forward_dead_lettered_messages_to: text(node, "ForwardDeadLetteredMessagesTo"),
    };
    Some(SubscriptionInfo {
        runtime: runtime_info(node),
        properties,
    })
}

pub(crate) fn parse_subscription(
    xml: &str,
    topic: &str,
) -> Result<Option<SubscriptionInfo>, MgmtError> {
    let doc = parse_doc(xml)?;
    Ok(entries(&doc)
        .into_iter()
        .find_map(|e| subscription_from_entry(e, topic)))
}

pub(crate) fn parse_subscription_feed(
    xml: &str,
    topic: &str,
) -> Result<Vec<SubscriptionInfo>, MgmtError> {
    let doc = parse_doc(xml)?;
    Ok(entries(&doc)
        .into_iter()
        .filter_map(|e| subscription_from_entry(e, topic))
        .collect())
}

// ---- rules ------------------------------------------------------------------

fn rule_from_entry(entry: Node<'_, '_>, topic: &str, subscription: &str) -> Option<RuleInfo> {
    let (name, node) = entry_description(entry, "RuleDescription")?;

    let filter_node = node.children().find(|n| n.tag_name().name() == "Filter");
    let filter = filter_node.map_or(RuleFilter::True, |f| {
        match f.attribute((XSI_NS, "type")).unwrap_or_default() {
            "SqlFilter" => RuleFilter::Sql {
                expression: text(f, "SqlExpression").unwrap_or_default(),
            },
            "CorrelationFilter" => RuleFilter::Correlation {
                correlation_id: text(f, "CorrelationId"),
                message_id: text(f, "MessageId"),
                to: text(f, "To"),
                reply_to: text(f, "ReplyTo"),
                subject: text(f, "Label"),
                session_id: text(f, "SessionId"),
                reply_to_session_id: text(f, "ReplyToSessionId"),
                content_type: text(f, "ContentType"),
                properties: f
                    .descendants()
                    .filter(|n| n.tag_name().name() == "KeyValueOfstringanyType")
                    .filter_map(|kv| {
                        Some((text(kv, "Key")?, text(kv, "Value").unwrap_or_default()))
                    })
                    .collect(),
            },
            "FalseFilter" => RuleFilter::False,
            _ => RuleFilter::True,
        }
    });

    let action = node
        .children()
        .find(|n| n.tag_name().name() == "Action")
        .filter(|a| a.attribute((XSI_NS, "type")) == Some("SqlRuleAction"))
        .and_then(|a| text(a, "SqlExpression"));

    Some(RuleInfo {
        properties: RuleProperties {
            topic: topic.to_owned(),
            subscription: subscription.to_owned(),
            // Rule entries carry the name both as <title> and <Name>.
            name: text(node, "Name").unwrap_or(name),
            filter,
            action,
        },
        created_at: timestamp(node, "CreatedAt"),
    })
}

pub(crate) fn parse_rule_feed(
    xml: &str,
    topic: &str,
    subscription: &str,
) -> Result<Vec<RuleInfo>, MgmtError> {
    let doc = parse_doc(xml)?;
    Ok(entries(&doc)
        .into_iter()
        .filter_map(|e| rule_from_entry(e, topic, subscription))
        .collect())
}

pub(crate) fn parse_rule(
    xml: &str,
    topic: &str,
    subscription: &str,
) -> Result<Option<RuleInfo>, MgmtError> {
    Ok(parse_rule_feed(xml, topic, subscription)?
        .into_iter()
        .next())
}

// ---- namespace ---------------------------------------------------------------

pub(crate) fn parse_namespace_info(xml: &str) -> Result<NamespaceInfo, MgmtError> {
    let doc = parse_doc(xml)?;
    let node = doc
        .descendants()
        .find(|n| n.tag_name().name() == "NamespaceInfo")
        .ok_or_else(|| MgmtError::Xml("response did not contain a NamespaceInfo".into()))?;

    let name = text(node, "Name")
        .ok_or_else(|| MgmtError::Xml("NamespaceInfo has no Name element".into()))?;
    Ok(NamespaceInfo {
        name,
        alias: text(node, "Alias"),
        namespace_type: text(node, "NamespaceType"),
        messaging_sku: text(node, "MessagingSKU"),
        messaging_units: integer(node, "MessagingUnits"),
        created_time: timestamp(node, "CreatedTime"),
        modified_time: timestamp(node, "ModifiedTime"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured shape of a real `$namespaceinfo` response.
    const NAMESPACE_INFO: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<entry xmlns="http://www.w3.org/2005/Atom">
  <id>https://contoso.servicebus.windows.net/$namespaceinfo?api-version=2021-05</id>
  <title type="text">contoso</title>
  <updated>2026-07-20T10:00:00Z</updated>
  <author><name>contoso</name></author>
  <content type="application/xml">
    <NamespaceInfo xmlns="http://schemas.microsoft.com/netservices/2010/10/servicebus/connect"
                   xmlns:i="http://www.w3.org/2001/XMLSchema-instance">
      <Alias i:nil="true"/>
      <CreatedTime>2024-03-01T08:30:00.123Z</CreatedTime>
      <MessagingSKU>Standard</MessagingSKU>
      <MessagingUnits i:nil="true">0</MessagingUnits>
      <ModifiedTime>2026-07-20T10:00:00Z</ModifiedTime>
      <Name>contoso</Name>
      <NamespaceType>Messaging</NamespaceType>
    </NamespaceInfo>
  </content>
</entry>"#;

    /// Captured shape of a queue feed entry (fields the service actually emits).
    const QUEUE_FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title type="text">Queues</title>
  <entry>
    <id>https://contoso.servicebus.windows.net/orders?api-version=2021-05</id>
    <title type="text">orders</title>
    <content type="application/xml">
      <QueueDescription xmlns="http://schemas.microsoft.com/netservices/2010/10/servicebus/connect"
                        xmlns:i="http://www.w3.org/2001/XMLSchema-instance">
        <LockDuration>PT1M</LockDuration>
        <MaxSizeInMegabytes>1024</MaxSizeInMegabytes>
        <RequiresDuplicateDetection>false</RequiresDuplicateDetection>
        <RequiresSession>false</RequiresSession>
        <DefaultMessageTimeToLive>P10675199DT2H48M5.4775807S</DefaultMessageTimeToLive>
        <DeadLetteringOnMessageExpiration>true</DeadLetteringOnMessageExpiration>
        <DuplicateDetectionHistoryTimeWindow>PT10M</DuplicateDetectionHistoryTimeWindow>
        <MaxDeliveryCount>10</MaxDeliveryCount>
        <EnableBatchedOperations>true</EnableBatchedOperations>
        <SizeInBytes>2048</SizeInBytes>
        <MessageCount>7</MessageCount>
        <IsAnonymousAccessible>false</IsAnonymousAccessible>
        <Status>Active</Status>
        <CreatedAt>2024-03-01T08:30:00Z</CreatedAt>
        <UpdatedAt>2026-07-01T09:00:00Z</UpdatedAt>
        <AccessedAt>2026-07-20T10:00:00Z</AccessedAt>
        <SupportOrdering>true</SupportOrdering>
        <CountDetails xmlns:d2p1="http://schemas.microsoft.com/netservices/2011/06/servicebus">
          <d2p1:ActiveMessageCount>5</d2p1:ActiveMessageCount>
          <d2p1:DeadLetterMessageCount>2</d2p1:DeadLetterMessageCount>
          <d2p1:ScheduledMessageCount>0</d2p1:ScheduledMessageCount>
          <d2p1:TransferMessageCount>0</d2p1:TransferMessageCount>
          <d2p1:TransferDeadLetterMessageCount>0</d2p1:TransferDeadLetterMessageCount>
        </CountDetails>
        <AutoDeleteOnIdle>P10675199DT2H48M5.4775807S</AutoDeleteOnIdle>
        <EnablePartitioning>false</EnablePartitioning>
        <EnableExpress>false</EnableExpress>
      </QueueDescription>
    </content>
  </entry>
</feed>"#;

    const RULE_FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <title type="text">$Default</title>
    <content type="application/xml">
      <RuleDescription xmlns="http://schemas.microsoft.com/netservices/2010/10/servicebus/connect"
                       xmlns:i="http://www.w3.org/2001/XMLSchema-instance">
        <Filter i:type="SqlFilter">
          <SqlExpression>1=1</SqlExpression>
          <CompatibilityLevel>20</CompatibilityLevel>
        </Filter>
        <Action i:type="EmptyRuleAction"/>
        <CreatedAt>2024-03-01T08:30:00Z</CreatedAt>
        <Name>$Default</Name>
      </RuleDescription>
    </content>
  </entry>
</feed>"#;

    #[test]
    fn parses_namespace_info_entry() {
        let info = parse_namespace_info(NAMESPACE_INFO).unwrap();
        assert_eq!(info.name, "contoso");
        assert_eq!(info.namespace_type.as_deref(), Some("Messaging"));
        assert_eq!(info.messaging_sku.as_deref(), Some("Standard"));
        assert!(info.alias.is_none());
        // nil-marked elements are ignored even when they contain text
        assert!(info.messaging_units.is_none());
        assert_eq!(info.created_time.unwrap().year(), 2024);
        assert_eq!(info.modified_time.unwrap().year(), 2026);
    }

    #[test]
    fn missing_name_is_an_error() {
        assert!(matches!(
            parse_namespace_info("<entry></entry>"),
            Err(MgmtError::Xml(_))
        ));
    }

    #[test]
    fn parses_queue_feed_with_counts() {
        let queues = parse_queue_feed(QUEUE_FEED).unwrap();
        assert_eq!(queues.len(), 1);
        let queue = &queues[0];
        assert_eq!(queue.properties.name, "orders");
        assert!(queue.properties.dead_lettering_on_message_expiration);
        assert!(crate::model::is_unlimited(
            queue.properties.default_message_time_to_live
        ));
        assert_eq!(queue.runtime.message_count, 7);
        assert_eq!(queue.runtime.count_details.active, 5);
        assert_eq!(queue.runtime.count_details.dead_letter, 2);
        assert_eq!(queue.runtime.size_in_bytes, 2048);
    }

    #[test]
    fn single_entry_parses_as_queue() {
        // GET of a single queue returns a bare <entry>, not a feed.
        let start = QUEUE_FEED.find("<entry>").unwrap();
        let end = QUEUE_FEED.find("</entry>").unwrap() + "</entry>".len();
        let entry = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>{}"#,
            QUEUE_FEED[start..end]
                .replace("<entry>", r#"<entry xmlns="http://www.w3.org/2005/Atom">"#)
        );
        let queue = parse_queue(&entry).unwrap().unwrap();
        assert_eq!(queue.properties.name, "orders");
    }

    #[test]
    fn parses_default_sql_rule() {
        let rules = parse_rule_feed(RULE_FEED, "events", "audit").unwrap();
        assert_eq!(rules.len(), 1);
        let rule = &rules[0];
        assert_eq!(rule.properties.name, "$Default");
        assert_eq!(rule.properties.topic, "events");
        assert_eq!(
            rule.properties.filter,
            RuleFilter::Sql {
                expression: "1=1".into()
            }
        );
        assert!(rule.properties.action.is_none());
    }
}
