//! Rule replacement with recovery of the original filter if replacement fails.

use std::future::Future;

use sift_mgmt::{MgmtError, RuleInfo, RuleProperties};

use crate::bridge::BackendError;

pub(crate) async fn replace<Delete, DeleteFuture, Create, CreateFuture>(
    original: RuleProperties,
    replacement: RuleProperties,
    delete: Delete,
    mut create: Create,
) -> Result<RuleInfo, BackendError>
where
    Delete: FnOnce() -> DeleteFuture,
    DeleteFuture: Future<Output = Result<(), MgmtError>>,
    Create: FnMut(RuleProperties) -> CreateFuture,
    CreateFuture: Future<Output = Result<RuleInfo, MgmtError>>,
{
    original.filter.validate().map_err(BackendError::new)?;
    replacement.filter.validate().map_err(BackendError::new)?;
    delete().await.map_err(|error| BackendError::new(format!(
        "Could not delete the existing rule: {error}. Replacement was not attempted; refresh to check its state."
    )))?;
    match create(replacement).await {
        Ok(rule) => Ok(rule),
        Err(replacement_error) => match create(original).await {
            Ok(_) => Err(BackendError::new(format!(
                "Rule replacement failed: {replacement_error}. The original rule was restored; your edits remain available."
            ))),
            Err(restore_error) => Err(BackendError::new(format!(
                "Rule replacement failed: {replacement_error}. Restoring the original rule also failed: {restore_error}. Refresh the subscription and restore its routing before retrying."
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::future::ready;
    use std::rc::Rc;

    use sift_mgmt::RuleFilter;

    use super::*;

    fn properties(expression: &str) -> RuleProperties {
        RuleProperties {
            topic: "events".into(),
            subscription: "audit".into(),
            name: "rule".into(),
            filter: RuleFilter::Sql {
                expression: expression.into(),
            },
            action: None,
        }
    }

    fn run<T>(future: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime")
            .block_on(future)
    }

    fn rejected() -> MgmtError {
        MgmtError::BadRequest {
            detail: "invalid filter".into(),
        }
    }

    #[test]
    fn invalid_original_or_replacement_is_rejected_before_deletion() {
        let mut invalid = properties("1=1");
        invalid.filter = RuleFilter::Correlation {
            correlation_id: None,
            message_id: None,
            to: None,
            reply_to: None,
            subject: None,
            session_id: None,
            reply_to_session_id: None,
            content_type: None,
            properties: vec![("enabled".into(), "not-a-boolean".into())],
            property_types: [(
                "enabled".into(),
                sift_mgmt::CorrelationPropertyType::Boolean,
            )]
            .into_iter()
            .collect(),
        };
        for (original, replacement) in [
            (invalid.clone(), properties("1=1")),
            (properties("1=1"), invalid),
        ] {
            let result = run(replace(
                original,
                replacement,
                || -> std::future::Ready<Result<(), MgmtError>> {
                    panic!("invalid rule must not be deleted")
                },
                |_| -> std::future::Ready<Result<RuleInfo, MgmtError>> {
                    panic!("invalid rule must not be created")
                },
            ));
            assert!(
                result
                    .expect_err("invalid value")
                    .message
                    .contains("boolean")
            );
        }
    }

    #[test]
    fn failed_deletion_never_attempts_creation() {
        let result = run(replace(
            properties("1=1"),
            properties("priority > 3"),
            || ready(Err(rejected())),
            |_| -> std::future::Ready<Result<RuleInfo, MgmtError>> {
                panic!("creation must not follow failed deletion")
            },
        ));
        assert!(
            result
                .expect_err("deletion rejected")
                .message
                .contains("Replacement was not attempted")
        );
    }

    #[test]
    fn successful_replacement_does_not_restore() {
        let mut calls = 0;
        let replacement = properties("priority > 3");
        let result = run(replace(
            properties("1=1"),
            replacement.clone(),
            || ready(Ok(())),
            |properties| {
                calls += 1;
                ready(Ok(RuleInfo {
                    properties,
                    created_at: None,
                }))
            },
        ));
        assert_eq!(result.expect("replaced").properties, replacement);
        assert_eq!(calls, 1);
    }

    #[test]
    fn rejected_replacement_restores_the_exact_original() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&calls);
        let original = properties("1=1");
        let replacement = properties("invalid SQL");
        let result = run(replace(
            original.clone(),
            replacement.clone(),
            || ready(Ok(())),
            move |properties| {
                recorded.borrow_mut().push(properties.clone());
                if recorded.borrow().len() == 1 {
                    ready(Err(rejected()))
                } else {
                    ready(Ok(RuleInfo {
                        properties,
                        created_at: None,
                    }))
                }
            },
        ));
        assert_eq!(*calls.borrow(), vec![replacement, original]);
        assert!(
            result
                .expect_err("replacement rejected")
                .message
                .contains("original rule was restored")
        );
    }

    #[test]
    fn restoration_failure_reports_both_failures() {
        let result = run(replace(
            properties("1=1"),
            properties("invalid SQL"),
            || ready(Ok(())),
            |_| ready(Err(rejected())),
        ));
        assert!(
            result
                .expect_err("restoration rejected")
                .message
                .contains("Restoring the original rule also failed")
        );
    }
}
