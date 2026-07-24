//! Live integration tests against a real Service Bus namespace.
//!
//! These self-skip unless `SIFT_TEST_SB_CONNECTION_STRING` is set, so they are
//! safe in CI. The mutation test additionally requires `SIFT_TEST_SB_MUTATE=1`
//! and only ever touches uniquely-named `sift-test-*` entities.

use std::time::Duration;

use sift_core::connection::NamespaceConnection;
use sift_mgmt::{
    ManagementClient, MgmtError, QueueProperties, RuleFilter, RuleProperties,
    SubscriptionProperties, TopicProperties,
};

fn client() -> Option<ManagementClient> {
    let connection_string = std::env::var("SIFT_TEST_SB_CONNECTION_STRING").ok()?;
    let conn = NamespaceConnection::parse(&connection_string)
        .expect("SIFT_TEST_SB_CONNECTION_STRING must be a valid connection string");
    Some(ManagementClient::new(&conn).expect("client construction"))
}

#[tokio::test]
async fn live_read_only() {
    let Some(client) = client() else {
        eprintln!("skipped: SIFT_TEST_SB_CONNECTION_STRING not set");
        return;
    };

    let info = client.get_namespace_info().await.expect("namespace info");
    assert!(!info.name.is_empty());

    let queues = client.list_queues().await.expect("list queues");
    let topics = client.list_topics().await.expect("list topics");
    eprintln!(
        "namespace '{}' ({:?}): {} queues, {} topics",
        info.name,
        info.messaging_sku,
        queues.len(),
        topics.len()
    );

    if let Some(queue) = queues.first() {
        let name = &queue.properties.name;
        let got = client.get_queue(name).await.expect("get queue");
        assert_eq!(&got.properties.name, name);
        eprintln!(
            "queue '{name}': active={}, dead-letter={}, scheduled={}",
            got.runtime.count_details.active,
            got.runtime.count_details.dead_letter,
            got.runtime.count_details.scheduled,
        );
    }

    if let Some(topic) = topics.first() {
        let topic_name = &topic.properties.name;
        let subs = client
            .list_subscriptions(topic_name)
            .await
            .expect("list subscriptions");
        eprintln!("topic '{topic_name}': {} subscriptions", subs.len());
        if let Some(sub) = subs.first() {
            let rules = client
                .list_rules(topic_name, &sub.properties.name)
                .await
                .expect("list rules");
            eprintln!(
                "subscription '{}': {} rules",
                sub.properties.name,
                rules.len()
            );
        }
    }
}

/// Full create → get → update → delete cycle. This is the test that validates
/// the XSD element order of the hand-written PUT bodies against the real
/// service (a wrong order fails with HTTP 400).
#[tokio::test]
async fn live_crud_cycle() {
    let Some(client) = client() else {
        eprintln!("skipped: SIFT_TEST_SB_CONNECTION_STRING not set");
        return;
    };
    if std::env::var("SIFT_TEST_SB_MUTATE").as_deref() != Ok("1") {
        eprintln!("skipped: SIFT_TEST_SB_MUTATE != 1");
        return;
    }

    let suffix = uuid::Uuid::new_v4();
    let queue_name = format!("sift-test-q-{suffix}");
    let topic_name = format!("sift-test-t-{suffix}");

    // Queue: create with non-default fields, verify they round-trip.
    let queue_props = QueueProperties {
        name: queue_name.clone(),
        lock_duration: Duration::from_secs(90),
        max_delivery_count: 7,
        dead_lettering_on_message_expiration: true,
        requires_session: false,
        user_metadata: Some("created by sift live test".into()),
        ..QueueProperties::default()
    };
    let created = client
        .create_queue(&queue_props)
        .await
        .expect("create queue");
    assert_eq!(created.properties.lock_duration, Duration::from_secs(90));
    assert_eq!(created.properties.max_delivery_count, 7);
    assert!(created.properties.dead_lettering_on_message_expiration);

    // Update: bump max delivery count.
    let mut updated_props = created.properties.clone();
    updated_props.max_delivery_count = 12;
    let updated = client
        .update_queue(&updated_props)
        .await
        .expect("update queue");
    assert_eq!(updated.properties.max_delivery_count, 12);

    // Topic + subscription + SQL rule.
    let topic = client
        .create_topic(&TopicProperties {
            name: topic_name.clone(),
            ..TopicProperties::default()
        })
        .await
        .expect("create topic");
    assert_eq!(topic.properties.name, topic_name);

    let sub = client
        .create_subscription(&SubscriptionProperties {
            topic: topic_name.clone(),
            name: "sift-test-sub".into(),
            max_delivery_count: 5,
            ..SubscriptionProperties::default()
        })
        .await
        .expect("create subscription");
    assert_eq!(sub.properties.max_delivery_count, 5);

    let rule = client
        .create_rule(&RuleProperties {
            topic: topic_name.clone(),
            subscription: "sift-test-sub".into(),
            name: "high-priority".into(),
            filter: RuleFilter::Sql {
                expression: "priority > 3".into(),
            },
            action: None,
        })
        .await
        .expect("create rule");
    assert_eq!(
        rule.properties.filter,
        RuleFilter::Sql {
            expression: "priority > 3".into()
        }
    );

    let rules = client
        .list_rules(&topic_name, "sift-test-sub")
        .await
        .expect("list rules");
    assert!(rules.iter().any(|r| r.properties.name == "high-priority"));

    // Cleanup (deleting the topic removes its subscriptions and rules).
    client
        .delete_queue(&queue_name)
        .await
        .expect("delete queue");
    client
        .delete_topic(&topic_name)
        .await
        .expect("delete topic");

    // Deleted entities must be gone.
    assert!(matches!(
        client.get_queue(&queue_name).await,
        Err(MgmtError::NotFound { .. })
    ));
    eprintln!("CRUD cycle OK: {queue_name}, {topic_name}");
}
