//! Service Bus connection-string parsing.
//!
//! Ports the semantics of the reference app's `ServiceBusNamespace.cs`:
//! parameters split on `;`, keys matched case-insensitively, values taken
//! verbatim after the *first* `=` (keys are base64 and contain `=` padding),
//! and endpoints without a scheme prefixed with `sb://`. The legacy
//! on-premises "Service Bus for Windows Server" parameters are recognized but
//! rejected with a warning rather than supported.

use url::Url;

use crate::secrets::SecretString;

/// AMQP transport variant. The legacy `NetMessaging` transport is WCF-bound
/// and unsupported; it parses as AMQP with a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportType {
    #[default]
    AmqpTcp,
    AmqpWebSockets,
}

/// Credential material carried by a connection string.
#[derive(Debug, Clone)]
pub enum Credential {
    /// `SharedAccessKeyName` + `SharedAccessKey`: sift mints SAS tokens itself.
    SasKey { key_name: String, key: SecretString },
    /// `SharedAccessSignature=...`: a pre-minted token used as-is.
    SasToken(SecretString),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConnectionStringError {
    #[error("the connection string is empty")]
    Empty,
    #[error("the connection string has no Endpoint parameter")]
    MissingEndpoint,
    #[error("the endpoint '{0}' is not a valid URI")]
    InvalidEndpoint(String),
    #[error(
        "the connection string has no credentials; expected SharedAccessKeyName and \
         SharedAccessKey, or SharedAccessSignature"
    )]
    MissingCredentials,
    #[error("SharedAccessKeyName and SharedAccessKey must both be present")]
    IncompleteSasKey,
}

/// A parsed Service Bus (or Event Hubs / Relay / Notification Hubs) namespace
/// connection string.
#[derive(Debug, Clone)]
pub struct NamespaceConnection {
    /// The `sb://` endpoint, normalized.
    pub endpoint: Url,
    /// Host name, e.g. `contoso.servicebus.windows.net`.
    pub fully_qualified_namespace: String,
    /// First host label, e.g. `contoso`.
    pub namespace: String,
    pub credential: Credential,
    /// Present when the connection string is scoped to a single entity.
    pub entity_path: Option<String>,
    pub transport: TransportType,
    /// `UseDevelopmentEmulator=true` (local Service Bus emulator).
    pub use_development_emulator: bool,
    /// Non-fatal issues found while parsing (unknown or legacy parameters).
    pub warnings: Vec<String>,
    /// The original string, needed verbatim for AMQP client crates.
    raw: SecretString,
}

