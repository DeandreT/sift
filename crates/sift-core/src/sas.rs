//! Shared Access Signature token generation.
//!
//! The algorithm matches .NET's `SharedAccessSignatureTokenProvider`:
//!
//! ```text
//! sr  = percent_encode(resource_uri)          // RFC 3986, unreserved chars kept
//! se  = unix_seconds(now + ttl)
//! sig = base64(HMAC-SHA256(key, sr + "\n" + se))
//! token = "SharedAccessSignature sr={sr}&sig={enc(sig)}&se={se}&skn={key_name}"
//! ```
//!
//! The `sr` embedded in the token must be byte-identical to the string that
//! was signed, so one encoder is used for both.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine as _;
use hmac::{Hmac, Mac};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::Sha256;
use time::OffsetDateTime;

use crate::connection::{Credential, NamespaceConnection};
use crate::secrets::SecretString;

/// RFC 3986 strict encoding: everything except unreserved characters.
const STRICT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

const DEFAULT_TTL: Duration = Duration::from_mins(20);
/// Refresh a cached token once less than this fraction of its life remains.
const REFRESH_FRACTION: f64 = 0.2;

/// A minted SAS token and its expiry.
#[derive(Debug, Clone)]
pub struct SasToken {
    pub value: String,
    pub expires_at: OffsetDateTime,
}

/// Compute a SAS token for `resource_uri` expiring at `expires_at`.
#[must_use]
pub fn generate(
    resource_uri: &str,
    key_name: &str,
    key: &SecretString,
    expires_at: OffsetDateTime,
) -> SasToken {
    let sr = utf8_percent_encode(resource_uri, STRICT).to_string();
    let se = expires_at.unix_timestamp();
    let string_to_sign = format!("{sr}\n{se}");

    let mut mac = Hmac::<Sha256>::new_from_slice(key.expose().as_bytes())
        .expect("HMAC accepts keys of any length");
    mac.update(string_to_sign.as_bytes());
    let signature = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    let sig = utf8_percent_encode(&signature, STRICT).to_string();

    SasToken {
        value: format!("SharedAccessSignature sr={sr}&sig={sig}&se={se}&skn={key_name}"),
        expires_at,
    }
}

/// Mints and caches SAS tokens per resource URI.
#[derive(Debug)]
pub struct SasTokenProvider {
    key_name: String,
    key: SecretString,
    ttl: Duration,
    cache: Mutex<HashMap<String, SasToken>>,
}

impl SasTokenProvider {
    #[must_use]
    pub fn new(key_name: String, key: SecretString) -> Self {
        Self {
            key_name,
            key,
            ttl: DEFAULT_TTL,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Build a provider from a parsed connection string. Returns `None` for
    /// pre-minted `SharedAccessSignature=` credentials, which cannot mint new
    /// tokens.
    #[must_use]
    pub fn from_connection(conn: &NamespaceConnection) -> Option<Self> {
        match &conn.credential {
            Credential::SasKey { key_name, key } => Some(Self::new(key_name.clone(), key.clone())),
            Credential::SasToken(_) => None,
        }
    }

    /// Get a token for `resource_uri`, reusing a cached one while it still has
    /// more than 20% of its lifetime left.
    #[must_use]
    pub fn token_for(&self, resource_uri: &str) -> SasToken {
        let now = OffsetDateTime::now_utc();
        let min_remaining = self.ttl.mul_f64(REFRESH_FRACTION);

        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(token) = cache.get(resource_uri)
            && token.expires_at - now >= min_remaining
        {
            return token.clone();
        }
        let token = generate(resource_uri, &self.key_name, &self.key, now + self.ttl);
        cache.insert(resource_uri.to_owned(), token.clone());
        token
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn key() -> SecretString {
        SecretString::from("dGVzdC1rZXktbWF0ZXJpYWw=")
    }

    #[test]
    fn token_has_expected_shape_and_encoding() {
        let token = generate(
            "https://contoso.servicebus.windows.net/myqueue",
            "RootManageSharedAccessKey",
            &key(),
            datetime!(2026-01-01 00:00:00 UTC),
        );
        // The resource URI must be percent-encoded inside the token.
        assert!(token.value.starts_with(
            "SharedAccessSignature sr=https%3A%2F%2Fcontoso.servicebus.windows.net%2Fmyqueue&sig="
        ));
        assert!(
            token
                .value
                .ends_with("&se=1767225600&skn=RootManageSharedAccessKey")
        );
    }

    #[test]
    fn signing_is_deterministic() {
        let at = datetime!(2026-01-01 00:00:00 UTC);
        let a = generate("sb://ns/q", "n", &key(), at);
        let b = generate("sb://ns/q", "n", &key(), at);
        assert_eq!(a.value, b.value);
    }

    #[test]
    fn different_resources_produce_different_signatures() {
        let at = datetime!(2026-01-01 00:00:00 UTC);
        let a = generate("sb://ns/q1", "n", &key(), at);
        let b = generate("sb://ns/q2", "n", &key(), at);
        assert_ne!(a.value, b.value);
    }

    #[test]
    fn provider_caches_per_resource() {
        let provider = SasTokenProvider::new("n".into(), key());
        let a = provider.token_for("https://ns/q");
        let b = provider.token_for("https://ns/q");
        assert_eq!(a.value, b.value);
        let c = provider.token_for("https://ns/other");
        assert_ne!(a.value, c.value);
    }
}
