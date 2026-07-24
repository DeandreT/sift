//! Secret material handling: an in-memory wrapper that never prints its
//! contents, plus a [`SecretStore`] abstraction over OS-native credential
//! storage (Windows Credential Manager, macOS Keychain, Linux Secret Service).

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use uuid::Uuid;
use zeroize::Zeroizing;

/// A secret value that is zeroed on drop and redacted in `Debug` output.
///
/// Deliberately does not implement `Serialize` so secret material cannot be
/// written to config files by accident.
#[derive(Clone, Default)]
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    /// Access the secret material. Keep the borrow as short-lived as possible.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self::new(value.to_owned())
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

/// What kind of secret a [`SecretRef`] points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    ConnectionString,
    SasKey,
    ClientSecret,
}

impl SecretKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ConnectionString => "connection_string",
            Self::SasKey => "sas_key",
            Self::ClientSecret => "client_secret",
        }
    }
}

/// Identifies one secret belonging to one namespace profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SecretRef {
    pub profile_id: Uuid,
    pub kind: SecretKind,
}

impl SecretRef {
    #[must_use]
    pub fn new(profile_id: Uuid, kind: SecretKind) -> Self {
        Self { profile_id, kind }
    }

    /// The account name used in the OS credential store.
    fn account(&self) -> String {
        format!("{}:{}", self.profile_id, self.kind.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("secret store error: {0}")]
    Store(String),
}

/// Abstraction over secret persistence so the rest of the app never touches
/// key material storage directly.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &SecretRef) -> Result<Option<SecretString>, SecretError>;
    fn set(&self, key: &SecretRef, value: &SecretString) -> Result<(), SecretError>;
    fn delete(&self, key: &SecretRef) -> Result<(), SecretError>;
    /// Human-readable backend name, shown in the options dialog.
    fn backend_name(&self) -> &'static str;
}

const KEYRING_SERVICE: &str = "sift";

/// OS-native credential storage via the `keyring` crate: Windows Credential
/// Manager (DPAPI-backed), macOS Keychain, or the Secret Service on Linux.
#[derive(Debug, Default)]
pub struct KeyringStore;

impl KeyringStore {
    fn entry(key: &SecretRef) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(KEYRING_SERVICE, &key.account())
            .map_err(|e| SecretError::Store(e.to_string()))
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &SecretRef) -> Result<Option<SecretString>, SecretError> {
        match Self::entry(key)?.get_password() {
            Ok(value) => Ok(Some(SecretString::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError::Store(e.to_string())),
        }
    }

    fn set(&self, key: &SecretRef, value: &SecretString) -> Result<(), SecretError> {
        Self::entry(key)?
            .set_password(value.expose())
            .map_err(|e| SecretError::Store(e.to_string()))
    }

    fn delete(&self, key: &SecretRef) -> Result<(), SecretError> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError::Store(e.to_string())),
        }
    }

    fn backend_name(&self) -> &'static str {
        if cfg!(windows) {
            "Windows Credential Manager"
        } else if cfg!(target_os = "macos") {
            "macOS Keychain"
        } else {
            "Secret Service"
        }
    }
}

/// Session-only fallback used when no OS credential store is available
/// (e.g. headless Linux without a D-Bus session). Secrets entered by the user
/// live only in memory and are lost on exit.
#[derive(Debug, Default)]
pub struct EphemeralStore {
    entries: Mutex<HashMap<String, SecretString>>,
}

impl SecretStore for EphemeralStore {
    fn get(&self, key: &SecretRef) -> Result<Option<SecretString>, SecretError> {
        Ok(self.lock().get(&key.account()).cloned())
    }

    fn set(&self, key: &SecretRef, value: &SecretString) -> Result<(), SecretError> {
        self.lock().insert(key.account(), value.clone());
        Ok(())
    }

    fn delete(&self, key: &SecretRef) -> Result<(), SecretError> {
        self.lock().remove(&key.account());
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "in-memory (session only)"
    }
}

impl EphemeralStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, SecretString>> {
        // A poisoned lock only means another thread panicked mid-insert;
        // the map itself is still usable.
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Probe the OS credential store with a canary entry and fall back to the
/// in-memory store when it is unavailable.
#[must_use]
pub fn open_default_store() -> Box<dyn SecretStore> {
    let store = KeyringStore;
    let canary = SecretRef::new(Uuid::nil(), SecretKind::ClientSecret);
    let probe = store
        .set(&canary, &SecretString::from("sift-canary"))
        .and_then(|()| store.get(&canary))
        .and_then(|_| store.delete(&canary));
    match probe {
        Ok(()) => Box::new(store),
        Err(e) => {
            tracing::warn!(
                "OS credential store unavailable ({e}); secrets will not persist across sessions"
            );
            Box::new(EphemeralStore::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_string_debug_is_redacted() {
        let secret = SecretString::from("super-secret");
        assert_eq!(format!("{secret:?}"), "SecretString(<redacted>)");
    }

    #[test]
    fn ephemeral_store_round_trips() {
        let store = EphemeralStore::default();
        let key = SecretRef::new(Uuid::new_v4(), SecretKind::ConnectionString);

        assert!(store.get(&key).unwrap().is_none());
        store.set(&key, &SecretString::from("value")).unwrap();
        assert_eq!(store.get(&key).unwrap().unwrap().expose(), "value");
        store.delete(&key).unwrap();
        assert!(store.get(&key).unwrap().is_none());
    }

    #[test]
    fn secret_ref_account_is_stable() {
        let id = Uuid::nil();
        let key = SecretRef::new(id, SecretKind::ConnectionString);
        assert_eq!(
            key.account(),
            "00000000-0000-0000-0000-000000000000:connection_string"
        );
    }
}
