//! The management HTTP client: request signing, api-version handling, and
//! status-code mapping.

use azure_core::credentials::TokenCredential;
use reqwest::header;
use sift_core::connection::{Credential, NamespaceConnection};
use sift_core::sas::SasTokenProvider;
use sift_core::secrets::SecretString;
use std::sync::Arc;
use url::Url;

use crate::atom;
use crate::error::MgmtError;
use crate::model::{
    NamespaceInfo, QueueInfo, QueueProperties, RuleInfo, RuleProperties, SubscriptionInfo,
    SubscriptionProperties, TopicInfo, TopicProperties,
};
use crate::write;

pub const API_VERSION: &str = "2021-05";
const USER_AGENT: &str = concat!("sift/", env!("CARGO_PKG_VERSION"));
const ATOM_CONTENT_TYPE: &str = "application/atom+xml;type=entry;charset=utf-8";
/// Maximum `$top` the service accepts when listing entities.
const PAGE_SIZE: usize = 100;

/// Produces `Authorization` header values for management requests.
#[derive(Debug)]
pub enum Authorizer {
    /// Mints a SAS token per resource URI from a shared access key.
    Sas(SasTokenProvider),
    /// A pre-minted `SharedAccessSignature ...` token used verbatim.
    StaticSas(SecretString),
    /// Shared refreshable credential, also used by the AMQP runtime.
    Bearer(Arc<dyn TokenCredential>),
}

impl Authorizer {
    #[must_use]
    pub fn from_connection(conn: &NamespaceConnection) -> Self {
        match &conn.credential {
            Credential::SasKey { .. } => Self::Sas(
                SasTokenProvider::from_connection(conn)
                    .expect("SasKey credential always yields a provider"),
            ),
            Credential::SasToken(token) => Self::StaticSas(token.clone()),
        }
    }

