//! Opt-in property edit and rule recovery test against a real namespace.
//! Only creates uniquely named scratch entities; cleanup also runs on panic.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sift_backend::{Command, EntityDescription, EntityInfo, Event, RequestId};
use sift_core::config::NamespaceProfile;
use sift_core::connection::NamespaceConnection;
use sift_core::secrets::SecretString;
use sift_mgmt::{
    ManagementClient, QueueProperties, RuleFilter, RuleProperties, SubscriptionProperties,
    TopicProperties,
};

struct Scratch {
    client: ManagementClient,
    runtime: tokio::runtime::Runtime,
    queue: String,
    topic: String,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        self.runtime.block_on(async {
            let _ = self.client.delete_queue(&self.queue).await;
            let _ = self.client.delete_topic(&self.topic).await;
        });
    }
}

fn until<T>(
    events: &crossbeam_channel::Receiver<Event>,
    mut pick: impl FnMut(Event) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("backend response before timeout");
        if let Some(value) = pick(events.recv_timeout(remaining).expect("backend event")) {
            return value;
        }
    }
}

fn mutation(
    events: &crossbeam_channel::Receiver<Event>,
    request: RequestId,
) -> Result<Option<EntityInfo>, sift_backend::BackendError> {
    until(events, |event| match event {
        Event::Mutated { req, result, .. } if req == request => Some(result),
        _ => None,
    })
}

#[allow(clippy::too_many_lines)] // One opt-in cloud scenario with shared scratch cleanup.
#[test]
fn live_property_updates_and_rejected_rule_recovery() {
    let Ok(connection_string) = std::env::var("SIFT_TEST_SB_CONNECTION_STRING") else {
        eprintln!("skipped: SIFT_TEST_SB_CONNECTION_STRING not set");
        return;
    };
    if std::env::var("SIFT_TEST_SB_MUTATE").as_deref() != Ok("1") {
        eprintln!("skipped: SIFT_TEST_SB_MUTATE != 1");
        return;
    }
    let conn = NamespaceConnection::parse(&connection_string).expect("valid connection string");
    let suffix = uuid::Uuid::new_v4();
    let scratch = Scratch {
        client: ManagementClient::new(&conn).expect("management client"),
        runtime: tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime"),
        queue: format!("sift-test-edit-q-{suffix}"),
        topic: format!("sift-test-edit-t-{suffix}"),
    };
    let queue = scratch
        .runtime
        .block_on(scratch.client.create_queue(&QueueProperties {
            name: scratch.queue.clone(),
            lock_duration: Duration::from_secs(90),
            user_metadata: Some("original metadata".into()),
            ..Default::default()
        }))
        .expect("scratch queue");
    let topic = scratch
        .runtime
        .block_on(scratch.client.create_topic(&TopicProperties {
            name: scratch.topic.clone(),
            ..Default::default()
        }))
        .expect("scratch topic");
    let subscription = scratch
        .runtime
        .block_on(scratch.client.create_subscription(&SubscriptionProperties {
            topic: scratch.topic.clone(),
            name: "audit".into(),
            ..Default::default()
        }))
        .expect("scratch subscription");
    let original_rule = RuleProperties {
        topic: scratch.topic.clone(),
        subscription: "audit".into(),
        name: "priority".into(),
        filter: RuleFilter::Sql {
            expression: "priority > 3".into(),
        },
        action: None,
    };
    scratch
        .runtime
        .block_on(scratch.client.create_rule(&original_rule))
        .expect("scratch rule");

    let (backend, events) = sift_backend::spawn(Arc::new(|| {}));
    let profile = NamespaceProfile::new_connection_string("edit live test".into());
    let ns = profile.id;
    backend.send(Command::Connect {
        req: backend.next_request(),
        profile,
        secret: SecretString::from(connection_string),
    });
    until(&events, |event| match event {
        Event::Connected { result, .. } => Some(result.expect("connect")),
        _ => None,
    });

    let mut queue_properties = queue.properties.clone();
    queue_properties.max_delivery_count = 14;
    queue_properties.default_message_time_to_live = Duration::from_secs(3600);
    let req = backend.next_request();
    backend.send(Command::UpdateEntity {
        req,
        ns,
        desc: EntityDescription::Queue(queue_properties),
    });
    let Some(EntityInfo::Queue(updated)) = mutation(&events, req).expect("queue update") else {
        panic!("updated queue response");
    };
    assert_eq!(updated.properties.max_delivery_count, 14);
    assert_eq!(
        updated.properties.lock_duration,
        queue.properties.lock_duration
    );
    assert_eq!(
        updated.properties.user_metadata,
        queue.properties.user_metadata
    );

    let mut topic_properties = topic.properties;
    topic_properties.user_metadata = Some("edited topic".into());
    let req = backend.next_request();
    backend.send(Command::UpdateEntity {
        req,
        ns,
        desc: EntityDescription::Topic(topic_properties),
    });
    let Some(EntityInfo::Topic(updated)) = mutation(&events, req).expect("topic update") else {
        panic!("updated topic response");
    };
    assert_eq!(
        updated.properties.user_metadata.as_deref(),
        Some("edited topic")
    );

    let mut subscription_properties = subscription.properties;
    subscription_properties.max_delivery_count = 18;
    let req = backend.next_request();
    backend.send(Command::UpdateEntity {
        req,
        ns,
        desc: EntityDescription::Subscription(subscription_properties),
    });
    let Some(EntityInfo::Subscription(updated)) =
        mutation(&events, req).expect("subscription update")
    else {
        panic!("updated subscription response");
    };
    assert_eq!(updated.properties.max_delivery_count, 18);

    let mut replacement = original_rule.clone();
    replacement.filter = RuleFilter::Sql {
        expression: "priority > 5".into(),
    };
    let req = backend.next_request();
    backend.send(Command::UpdateEntity {
        req,
        ns,
        desc: EntityDescription::Rule(replacement.clone()),
    });
    let Some(EntityInfo::Rule(updated)) = mutation(&events, req).expect("rule replacement") else {
        panic!("updated rule response");
    };
    assert_eq!(updated.properties, replacement);

    let mut rejected = replacement.clone();
    rejected.filter = RuleFilter::Sql {
        expression: "priority >".into(),
    };
    let req = backend.next_request();
    backend.send(Command::UpdateEntity {
        req,
        ns,
        desc: EntityDescription::Rule(rejected),
    });
    let error = mutation(&events, req).expect_err("invalid SQL rejected");
    assert!(error.message.contains("original rule was restored"));
    let rules = scratch
        .runtime
        .block_on(scratch.client.list_rules(&scratch.topic, "audit"))
        .expect("rules after restoration");
    assert_eq!(
        rules
            .into_iter()
            .find(|rule| rule.properties.name == "priority")
            .expect("restored rule")
            .properties,
        replacement
    );
    backend.send(Command::Disconnect { ns });
}
