//! A portable JSON snapshot of a namespace's entity descriptions, for
//! export/import. This is sift's own format (not the .NET XML format);
//! durations serialize via serde's default `Duration` representation.

use serde::{Deserialize, Serialize};

use crate::client::ManagementClient;
use crate::error::MgmtError;
use crate::model::{QueueProperties, RuleProperties, SubscriptionProperties, TopicProperties};

/// Everything sift exports and re-creates: entity descriptions only (no
/// message data, no runtime counters).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NamespaceExport {
    /// Schema marker so future formats can be distinguished.
    pub sift_export_version: u32,
    #[serde(default)]
    pub queues: Vec<QueueProperties>,
    #[serde(default)]
    pub topics: Vec<TopicProperties>,
    #[serde(default)]
    pub subscriptions: Vec<SubscriptionProperties>,
    #[serde(default)]
    pub rules: Vec<RuleProperties>,
}

impl NamespaceExport {
    pub const VERSION: u32 = 1;

    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.queues.len() + self.topics.len() + self.subscriptions.len() + self.rules.len()
    }
}

/// How to treat an entity that already exists on import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportPolicy {
    /// Leave existing entities untouched.
    Skip,
    /// Update existing entities to match the import.
    Overwrite,
}

/// Result of an import: created, updated, skipped, and any per-entity errors.
#[derive(Debug, Default)]
pub struct ImportOutcome {
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

impl std::fmt::Display for ImportOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} created, {} updated, {} skipped, {} error(s)",
            self.created,
            self.updated,
            self.skipped,
            self.errors.len()
        )
    }
}

/// Collect every queue, topic, subscription, and rule description.
pub async fn export(client: &ManagementClient) -> Result<NamespaceExport, MgmtError> {
    let mut out = NamespaceExport {
        sift_export_version: NamespaceExport::VERSION,
        ..NamespaceExport::default()
    };

    for queue in client.list_queues().await? {
        out.queues.push(queue.properties);
    }
    for topic in client.list_topics().await? {
        let topic_name = topic.properties.name.clone();
        out.topics.push(topic.properties);
        for sub in client.list_subscriptions(&topic_name).await? {
            let sub_name = sub.properties.name.clone();
            out.subscriptions.push(sub.properties);
            for rule in client.list_rules(&topic_name, &sub_name).await? {
                out.rules.push(rule.properties);
            }
        }
    }
    Ok(out)
}

/// Create (or update) entities from an export. Parents are created before
/// children so subscriptions and rules land on existing topics.
pub async fn import(
    client: &ManagementClient,
    data: &NamespaceExport,
    policy: ImportPolicy,
) -> ImportOutcome {
    let mut outcome = ImportOutcome::default();

    for queue in &data.queues {
        apply(
            &mut outcome,
            queue.name.clone(),
            client.get_queue(&queue.name).await.is_ok(),
            policy,
            || client.create_queue(queue),
            || client.update_queue(queue),
        )
        .await;
    }
    for topic in &data.topics {
        apply(
            &mut outcome,
            topic.name.clone(),
            client.get_topic(&topic.name).await.is_ok(),
            policy,
            || client.create_topic(topic),
            || client.update_topic(topic),
        )
        .await;
    }
    for sub in &data.subscriptions {
        let label = format!("{}/{}", sub.topic, sub.name);
        apply(
            &mut outcome,
            label,
            client.get_subscription(&sub.topic, &sub.name).await.is_ok(),
            policy,
            || client.create_subscription(sub),
            || client.update_subscription(sub),
        )
        .await;
    }
    for rule in &data.rules {
        // Rules have no update verb; recreate on overwrite.
        let exists = client
            .list_rules(&rule.topic, &rule.subscription)
            .await
            .is_ok_and(|rules| rules.iter().any(|r| r.properties.name == rule.name));
        let label = format!("{}/{}/{}", rule.topic, rule.subscription, rule.name);
        if exists && policy == ImportPolicy::Skip {
            outcome.skipped += 1;
            continue;
        }
        if exists {
            let _ = client
                .delete_rule(&rule.topic, &rule.subscription, &rule.name)
                .await;
        }
        match client.create_rule(rule).await {
            Ok(_) if exists => outcome.updated += 1,
            Ok(_) => outcome.created += 1,
            Err(e) => outcome.errors.push(format!("{label}: {e}")),
        }
    }
    outcome
}

/// Shared create-or-update-or-skip flow for entities that have an update verb.
async fn apply<T, C, U, CFut, UFut>(
    outcome: &mut ImportOutcome,
    label: String,
    exists: bool,
    policy: ImportPolicy,
    create: C,
    update: U,
) where
    C: FnOnce() -> CFut,
    U: FnOnce() -> UFut,
    CFut: Future<Output = Result<T, MgmtError>>,
    UFut: Future<Output = Result<T, MgmtError>>,
{
    match (exists, policy) {
        (true, ImportPolicy::Skip) => outcome.skipped += 1,
        (true, ImportPolicy::Overwrite) => match update().await {
            Ok(_) => outcome.updated += 1,
            Err(e) => outcome.errors.push(format!("{label}: {e}")),
        },
        (false, _) => match create().await {
            Ok(_) => outcome.created += 1,
            Err(e) => outcome.errors.push(format!("{label}: {e}")),
        },
    }
}