    async fn header_value(&self, resource_uri: &str) -> Result<String, MgmtError> {
        Ok(match self {
            Self::Sas(provider) => provider.token_for(resource_uri).value,
            Self::StaticSas(token) => token.expose().to_owned(),
            Self::Bearer(credential) => {
                let token = credential
                    .get_token(&[crate::auth::SERVICE_BUS_SCOPE], None)
                    .await
                    .map_err(|_| MgmtError::Authentication(crate::auth::SIGN_IN_HELP.into()))?;
                format!("Bearer {}", token.token.secret())
            }
        })
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
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            base: conn.https_base(),
            authorizer: Authorizer::from_connection(conn),
        })
    }

    pub fn new_with_credential(
        endpoint: &Url,
        credential: Arc<dyn TokenCredential>,
    ) -> Result<Self, MgmtError> {
        let endpoint = sift_core::connection::namespace_endpoint(endpoint.as_str())
            .map_err(MgmtError::Authentication)?;
        let base = Url::parse(&format!(
            "https://{}/",
            endpoint.host_str().unwrap_or_default()
        ))?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(USER_AGENT)
            .build()?;
        Ok(Self {
            http,
            base,
            authorizer: Authorizer::Bearer(credential),
        })
    }

    // ---- namespace ------------------------------------------------------

    /// `GET /$namespaceinfo` — namespace name, type, and SKU. Doubles as the
    /// cheapest way to validate a connection.
    pub async fn get_namespace_info(&self) -> Result<NamespaceInfo, MgmtError> {
        let body = self.get("$namespaceinfo", &[]).await?;
        atom::parse_namespace_info(&body)
    }

    // ---- queues -----------------------------------------------------------

    pub async fn list_queues(&self) -> Result<Vec<QueueInfo>, MgmtError> {
        self.list("$Resources/queues", atom::parse_queue_feed).await
    }

    pub async fn get_queue(&self, path: &str) -> Result<QueueInfo, MgmtError> {
        found(atom::parse_queue(&self.get(path, &[]).await?)?, path)
    }

    pub async fn create_queue(&self, props: &QueueProperties) -> Result<QueueInfo, MgmtError> {
        let body = self
            .put(&props.name, &write::queue_body(props), false)
            .await?;
        let mut queue = returned(atom::parse_queue(&body)?)?;
        props.name.clone_into(&mut queue.properties.name);
        Ok(queue)
    }

    pub async fn update_queue(&self, props: &QueueProperties) -> Result<QueueInfo, MgmtError> {
        let body = self
            .put(&props.name, &write::queue_body(props), true)
            .await?;
        let mut queue = returned(atom::parse_queue(&body)?)?;
        props.name.clone_into(&mut queue.properties.name);
        Ok(queue)
    }

    pub async fn delete_queue(&self, path: &str) -> Result<(), MgmtError> {
        self.delete(path).await
    }

    // ---- topics -----------------------------------------------------------

    pub async fn list_topics(&self) -> Result<Vec<TopicInfo>, MgmtError> {
        self.list("$Resources/topics", atom::parse_topic_feed).await
    }

    pub async fn get_topic(&self, path: &str) -> Result<TopicInfo, MgmtError> {
        found(atom::parse_topic(&self.get(path, &[]).await?)?, path)
    }

    pub async fn create_topic(&self, props: &TopicProperties) -> Result<TopicInfo, MgmtError> {
        let body = self
            .put(&props.name, &write::topic_body(props), false)
            .await?;
        let mut topic = returned(atom::parse_topic(&body)?)?;
        props.name.clone_into(&mut topic.properties.name);
        Ok(topic)
    }

    pub async fn update_topic(&self, props: &TopicProperties) -> Result<TopicInfo, MgmtError> {
        let body = self
            .put(&props.name, &write::topic_body(props), true)
            .await?;
        let mut topic = returned(atom::parse_topic(&body)?)?;
        props.name.clone_into(&mut topic.properties.name);
        Ok(topic)
    }

    pub async fn delete_topic(&self, path: &str) -> Result<(), MgmtError> {
        self.delete(path).await
    }

    // ---- subscriptions -----------------------------------------------------

    pub async fn list_subscriptions(
        &self,
        topic: &str,
    ) -> Result<Vec<SubscriptionInfo>, MgmtError> {
        let path = format!("{topic}/subscriptions");
        let topic = topic.to_owned();
        self.list(&path, move |xml| atom::parse_subscription_feed(xml, &topic))
            .await
    }

    pub async fn get_subscription(
        &self,
        topic: &str,
        name: &str,
    ) -> Result<SubscriptionInfo, MgmtError> {
        let path = subscription_path(topic, name);
        let body = self.get(&path, &[]).await?;
        found(atom::parse_subscription(&body, topic)?, &path)
    }

    pub async fn create_subscription(
        &self,
        props: &SubscriptionProperties,
    ) -> Result<SubscriptionInfo, MgmtError> {
        let path = subscription_path(&props.topic, &props.name);
        let body = self
            .put(&path, &write::subscription_body(props), false)
            .await?;
        let mut sub = returned(atom::parse_subscription(&body, &props.topic)?)?;
        props.name.clone_into(&mut sub.properties.name);
        Ok(sub)
    }

    pub async fn update_subscription(
        &self,
        props: &SubscriptionProperties,
    ) -> Result<SubscriptionInfo, MgmtError> {
        let path = subscription_path(&props.topic, &props.name);
        let body = self
            .put(&path, &write::subscription_body(props), true)
            .await?;
        let mut sub = returned(atom::parse_subscription(&body, &props.topic)?)?;
        props.name.clone_into(&mut sub.properties.name);
        Ok(sub)
    }

    pub async fn delete_subscription(&self, topic: &str, name: &str) -> Result<(), MgmtError> {
        self.delete(&subscription_path(topic, name)).await
    }

    // ---- rules --------------------------------------------------------------

    pub async fn list_rules(&self, topic: &str, sub: &str) -> Result<Vec<RuleInfo>, MgmtError> {
        let path = format!("{}/rules", subscription_path(topic, sub));
        let (topic, sub) = (topic.to_owned(), sub.to_owned());
        self.list(&path, move |xml| atom::parse_rule_feed(xml, &topic, &sub))
            .await
    }

    pub async fn create_rule(&self, props: &RuleProperties) -> Result<RuleInfo, MgmtError> {
        props
            .filter
            .validate()
            .map_err(|detail| MgmtError::BadRequest { detail })?;
        let path = format!(
            "{}/rules/{}",
            subscription_path(&props.topic, &props.subscription),
            props.name
        );
        let body = self.put(&path, &write::rule_body(props), false).await?;
        returned(atom::parse_rule(&body, &props.topic, &props.subscription)?)
    }

    pub async fn delete_rule(&self, topic: &str, sub: &str, name: &str) -> Result<(), MgmtError> {
        self.delete(&format!("{}/rules/{name}", subscription_path(topic, sub)))
            .await
    }

    // ---- HTTP plumbing -------------------------------------------------------

    fn url_for(&self, path: &str, query: &[(&str, String)]) -> Result<Url, MgmtError> {
        // Entity names may come from imported definitions. Never let them
        // redirect a namespace credential to another origin or inject a query.
        let invalid = || MgmtError::BadRequest {
            detail: "Entity paths must be relative to the connected namespace.".into(),
        };
        if path.starts_with(['/', '\\']) || Url::parse(path).is_ok() {
            return Err(invalid());
        }
        let mut url = self.base.join(path)?;
        if url.origin() != self.base.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid());
        }
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("api-version", API_VERSION);
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        Ok(url)
    }

    /// Audience the SAS token is minted for: scheme + host + path, no query.
    fn resource_uri(url: &Url) -> String {
        format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            url.path()
        )
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<String, MgmtError> {
        let url = self.url_for(path, query)?;
        tracing::debug!(%url, "management GET");
        let request = self
            .http
            .get(url.clone())
            .header(
                header::AUTHORIZATION,
                self.authorizer
                    .header_value(&Self::resource_uri(&url))
                    .await?,
            )
            .send()
            .await?;
        Self::read_body(path, request).await
    }

    /// PUT an ATOM entry; `if_match` distinguishes update (must exist) from
    /// create (409 when it already exists).
    async fn put(&self, path: &str, body: &str, if_match: bool) -> Result<String, MgmtError> {
        let url = self.url_for(path, &[])?;
        tracing::debug!(%url, if_match, "management PUT");
        let mut request = self
            .http
            .put(url.clone())
            .header(
                header::AUTHORIZATION,
                self.authorizer
                    .header_value(&Self::resource_uri(&url))
                    .await?,
            )
            .header(header::CONTENT_TYPE, ATOM_CONTENT_TYPE)
            .body(body.to_owned());
        if if_match {
            request = request.header(header::IF_MATCH, "*");
        }
        Self::read_body(path, request.send().await?).await
    }

    async fn delete(&self, path: &str) -> Result<(), MgmtError> {
        let url = self.url_for(path, &[])?;
        tracing::debug!(%url, "management DELETE");
        let response = self
            .http
            .delete(url.clone())
            .header(
                header::AUTHORIZATION,
                self.authorizer
                    .header_value(&Self::resource_uri(&url))
                    .await?,
            )
            .send()
            .await?;
        Self::read_body(path, response).await.map(|_| ())
    }

    async fn read_body(path: &str, response: reqwest::Response) -> Result<String, MgmtError> {
        let status = response.status();
        let body = response.text().await?;
        if status.is_success() {
            Ok(body)
        } else {
            tracing::debug!(%status, %body, "management request failed");
            Err(MgmtError::from_status(status, path, body))
        }
    }

    /// Fetch every page of a feed (`$skip`/`$top`).
    async fn list<T>(
        &self,
        path: &str,
        parse: impl Fn(&str) -> Result<Vec<T>, MgmtError>,
    ) -> Result<Vec<T>, MgmtError> {
        let mut all = Vec::new();
        let mut skip = 0usize;
        loop {
            let query = [("$skip", skip.to_string()), ("$top", PAGE_SIZE.to_string())];
            let body = self.get(path, &query).await?;
            let page = parse(&body)?;
            let fetched = page.len();
            all.extend(page);
            if fetched < PAGE_SIZE {
                return Ok(all);
            }
            skip += PAGE_SIZE;
        }
    }
}

