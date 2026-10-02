//! Shared Entra credentials for management and AMQP. Azure CLI owns persisted
//! sign-in material; Sift retains only short-lived tokens in memory.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use azure_core::credentials::{AccessToken, TokenCredential, TokenRequestOptions};
use azure_core::error::ErrorKind;
use azure_identity::{AzureCliCredential, AzureCliCredentialOptions};
use tokio::sync::Mutex;

pub const SERVICE_BUS_SCOPE: &str = "https://servicebus.azure.net/.default";
pub const SIGN_IN_HELP: &str = "Microsoft Entra ID sign-in is required. Install Azure CLI 2.54 or newer, then choose Sign in in the connection dialog.";

#[derive(Debug)]
struct CliExecutor {
    timeout: Duration,
}

#[async_trait::async_trait]
impl azure_core::process::Executor for CliExecutor {
    async fn run(&self, program: &OsStr, args: &[&OsStr]) -> std::io::Result<Output> {
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        tokio::time::timeout(self.timeout, command.output())
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Azure credential request timed out",
                )
            })?
    }
}

/// Reject tenant values that cannot be passed safely to Azure CLI.
pub fn validate_tenant(tenant: Option<&str>) -> Result<(), String> {
    if tenant.is_some_and(|value| {
        value.is_empty()
            || !value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    }) {
        return Err("Enter a tenant ID or tenant domain.".into());
    }
    Ok(())
}

pub fn azure_cli(tenant_id: Option<String>) -> azure_core::Result<Arc<dyn TokenCredential>> {
    validate_tenant(tenant_id.as_deref())
        .map_err(|message| azure_core::Error::message(ErrorKind::Credential, message))?;
    let mut options = AzureCliCredentialOptions::default();
    options.tenant_id = tenant_id;
    options.executor = Some(Arc::new(CliExecutor {
        timeout: Duration::from_secs(30),
    }));
    let source = AzureCliCredential::new(Some(options))?;
    Ok(Arc::new(CachedCredential::new(source)))
}

/// Serialize refreshes and reuse a token until two minutes before expiration.
/// Errors from external tools are replaced with a safe sign-in instruction.
pub struct CachedCredential {
    source: Arc<dyn TokenCredential>,
    tokens: Mutex<HashMap<Vec<String>, AccessToken>>,
}

impl std::fmt::Debug for CachedCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CachedCredential(<redacted>)")
    }
}

impl CachedCredential {
    #[must_use]
    pub fn new(source: Arc<dyn TokenCredential>) -> Self {
        Self {
            source,
            tokens: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl TokenCredential for CachedCredential {
    async fn get_token(
        &self,
        scopes: &[&str],
        options: Option<TokenRequestOptions>,
    ) -> azure_core::Result<AccessToken> {
        let key: Vec<String> = scopes.iter().map(|scope| (*scope).to_owned()).collect();
        let mut tokens = self.tokens.lock().await;
        let threshold = time::OffsetDateTime::now_utc() + time::Duration::minutes(2);
        if let Some(token) = tokens
            .get(&key)
            .filter(|token| token.expires_on > threshold)
        {
            return Ok(token.clone());
        }
        let token = self
            .source
            .get_token(scopes, options)
            .await
            .map_err(|_| azure_core::Error::message(ErrorKind::Credential, SIGN_IN_HELP))?;
        if token.expires_on <= time::OffsetDateTime::now_utc() || token.token.secret().is_empty() {
            return Err(azure_core::Error::message(
                ErrorKind::Credential,
                SIGN_IN_HELP,
            ));
        }
        tokens.insert(key, token.clone());
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(unix)]
    #[tokio::test]
    async fn hung_credential_process_is_bounded() {
        use azure_core::process::Executor;
        let executor = CliExecutor {
            timeout: Duration::from_millis(25),
        };
        let result = executor.run(OsStr::new("sleep"), &[OsStr::new("5")]).await;
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    }

    #[derive(Debug)]
    struct FakeCredential {
        calls: AtomicUsize,
        lifetime: time::Duration,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl TokenCredential for FakeCredential {
        async fn get_token(
            &self,
            _: &[&str],
            _: Option<TokenRequestOptions>,
        ) -> azure_core::Result<AccessToken> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(azure_core::Error::message(
                    ErrorKind::Credential,
                    "secret-tool-output",
                ));
            }
            Ok(AccessToken::new(
                "secret-token",
                time::OffsetDateTime::now_utc() + self.lifetime,
            ))
        }
    }

    #[tokio::test]
    async fn shares_valid_tokens_and_refreshes_near_expiry() {
        for (lifetime, expected_calls) in [
            (time::Duration::hours(1), 1),
            (time::Duration::seconds(60), 2),
        ] {
            let source = Arc::new(FakeCredential {
                calls: AtomicUsize::new(0),
                lifetime,
                fail: false,
            });
            let cache = CachedCredential::new(source.clone());
            cache.get_token(&[SERVICE_BUS_SCOPE], None).await.unwrap();
            cache.get_token(&[SERVICE_BUS_SCOPE], None).await.unwrap();
            assert_eq!(source.calls.load(Ordering::SeqCst), expected_calls);
            assert!(!format!("{cache:?}").contains("secret-token"));
        }
    }

    #[tokio::test]
    async fn does_not_leak_external_tool_output_on_failure() {
        let cache = CachedCredential::new(Arc::new(FakeCredential {
            calls: AtomicUsize::new(0),
            lifetime: time::Duration::hours(1),
            fail: true,
        }));
        let error = cache
            .get_token(&[SERVICE_BUS_SCOPE], None)
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("secret-tool-output"));
        assert!(error.to_string().contains("Sign in"));
    }

    #[test]
    fn validates_tenant_arguments() {
        assert!(validate_tenant(None).is_ok());
        assert!(validate_tenant(Some("contoso.onmicrosoft.com")).is_ok());
        for value in ["", "-tenant && malicious", "tenant/id", "tenant\nvalue"] {
            assert!(validate_tenant(Some(value)).is_err());
        }
    }
}
