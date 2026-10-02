//! A portable JSON snapshot of a namespace's entity descriptions, for
//! export/import. This is sift's own format (not the .NET XML format);
//! durations serialize via serde's default `Duration` representation.

use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
use crate::client::ManagementClient;
#[cfg(not(target_arch = "wasm32"))]
use crate::error::MgmtError;
use crate::model::{QueueProperties, RuleProperties, SubscriptionProperties, TopicProperties};

/// Everything sift exports and re-creates: entity descriptions only (no
/// message data, no runtime counters).
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Check the schema before interpreting descriptions or mutating a namespace.
    pub fn validate_version(&self) -> Result<(), UnsupportedExportVersion> {
        if self.sift_export_version != Self::VERSION {
            return Err(UnsupportedExportVersion(self.sift_export_version));
        }
        Ok(())
    }

    /// Validate every rule before creating any parent entities. Unknown type
    /// names already fail JSON deserialization; this catches invalid values
    /// and inconsistent metadata in an otherwise supported export.
    pub fn validate_contents(&self) -> Result<(), String> {
        for rule in &self.rules {
            rule.filter.validate().map_err(|error| {
                format!(
                    "rule '{}/{}/{}': {error}",
                    rule.topic, rule.subscription, rule.name
                )
            })?;
        }
        Ok(())
    }

    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.queues.len() + self.topics.len() + self.subscriptions.len() + self.rules.len()
    }
}

impl Default for NamespaceExport {
    fn default() -> Self {
        Self {
            sift_export_version: Self::VERSION,
            queues: Vec::new(),
            topics: Vec::new(),
            subscriptions: Vec::new(),
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "unsupported namespace export version {0}; this Sift version supports version 1. Use a compatible Sift version without changing the export's version marker"
)]
pub struct UnsupportedExportVersion(pub u32);

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
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
pub async fn import(
    client: &ManagementClient,
    data: &NamespaceExport,
    policy: ImportPolicy,
) -> ImportOutcome {
    let mut outcome = ImportOutcome::default();
    if let Err(error) = data.validate_version() {
        outcome.errors.push(error.to_string());
        return outcome;
    }
    if let Err(error) = data.validate_contents() {
        outcome.errors.push(error);
        return outcome;
    }

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
        let label = format!("{}/{}/{}", rule.topic, rule.subscription, rule.name);
        match import_rule(client, rule, policy).await {
            Ok(RuleImport::Created) => outcome.created += 1,
            Ok(RuleImport::Updated) => outcome.updated += 1,
            Ok(RuleImport::Skipped) => outcome.skipped += 1,
            Err(e) => outcome.errors.push(format!("{label}: {e}")),
        }
    }
    outcome
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
enum RuleImport {
    Created,
    Updated,
    Skipped,
}

#[cfg(not(target_arch = "wasm32"))]
async fn import_rule(
    client: &ManagementClient,
    replacement: &RuleProperties,
    policy: ImportPolicy,
) -> Result<RuleImport, String> {
    // A failed read is not evidence that a rule is missing. Preserve routing
    // rather than attempting a create after an authorization/network failure.
    let original = client
        .list_rules(&replacement.topic, &replacement.subscription)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|rule| rule.properties.name == replacement.name);
    match original {
        None => client
            .create_rule(replacement)
            .await
            .map(|_| RuleImport::Created)
            .map_err(|error| error.to_string()),
        Some(_) if policy == ImportPolicy::Skip => Ok(RuleImport::Skipped),
        Some(original) => {
            replace_imported_rule(
                original.properties,
                replacement.clone(),
                || {
                    client.delete_rule(
                        &replacement.topic,
                        &replacement.subscription,
                        &replacement.name,
                    )
                },
                |properties| async move { client.create_rule(&properties).await },
            )
            .await?;
            Ok(RuleImport::Updated)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn replace_imported_rule<Delete, DeleteFuture, Create, CreateFuture>(
    original: RuleProperties,
    replacement: RuleProperties,
    delete: Delete,
    mut create: Create,
) -> Result<(), String>
where
    Delete: FnOnce() -> DeleteFuture,
    DeleteFuture: Future<Output = Result<(), MgmtError>>,
    Create: FnMut(RuleProperties) -> CreateFuture,
    CreateFuture: Future<Output = Result<crate::model::RuleInfo, MgmtError>>,
{
    original.filter.validate()?;
    replacement.filter.validate()?;
    delete().await.map_err(|error| format!(
        "Could not delete the existing rule: {error}. Replacement was not attempted; refresh to check its state."
    ))?;
    match create(replacement).await {
        Ok(_) => Ok(()),
        Err(replacement_error) => match create(original).await {
            Ok(_) => Err(format!(
                "Rule replacement failed: {replacement_error}. The original rule was restored."
            )),
            Err(restore_error) => Err(format!(
                "Rule replacement failed: {replacement_error}. Restoring the original rule also failed: {restore_error}. Refresh the subscription and restore its routing before retrying."
            )),
        },
    }
}

/// Shared create-or-update-or-skip flow for entities that have an update verb.
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use azure_core::credentials::{AccessToken, TokenCredential, TokenRequestOptions};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, Default)]
    struct CountingCredential(AtomicUsize);

