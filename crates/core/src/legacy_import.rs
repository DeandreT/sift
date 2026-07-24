//! Import saved namespaces from a legacy .NET explorer-tool config file.
//!
//! Older tools store connection strings (in plaintext) in a
//! `serviceBusNamespaces` dictionary section of an XML `.config` file.
//! Importing moves the key material into the OS secret store and creates a
//! sift profile per entry.

use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::config::{AppConfig, NamespaceProfile};
use crate::connection::NamespaceConnection;
use crate::secrets::{SecretKind, SecretRef, SecretStore, SecretString};

#[derive(Debug, thiserror::Error)]
pub enum LegacyImportError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a valid namespaces config: {detail}")]
    Parse { path: PathBuf, detail: String },
    #[error("no <serviceBusNamespaces> entries found in {path}")]
    NoNamespaces { path: PathBuf },
}

/// Outcome of one import run.
#[derive(Debug, Default)]
pub struct ImportReport {
    /// Profile names created.
    pub imported: Vec<String>,
    /// Profile names whose stored secret/endpoint was refreshed.
    pub updated: Vec<String>,
    /// `(name, reason)` for entries that could not be imported.
    pub skipped: Vec<(String, String)>,
    /// Non-fatal notes (e.g. legacy transport stripped).
    pub warnings: Vec<String>,
}

impl std::fmt::Display for ImportReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} imported, {} updated, {} skipped",
            self.imported.len(),
            self.updated.len(),
            self.skipped.len()
        )
    }
}

/// Read `path` and import every `serviceBusNamespaces` entry into `config`
/// and `secrets`. Idempotent: entries whose name already exists as a profile
/// have their secret and endpoint refreshed instead of being duplicated.
pub fn import_from_file(
    path: &Path,
    config: &mut AppConfig,
    secrets: &dyn SecretStore,
) -> Result<ImportReport, LegacyImportError> {
    let xml = std::fs::read_to_string(path).map_err(|source| LegacyImportError::Io {
        path: path.to_owned(),
        source,
    })?;
    let entries = parse_legacy_config(&xml).map_err(|detail| LegacyImportError::Parse {
        path: path.to_owned(),
        detail,
    })?;
    if entries.is_empty() {
        return Err(LegacyImportError::NoNamespaces {
            path: path.to_owned(),
        });
    }
    Ok(import_entries(entries, config, secrets))
}

/// Extract `(name, connection string)` pairs from the
/// `<serviceBusNamespaces><add key="…" value="…"/></serviceBusNamespaces>`
/// section of a .NET-style config file.
pub fn parse_legacy_config(xml: &str) -> Result<Vec<(String, SecretString)>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut entries = Vec::new();
    let mut in_section = false;

    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(e) if e.local_name().as_ref() == b"serviceBusNamespaces" => {
                in_section = true;
            }
            Event::End(e) if e.local_name().as_ref() == b"serviceBusNamespaces" => {
                in_section = false;
            }
            Event::Empty(e) | Event::Start(e)
                if in_section && e.local_name().as_ref() == b"add" =>
            {
                let mut key = None;
                let mut value = None;
                for attr in e.attributes().flatten() {
                    let attr_value = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|e| e.to_string())?
                        .into_owned();
                    match attr.key.local_name().as_ref() {
                        b"key" => key = Some(attr_value),
                        b"value" => value = Some(attr_value),
                        _ => {}
                    }
                }
                if let (Some(key), Some(value)) = (key, value) {
                    entries.push((key, SecretString::new(value)));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(entries)
}

