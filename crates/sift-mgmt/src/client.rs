//! The management HTTP client: request signing, api-version handling, and
//! status-code mapping.

use reqwest::header;
use sift_core::connection::{Credential, NamespaceConnection};
use sift_core::sas::SasTokenProvider;
use url::Url;

use crate::atom;
use crate::error::MgmtError;
use crate::model::NamespaceInfo;

pub const API_VERSION: &str = "2021-05";
const USER_AGENT: &str = concat!("sift/", env!("CARGO_PKG_VERSION"));

/// Produces `Authorization` header values for management requests.
///
/// A `Bearer` variant for Microsoft Entra ID credentials is added in Phase 1.
#[derive(Debug)]
pub enum Authorizer {
    /// Mints a SAS token per resource URI from a shared access key.
    Sas(SasTokenProvider),
    /// A pre-minted `SharedAccessSignature ...` token used verbatim.
    StaticSas(String),
}

impl Authorizer {
    #[must_use]
    pub fn from_connection(conn: &NamespaceConnection) -> Self {
        match &conn.credential {
            Credential::SasKey { .. } => Self::Sas(
                SasTokenProvider::from_connection(conn)
                    .expect("SasKey credential always yields a provider"),
            ),
            Credential::SasToken(token) => Self::StaticSas(token.expose().to_owned()),
        }
    }

    fn header_value(&self, resource_uri: &str) -> String {
        match self {
            Self::Sas(provider) => provider.token_for(resource_uri).value,
            Self::StaticSas(token) => token.clone(),
        }
    }
}

/// Client for one namespace's management endpoint.
#[derive(Debug)]
pub struct ManagementClient {
    http: reqwest::Client,
    base: Url,
    authorizer: Authorizer,
}

impl ManagementClient {
    pub fn new(conn: &NamespaceConnection) -> Result<Self, MgmtError> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            base: conn.https_base(),
            authorizer: Authorizer::from_connection(conn),
        })
    }

    /// `GET /$namespaceinfo` — namespace name, type, and SKU. Doubles as the
    /// cheapest way to validate a connection.
    pub async fn get_namespace_info(&self) -> Result<NamespaceInfo, MgmtError> {
        let body = self.get("$namespaceinfo").await?;
        atom::parse_namespace_info(&body)
    }

    fn url_for(&self, path: &str) -> Result<Url, MgmtError> {
        let mut url = self.base.join(path)?;
        url.query_pairs_mut()
            .append_pair("api-version", API_VERSION);
        Ok(url)
    }

    async fn get(&self, path: &str) -> Result<String, MgmtError> {
        let url = self.url_for(path)?;
        // Tokens are minted for the URL without the query string; the service
        // validates the audience against scheme + host + path.
        let resource = format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            url.path()
        );
        tracing::debug!(%url, "management GET");
        let response = self
            .http
            .get(url)
            .header(
                header::AUTHORIZATION,
                self.authorizer.header_value(&resource),
            )
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if status.is_success() {
            Ok(body)
        } else {
            tracing::debug!(%status, %body, "management request failed");
            Err(MgmtError::from_status(status, path, body))
        }
    }
}