fn subscription_path(topic: &str, name: &str) -> String {
    format!("{topic}/subscriptions/{name}")
}

/// GETs answer "entity does not exist" with 200 + an empty feed, not 404.
fn found<T>(parsed: Option<T>, path: &str) -> Result<T, MgmtError> {
    parsed.ok_or_else(|| MgmtError::NotFound {
        path: path.to_owned(),
    })
}

/// Create/update responses must echo the entity back.
fn returned<T>(parsed: Option<T>) -> Result<T, MgmtError> {
    parsed.ok_or_else(|| {
        MgmtError::Xml("the service response did not contain an entity description".into())
    })
}

#[cfg(test)]
mod authentication_tests {
    use super::*;
    use azure_core::credentials::{AccessToken, TokenRequestOptions};

    #[derive(Debug)]
    struct RecordingCredential;

    #[async_trait::async_trait]
    impl TokenCredential for RecordingCredential {
        async fn get_token(
            &self,
            scopes: &[&str],
            _: Option<TokenRequestOptions>,
        ) -> azure_core::Result<AccessToken> {
            assert_eq!(scopes, [crate::auth::SERVICE_BUS_SCOPE]);
            Ok(AccessToken::new(
                "test-secret-token",
                time::OffsetDateTime::now_utc() + time::Duration::hours(1),
            ))
        }
    }