    #[async_trait::async_trait]
    impl TokenCredential for CountingCredential {
        async fn get_token(
            &self,
            _: &[&str],
            _: Option<TokenRequestOptions>,
        ) -> azure_core::Result<AccessToken> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(azure_core::Error::message(
                azure_core::error::ErrorKind::Credential,
                "no external authentication is permitted in this test",
            ))
        }
    }

    #[tokio::test]
    async fn rejects_future_export_before_requesting_credentials_or_mutating_entities() {
        let credential = Arc::new(CountingCredential::default());
        let client = ManagementClient::new_with_credential(
            &url::Url::parse("sb://not-a-live-namespace.servicebus.windows.net/").unwrap(),
            credential.clone(),
        )
        .unwrap();
        let export = NamespaceExport {
            sift_export_version: 2,
            queues: vec![QueueProperties {
                name: "must-not-be-created".into(),
                ..QueueProperties::default()
            }],
            ..NamespaceExport::default()
        };
        let outcome = import(&client, &export, ImportPolicy::Overwrite).await;
        assert_eq!(
            (outcome.created, outcome.updated, outcome.skipped),
            (0, 0, 0)
        );
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("unsupported namespace export version 2"));
        assert_eq!(credential.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn current_export_default_is_valid_and_invalid_markers_are_rejected() {
        assert_eq!(NamespaceExport::default().sift_export_version, 1);
        assert!(NamespaceExport::default().validate_version().is_ok());
        for version in [0, 2, u32::MAX] {
            let export = NamespaceExport {
                sift_export_version: version,
                ..NamespaceExport::default()
            };
            assert_eq!(
                export.validate_version(),
                Err(UnsupportedExportVersion(version))
            );
        }
    }

    fn rule(expression: &str) -> RuleProperties {
        RuleProperties {
            topic: "events".into(),
            subscription: "audit".into(),
            name: "routing".into(),
            filter: crate::model::RuleFilter::Sql {
                expression: expression.into(),
            },
            action: Some("SET source = 'preserve original action'".into()),
        }
    }

    fn typed_rule() -> RuleProperties {
        use crate::model::{CorrelationPropertyType as Kind, RuleFilter};
        RuleProperties {
            filter: RuleFilter::Correlation {
                correlation_id: None,
                message_id: None,
                to: None,
                reply_to: None,
                subject: None,
                session_id: None,
                reply_to_session_id: None,
                content_type: None,
                properties: vec![
                    ("priority".into(), "9223372036854775807".into()),
                    ("enabled".into(), "true".into()),
                ],
                property_types: [
                    ("priority".into(), Kind::Long),
                    ("enabled".into(), Kind::Boolean),
                ]
                .into(),
            },
            ..rule("1=1")
        }
    }

    #[test]
    fn export_json_preserves_types_and_legacy_string_exports_still_load() {
        let original = typed_rule();
        let export = NamespaceExport {
            rules: vec![original.clone()],
            ..NamespaceExport::default()
        };
        let mut json = serde_json::to_value(&export).unwrap();
        let decoded: NamespaceExport = serde_json::from_value(json.clone()).unwrap();
        decoded.validate_contents().unwrap();
        assert_eq!(decoded.rules, vec![original]);
        json["rules"][0]["filter"]["Correlation"]
            .as_object_mut()
            .unwrap()
            .remove("property_types");
        let legacy: NamespaceExport = serde_json::from_value(json.clone()).unwrap();
        legacy.validate_contents().unwrap();
        let crate::model::RuleFilter::Correlation { property_types, .. } = &legacy.rules[0].filter
        else {
            panic!("expected correlation");
        };
        assert!(property_types.is_empty());
        json["rules"][0]["filter"]["Correlation"]["property_types"] =
            serde_json::json!({"priority": "futureNumeric"});
        assert!(serde_json::from_value::<NamespaceExport>(json).is_err());
    }

    #[tokio::test]
    async fn invalid_typed_rule_prevents_all_parent_entity_mutations() {
        let credential = Arc::new(CountingCredential::default());
        let client = ManagementClient::new_with_credential(
            &url::Url::parse("sb://not-a-live-namespace.servicebus.windows.net/").unwrap(),
            credential.clone(),
        )
        .unwrap();
        let mut invalid = typed_rule();
        let crate::model::RuleFilter::Correlation { properties, .. } = &mut invalid.filter else {
            panic!("expected correlation");
        };
        properties[1].1 = "yes".into();
        let export = NamespaceExport {
            queues: vec![QueueProperties {
                name: "must-not-be-created".into(),
                ..QueueProperties::default()
            }],
            rules: vec![invalid],
            ..NamespaceExport::default()
        };
        let outcome = import(&client, &export, ImportPolicy::Overwrite).await;
        assert_eq!(
            (outcome.created, outcome.updated, outcome.skipped),
            (0, 0, 0)
        );
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("boolean"));
        assert_eq!(credential.0.load(Ordering::SeqCst), 0);
    }

    fn rejected(detail: &str) -> MgmtError {
        MgmtError::BadRequest {
            detail: detail.into(),
        }
    }

    #[tokio::test]
    async fn import_rule_deletion_failure_never_attempts_replacement() {
        let result = replace_imported_rule(
            rule("1=1"),
            rule("priority > 3"),
            || std::future::ready(Err(rejected("deletion failed"))),
            |_| -> std::future::Ready<Result<crate::model::RuleInfo, MgmtError>> {
                panic!("replacement must not follow failed deletion")
            },
        )
        .await;
        assert!(
            result
                .unwrap_err()
                .contains("Replacement was not attempted")
        );
    }

    #[tokio::test]
    async fn import_rejected_replacement_restores_exact_original_filter_and_action() {
        let original = typed_rule();
        let replacement = rule("invalid SQL");
        let mut attempted = Vec::new();
        let result = replace_imported_rule(
            original.clone(),
            replacement.clone(),
            || std::future::ready(Ok(())),
            |properties| {
                attempted.push(properties.clone());
                if attempted.len() == 1 {
                    std::future::ready(Err(rejected("replacement rejected")))
                } else {
                    std::future::ready(Ok(crate::model::RuleInfo {
                        properties,
                        created_at: None,
                    }))
                }
            },
        )
        .await;
        assert_eq!(attempted, vec![replacement, original]);
        assert!(result.unwrap_err().contains("original rule was restored"));
    }

    #[tokio::test]
    async fn import_rule_invalid_metadata_never_deletes_the_original() {
        for invalid_original in [false, true] {
            let mut invalid = typed_rule();
            let crate::model::RuleFilter::Correlation { property_types, .. } = &mut invalid.filter
            else {
                panic!("expected correlation");
            };
            property_types.insert(
                "missing-property".into(),
                crate::model::CorrelationPropertyType::Int,
            );
            let (original, replacement) = if invalid_original {
                (invalid, typed_rule())
            } else {
                (typed_rule(), invalid)
            };
            let result = replace_imported_rule(
                original,
                replacement,
                || -> std::future::Ready<Result<(), MgmtError>> {
                    panic!("invalid original or replacement must not be deleted");
                },
                |_| -> std::future::Ready<Result<crate::model::RuleInfo, MgmtError>> {
                    panic!("invalid original or replacement must not be created");
                },
            )
            .await;
            assert!(result.unwrap_err().contains("missing property"));
        }
    }

    #[tokio::test]
    async fn import_restoration_failure_reports_both_failures() {
        let mut attempts = 0;
        let result = replace_imported_rule(
            rule("1=1"),
            rule("invalid SQL"),
            || std::future::ready(Ok(())),
            |_| {
                attempts += 1;
                std::future::ready(Err(rejected(if attempts == 1 {
                    "replacement rejected"
                } else {
                    "original restore rejected"
                })))
            },
        )
        .await;
        let error = result.unwrap_err();
        assert!(error.contains("replacement rejected"));
        assert!(error.contains("original restore rejected"));
        assert_eq!(attempts, 2);
    }

    #[tokio::test]
    async fn successful_import_replacement_does_not_restore_the_original() {
        let mut attempted = Vec::new();
        let replacement = rule("priority > 3");
        replace_imported_rule(
            rule("1=1"),
            replacement.clone(),
            || std::future::ready(Ok(())),
            |properties| {
                attempted.push(properties.clone());
                std::future::ready(Ok(crate::model::RuleInfo {
                    properties,
                    created_at: None,
                }))
            },
        )
        .await
        .unwrap();
        assert_eq!(attempted, vec![replacement]);
    }
}
