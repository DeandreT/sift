//! Application configuration: a versioned TOML file in the platform config
//! directory holding UI preferences and saved namespace profiles.
//!
//! Profiles never contain secret material — connection strings and keys live
//! in the [`crate::secrets::SecretStore`], keyed by profile id.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use url::Url;
use uuid::Uuid;

use crate::connection::TransportType;

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "DeandreT";
const APPLICATION: &str = "sift";
const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not determine the platform configuration directory")]
    NoConfigDir,
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid configuration: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("failed to serialize configuration: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// How the user authenticates a namespace profile.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthMethod {
    /// SAS connection string, stored in the secret store under
    /// [`crate::secrets::SecretKind::ConnectionString`].
    ConnectionString,
    /// Microsoft Entra ID (Azure AD). Wired up in a later phase.
    AzureAd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },
}

/// A saved namespace connection, minus its secret material.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NamespaceProfile {
    pub id: Uuid,
    pub name: String,
    /// Endpoint for display purposes; the authoritative endpoint comes from
    /// the stored connection string at connect time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<Url>,
    #[serde(default)]
    pub transport: TransportType,
    pub auth: AuthMethod,
}

impl NamespaceProfile {
    #[must_use]
    pub fn new_connection_string(name: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            endpoint: None,
            transport: TransportType::default(),
            auth: AuthMethod::ConnectionString,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub theme: ThemePreference,
    /// Messages fetched per peek page.
    pub peek_batch: u32,
    /// Require typing the entity name to confirm deletes/purges.
    pub confirm_delete_typed_name: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: ThemePreference::default(),
            peek_batch: 100,
            confirm_delete_typed_name: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    pub count: u32,
    pub backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            count: 3,
            backoff_ms: 500,
        }
    }
}

/// Root of `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub schema_version: u32,
    pub ui: UiConfig,
    pub retry: RetryConfig,
    pub profiles: Vec<NamespaceProfile>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            ui: UiConfig::default(),
            retry: RetryConfig::default(),
            profiles: Vec::new(),
        }
    }
}

impl AppConfig {
    /// Platform path of `config.toml`
    /// (e.g. `%APPDATA%\DeandreT\sift\config\config.toml` on Windows,
    /// `~/.config/sift/config.toml` on Linux).
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        let dirs = directories::ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
            .ok_or(ConfigError::NoConfigDir)?;
        Ok(dirs.config_dir().join(CONFIG_FILE))
    }

    /// Load from the default path; a missing file yields the default config.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(&Self::default_path()?)
    }

    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source: Box::new(source),
        })
    }

    /// Save to the default path.
    pub fn save(&self) -> Result<(), ConfigError> {
        self.save_to(&Self::default_path()?)
    }

    /// Atomic save: write to a temp file in the target directory, then rename
    /// over the destination so a crash can never leave a half-written config.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let io_err = |source| ConfigError::Io {
            path: path.to_owned(),
            source,
        };
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir).map_err(io_err)?;

        let text = toml::to_string_pretty(self)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(io_err)?;
        tmp.write_all(text.as_bytes()).map_err(io_err)?;
        tmp.persist(path).map_err(|e| io_err(e.error)).map(|_| ())
    }

    pub fn profile(&self, id: Uuid) -> Option<&NamespaceProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Insert or replace a profile by id.
    pub fn upsert_profile(&mut self, profile: NamespaceProfile) {
        match self.profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => *existing = profile,
            None => self.profiles.push(profile),
        }
    }

    pub fn remove_profile(&mut self, id: Uuid) {
        self.profiles.retain(|p| p.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let config = AppConfig::default();
        assert_eq!(config.schema_version, 1);
        assert_eq!(config.ui.peek_batch, 100);
        assert!(config.ui.confirm_delete_typed_name);
        assert!(config.profiles.is_empty());
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let mut config = AppConfig::default();
        let mut profile = NamespaceProfile::new_connection_string("prod-orders".into());
        profile.endpoint = Some(Url::parse("sb://orders.servicebus.windows.net/").unwrap());
        config.upsert_profile(profile.clone());

        config.save_to(&path).unwrap();
        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded, config);
        assert_eq!(loaded.profile(profile.id), Some(&profile));
    }

    #[test]
    fn missing_file_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = AppConfig::load_from(&dir.path().join("nope.toml")).unwrap();
        assert_eq!(loaded, AppConfig::default());
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "schema_version = 1\nfuture_field = true\n").unwrap();
        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded.schema_version, 1);
    }

    #[test]
    fn upsert_replaces_existing_profile() {
        let mut config = AppConfig::default();
        let mut profile = NamespaceProfile::new_connection_string("a".into());
        config.upsert_profile(profile.clone());
        profile.name = "b".into();
        config.upsert_profile(profile.clone());
        assert_eq!(config.profiles.len(), 1);
        assert_eq!(config.profiles[0].name, "b");
    }
}