impl NamespaceConnection {
    /// Parse a connection string of the form
    /// `Endpoint=sb://ns.servicebus.windows.net/;SharedAccessKeyName=...;SharedAccessKey=...`.
    pub fn parse(input: &str) -> Result<Self, ConnectionStringError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(ConnectionStringError::Empty);
        }

        let mut endpoint_raw: Option<String> = None;
        let mut key_name: Option<String> = None;
        let mut key: Option<String> = None;
        let mut sas_token: Option<String> = None;
        let mut entity_path: Option<String> = None;
        let mut transport = TransportType::default();
        let mut use_development_emulator = false;
        let mut warnings = Vec::new();

        for part in trimmed.split(';') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            // Split at the first '=' only: values are frequently base64 and
            // end in '=' padding.
            let Some((raw_key, value)) = part.split_once('=') else {
                warnings.push(format!("ignoring malformed parameter '{part}'"));
                continue;
            };
            let value = value.trim();
            match raw_key.trim().to_ascii_lowercase().as_str() {
                "endpoint" => endpoint_raw = Some(value.to_owned()),
                "sharedaccesskeyname" => key_name = Some(value.to_owned()),
                "sharedaccesskey" => key = Some(value.to_owned()),
                "sharedaccesssignature" => sas_token = Some(value.to_owned()),
                "entitypath" => entity_path = Some(value.to_owned()),
                "transporttype" => {
                    transport = match value.to_ascii_lowercase().as_str() {
                        "amqp" => TransportType::AmqpTcp,
                        "amqpwebsockets" => TransportType::AmqpWebSockets,
                        // Match the reference app's Enum.TryParse leniency:
                        // legacy (NetMessaging) or unknown transports fall back
                        // to AMQP with a warning instead of failing the parse.
                        other => {
                            warnings.push(format!(
                                "transport type '{other}' is not supported; using AMQP"
                            ));
                            TransportType::AmqpTcp
                        }
                    };
                }
                "usedevelopmentemulator" => {
                    use_development_emulator = value.eq_ignore_ascii_case("true");
                }
                // Parameters for the long-dead "Service Bus for Windows Server".
                "stsendpoint" | "runtimeport" | "managementport" | "windowsdomain"
                | "windowsusername" | "windowspassword" => {
                    warnings.push(format!(
                        "ignoring legacy on-premises parameter '{}'",
                        raw_key.trim()
                    ));
                }
                other => warnings.push(format!("ignoring unknown parameter '{other}'")),
            }
        }

        let endpoint_raw = endpoint_raw.ok_or(ConnectionStringError::MissingEndpoint)?;
        // Match the reference app: endpoints without a scheme get "sb://".
        let endpoint_raw = if endpoint_raw.contains("://") {
            endpoint_raw
        } else {
            format!("sb://{endpoint_raw}")
        };
        let endpoint = Url::parse(&endpoint_raw)
            .map_err(|_| ConnectionStringError::InvalidEndpoint(endpoint_raw.clone()))?;
        let fully_qualified_namespace = endpoint
            .host_str()
            .ok_or_else(|| ConnectionStringError::InvalidEndpoint(endpoint_raw.clone()))?
            .to_owned();
        let namespace = fully_qualified_namespace
            .split('.')
            .next()
            .unwrap_or(&fully_qualified_namespace)
            .to_owned();

        let credential = match (key_name, key, sas_token) {
            (Some(name), Some(key), _) if !name.is_empty() && !key.is_empty() => {
                Credential::SasKey {
                    key_name: name,
                    key: SecretString::new(key),
                }
            }
            (Some(_), _, None) | (_, Some(_), None) => {
                return Err(ConnectionStringError::IncompleteSasKey);
            }
            (_, _, Some(token)) if !token.is_empty() => {
                Credential::SasToken(SecretString::new(token))
            }
            _ => return Err(ConnectionStringError::MissingCredentials),
        };

        Ok(Self {
            endpoint,
            fully_qualified_namespace,
            namespace,
            credential,
            entity_path,
            transport,
            use_development_emulator,
            warnings,
            raw: SecretString::from(trimmed),
        })
    }

    /// The original connection string, for AMQP clients that consume it verbatim.
    #[must_use]
    pub fn raw(&self) -> &SecretString {
        &self.raw
    }

    /// Base URL for the ATOM management REST API (`sb://` → `https://`).
    ///
    /// The local development emulator serves its management API over plain
    /// HTTP; real namespaces are always HTTPS.
    #[must_use]
    pub fn https_base(&self) -> Url {
        let scheme = if self.use_development_emulator {
            "http"
        } else {
            "https"
        };
        let host = &self.fully_qualified_namespace;
        let base = match self.endpoint.port() {
            Some(port) => format!("{scheme}://{host}:{port}/"),
            None => format!("{scheme}://{host}/"),
        };
        Url::parse(&base).expect("scheme and validated host always form a valid URL")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = "Endpoint=sb://contoso.servicebus.windows.net/;\
        SharedAccessKeyName=RootManageSharedAccessKey;SharedAccessKey=abc123def456==";

    #[test]
    fn parses_standard_sas_connection_string() {
        let conn = NamespaceConnection::parse(FULL).unwrap();
        assert_eq!(
            conn.fully_qualified_namespace,
            "contoso.servicebus.windows.net"
        );
        assert_eq!(conn.namespace, "contoso");
        assert_eq!(conn.transport, TransportType::AmqpTcp);
        assert!(conn.entity_path.is_none());
        assert!(conn.warnings.is_empty());
        let Credential::SasKey { key_name, key } = &conn.credential else {
            panic!("expected SasKey credential");
        };
        assert_eq!(key_name, "RootManageSharedAccessKey");
        // The '=' padding after the first '=' must be preserved.
        assert_eq!(key.expose(), "abc123def456==");
    }

    #[test]
    fn keys_are_case_insensitive_and_entity_path_is_kept() {
        let conn = NamespaceConnection::parse(
            "ENDPOINT=sb://c.servicebus.windows.net;sharedaccesskeyname=n;\
             SHAREDACCESSKEY=k;EntityPath=orders;TransportType=AmqpWebSockets",
        )
        .unwrap();
        assert_eq!(conn.entity_path.as_deref(), Some("orders"));
        assert_eq!(conn.transport, TransportType::AmqpWebSockets);
    }

    #[test]
    fn missing_scheme_gets_sb_prefix() {
        let conn = NamespaceConnection::parse(
            "Endpoint=contoso.servicebus.windows.net;SharedAccessKeyName=n;SharedAccessKey=k",
        )
        .unwrap();
        assert_eq!(conn.endpoint.scheme(), "sb");
        assert_eq!(conn.namespace, "contoso");
    }

    #[test]
    fn presigned_sas_token_is_accepted() {
        let conn = NamespaceConnection::parse(
            "Endpoint=sb://c.servicebus.windows.net/;\
             SharedAccessSignature=SharedAccessSignature sr=x&sig=y&se=1&skn=n",
        )
        .unwrap();
        assert!(matches!(conn.credential, Credential::SasToken(_)));
    }

    #[test]
    fn net_messaging_transport_falls_back_to_amqp_with_warning() {
        let conn = NamespaceConnection::parse(
            "Endpoint=sb://c.servicebus.windows.net/;SharedAccessKeyName=n;\
             SharedAccessKey=k;TransportType=NetMessaging",
        )
        .unwrap();
        assert_eq!(conn.transport, TransportType::AmqpTcp);
        assert_eq!(conn.warnings.len(), 1);
        assert!(conn.warnings[0].contains("netmessaging"));
    }

    #[test]
    fn missing_endpoint_is_rejected() {
        let err =
            NamespaceConnection::parse("SharedAccessKeyName=n;SharedAccessKey=k").unwrap_err();
        assert_eq!(err, ConnectionStringError::MissingEndpoint);
    }

    #[test]
    fn key_name_without_key_is_rejected() {
        let err = NamespaceConnection::parse(
            "Endpoint=sb://c.servicebus.windows.net/;SharedAccessKeyName=n",
        )
        .unwrap_err();
        assert_eq!(err, ConnectionStringError::IncompleteSasKey);
    }

    #[test]
    fn no_credentials_is_rejected() {
        let err =
            NamespaceConnection::parse("Endpoint=sb://c.servicebus.windows.net/").unwrap_err();
        assert_eq!(err, ConnectionStringError::MissingCredentials);
    }

    #[test]
    fn legacy_and_unknown_parameters_produce_warnings() {
        let conn = NamespaceConnection::parse(
            "Endpoint=sb://c.servicebus.windows.net/;SharedAccessKeyName=n;SharedAccessKey=k;\
             StsEndpoint=https://x;RuntimePort=1;Frobnicate=y",
        )
        .unwrap();
        assert_eq!(conn.warnings.len(), 3);
    }

    #[test]
    fn https_base_maps_sb_to_https() {
        let conn = NamespaceConnection::parse(FULL).unwrap();
        assert_eq!(
            conn.https_base().as_str(),
            "https://contoso.servicebus.windows.net/"
        );
    }

    #[test]
    fn emulator_uses_http_management_base() {
        let conn = NamespaceConnection::parse(
            "Endpoint=sb://localhost;SharedAccessKeyName=n;SharedAccessKey=k;\
             UseDevelopmentEmulator=true",
        )
        .unwrap();
        assert!(conn.use_development_emulator);
        assert_eq!(conn.https_base().as_str(), "http://localhost/");
    }
}