fn import_entries(
    entries: Vec<(String, SecretString)>,
    config: &mut AppConfig,
    secrets: &dyn SecretStore,
) -> ImportReport {
    let mut report = ImportReport::default();

    for (name, raw) in entries {
        let (sanitized, note) = sanitize_connection_string(raw.expose());
        if let Some(note) = note {
            report.warnings.push(format!("{name}: {note}"));
        }
        let secret = SecretString::new(sanitized);

        let conn = match NamespaceConnection::parse(secret.expose()) {
            Ok(conn) => conn,
            Err(e) => {
                report.skipped.push((name, e.to_string()));
                continue;
            }
        };

        let existing = config.profiles.iter().find(|p| p.name == name).cloned();
        let is_update = existing.is_some();
        let mut profile =
            existing.unwrap_or_else(|| NamespaceProfile::new_connection_string(name.clone()));
        profile.endpoint = Some(conn.endpoint.clone());
        profile.transport = conn.transport;

        if let Err(e) = secrets.set(
            &SecretRef::new(profile.id, SecretKind::ConnectionString),
            &secret,
        ) {
            report.skipped.push((name, e.to_string()));
            continue;
        }
        config.upsert_profile(profile);
        if is_update {
            report.updated.push(name);
        } else {
            report.imported.push(name);
        }
    }
    report
}

/// Drop connection-string parameters that only made sense for the legacy .NET
/// SDK (`TransportType=NetMessaging`), so the stored string can be handed to
/// AMQP clients verbatim.
fn sanitize_connection_string(raw: &str) -> (String, Option<String>) {
    let mut removed = false;
    let kept: Vec<&str> = raw
        .split(';')
        .filter(|part| {
            let is_net_messaging = part.split_once('=').is_some_and(|(key, value)| {
                key.trim().eq_ignore_ascii_case("transporttype")
                    && value.trim().eq_ignore_ascii_case("netmessaging")
            });
            removed |= is_net_messaging;
            !is_net_messaging
        })
        .collect();
    let note = removed.then(|| "removed legacy 'TransportType=NetMessaging'".to_owned());
    (kept.join(";"), note)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::EphemeralStore;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<configuration>
  <configSections>
    <section name="serviceBusNamespaces" type="System.Configuration.DictionarySectionHandler, System" />
  </configSections>
  <serviceBusNamespaces>
    <add key="SB Dev" value="Endpoint=sb://dev.servicebus.windows.net/;SharedAccessKeyName=RootManageSharedAccessKey;SharedAccessKey=devkey==;TransportType=NetMessaging" />
    <add key="SB Prod" value="Endpoint=sb://prod.servicebus.windows.net/;SharedAccessKeyName=RootManageSharedAccessKey;SharedAccessKey=prodkey==" />
    <add key="Broken" value="ThisIsNotAConnectionString" />
  </serviceBusNamespaces>
</configuration>"#;

    #[test]
    fn parses_namespace_entries() {
        let entries = parse_legacy_config(SAMPLE).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].0, "SB Dev");
        assert!(entries[1].1.expose().contains("prodkey=="));
    }

    #[test]
    fn imports_valid_entries_and_reports_broken_ones() {
        let mut config = AppConfig::default();
        let secrets = EphemeralStore::default();

        let entries = parse_legacy_config(SAMPLE).unwrap();
        let report = import_entries(entries, &mut config, &secrets);

        assert_eq!(report.imported, vec!["SB Dev", "SB Prod"]);
        assert!(report.updated.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].0, "Broken");
        assert_eq!(config.profiles.len(), 2);

        // NetMessaging is stripped from the stored secret.
        let dev = config.profiles.iter().find(|p| p.name == "SB Dev").unwrap();
        let stored = secrets
            .get(&SecretRef::new(dev.id, SecretKind::ConnectionString))
            .unwrap()
            .unwrap();
        assert!(
            !stored
                .expose()
                .to_ascii_lowercase()
                .contains("netmessaging")
        );
        assert!(stored.expose().contains("SharedAccessKey=devkey=="));
        assert_eq!(report.warnings.len(), 1);
    }

    #[test]
    fn reimport_updates_instead_of_duplicating() {
        let mut config = AppConfig::default();
        let secrets = EphemeralStore::default();

        let entries = parse_legacy_config(SAMPLE).unwrap();
        import_entries(entries, &mut config, &secrets);
        let report = import_entries(parse_legacy_config(SAMPLE).unwrap(), &mut config, &secrets);

        assert!(report.imported.is_empty());
        assert_eq!(report.updated, vec!["SB Dev", "SB Prod"]);
        assert_eq!(config.profiles.len(), 2);
    }
}