    #[tokio::test]
    async fn bearer_management_uses_service_bus_scope_and_redacts_debug() {
        let authorizer = Authorizer::Bearer(Arc::new(RecordingCredential));
        assert_eq!(
            authorizer
                .header_value("https://orders.servicebus.windows.net/$namespaceinfo")
                .await
                .unwrap(),
            "Bearer test-secret-token"
        );
        assert!(!format!("{authorizer:?}").contains("test-secret-token"));
        let endpoint =
            sift_core::connection::namespace_endpoint("orders.servicebus.windows.net").unwrap();
        let client =
            ManagementClient::new_with_credential(&endpoint, Arc::new(RecordingCredential))
                .unwrap();
        assert_eq!(
            client.base.as_str(),
            "https://orders.servicebus.windows.net/"
        );
    }

    #[test]
    fn imported_entity_paths_cannot_change_the_authorization_destination() {
        let endpoint =
            sift_core::connection::namespace_endpoint("orders.servicebus.windows.net").unwrap();
        let client =
            ManagementClient::new_with_credential(&endpoint, Arc::new(RecordingCredential))
                .unwrap();
        for path in [
            "https://attacker.example/queue",
            "//attacker.example/queue",
            "\\\\attacker.example/queue",
            "https://orders.servicebus.windows.net/queue",
            "https://user@orders.servicebus.windows.net/queue",
            "queue?extra=query",
            "queue#fragment",
        ] {
            assert!(client.url_for(path, &[]).is_err(), "accepted {path}");
        }
        let url = client
            .url_for("parent/queue/subscriptions/sub/rules/$Default", &[])
            .unwrap();
        assert_eq!(url.origin(), client.base.origin());
        assert_eq!(url.path(), "/parent/queue/subscriptions/sub/rules/$Default");
        assert_eq!(url.query(), Some("api-version=2021-05"));
    }

    #[tokio::test]
    async fn static_sas_headers_remain_verbatim_and_debug_is_redacted() {
        let authorizer = Authorizer::StaticSas(SecretString::from("SharedAccessSignature secret"));
        assert_eq!(
            authorizer
                .header_value("https://orders.servicebus.windows.net")
                .await
                .unwrap(),
            "SharedAccessSignature secret"
        );
        assert!(!format!("{authorizer:?}").contains("SharedAccessSignature secret"));
    }
}
