//! The Command/Event vocabulary shared between the UI and the backend, plus
//! the handle the UI uses to talk to the backend.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sift_core::config::NamespaceProfile;
use sift_core::secrets::SecretString;
use sift_mgmt::{
    MgmtError, NamespaceInfo, QueueInfo, QueueProperties, RuleInfo, RuleProperties,
    SubscriptionInfo, SubscriptionProperties, TopicInfo, TopicProperties,
};
use uuid::Uuid;

/// Correlates a one-shot request with its response event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

/// Namespaces are identified by their profile id.
pub type NamespaceId = Uuid;

/// Addresses one entity inside the connected namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum EntityPath {
    Queue(String),
    Topic(String),
    Subscription {
        topic: String,
        name: String,
    },
    Rule {
        topic: String,
        subscription: String,
        name: String,
    },
}

impl EntityPath {
    /// The entity's own name (last path segment).
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Queue(name)
            | Self::Topic(name)
            | Self::Subscription { name, .. }
            | Self::Rule { name, .. } => name,
        }
    }

    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Queue(_) => "queue",
            Self::Topic(_) => "topic",
            Self::Subscription { .. } => "subscription",
            Self::Rule { .. } => "rule",
        }
    }
}

impl std::fmt::Display for EntityPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Queue(name) | Self::Topic(name) => f.write_str(name),
            Self::Subscription { topic, name } => write!(f, "{topic}/{name}"),
            Self::Rule {
                topic,
                subscription,
                name,
            } => write!(f, "{topic}/{subscription}/{name}"),
        }
    }
}

/// Full user-settable description, for create/update commands.
#[derive(Debug, Clone)]
pub enum EntityDescription {
    Queue(QueueProperties),
    Topic(TopicProperties),
    Subscription(SubscriptionProperties),
    Rule(RuleProperties),
}

impl EntityDescription {
    #[must_use]
    pub fn path(&self) -> EntityPath {
        match self {
            Self::Queue(p) => EntityPath::Queue(p.name.clone()),
            Self::Topic(p) => EntityPath::Topic(p.name.clone()),
            Self::Subscription(p) => EntityPath::Subscription {
                topic: p.topic.clone(),
                name: p.name.clone(),
            },
            Self::Rule(p) => EntityPath::Rule {
                topic: p.topic.clone(),
                subscription: p.subscription.clone(),
                name: p.name.clone(),
            },
        }
    }
}

/// Full entity state as returned by the service.
#[derive(Debug, Clone)]
pub enum EntityInfo {
    Queue(QueueInfo),
    Topic(TopicInfo),
    Subscription(SubscriptionInfo),
    Rule(RuleInfo),
}

impl EntityInfo {
    #[must_use]
    pub fn path(&self) -> EntityPath {
        match self {
            Self::Queue(q) => EntityPath::Queue(q.properties.name.clone()),
            Self::Topic(t) => EntityPath::Topic(t.properties.name.clone()),
            Self::Subscription(s) => EntityPath::Subscription {
                topic: s.properties.topic.clone(),
                name: s.properties.name.clone(),
            },
            Self::Rule(r) => EntityPath::Rule {
                topic: r.properties.topic.clone(),
                subscription: r.properties.subscription.clone(),
                name: r.properties.name.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationOp {
    Created,
    Updated,
    Deleted,
}

/// UI → backend.
#[derive(Debug)]
pub enum Command {
    /// Resolve the profile's secret, build a management client, and validate
    /// the connection with `GET /$namespaceinfo`.
    Connect {
        req: RequestId,
        profile: NamespaceProfile,
        secret: SecretString,
    },
    Disconnect {
        ns: NamespaceId,
    },
    ListQueues {
        req: RequestId,
        ns: NamespaceId,
    },
    ListTopics {
        req: RequestId,
        ns: NamespaceId,
    },
    ListSubscriptions {
        req: RequestId,
        ns: NamespaceId,
        topic: String,
    },
    ListRules {
        req: RequestId,
        ns: NamespaceId,
        topic: String,
        subscription: String,
    },
    GetEntity {
        req: RequestId,
        ns: NamespaceId,
        path: EntityPath,
    },
    CreateEntity {
        req: RequestId,
        ns: NamespaceId,
        desc: EntityDescription,
    },
    UpdateEntity {
        req: RequestId,
        ns: NamespaceId,
        desc: EntityDescription,
    },
    DeleteEntity {
        req: RequestId,
        ns: NamespaceId,
        path: EntityPath,
    },
    Shutdown,
}

/// Backend → UI.
#[derive(Debug)]
pub enum Event {
    Connected {
        req: RequestId,
        ns: NamespaceId,
        result: Result<NamespaceInfo, BackendError>,
    },
    Disconnected {
        ns: NamespaceId,
    },
    Queues {
        req: RequestId,
        result: Result<Vec<QueueInfo>, BackendError>,
    },
    Topics {
        req: RequestId,
        result: Result<Vec<TopicInfo>, BackendError>,
    },
    Subscriptions {
        req: RequestId,
        topic: String,
        result: Result<Vec<SubscriptionInfo>, BackendError>,
    },
    Rules {
        req: RequestId,
        topic: String,
        subscription: String,
        result: Result<Vec<RuleInfo>, BackendError>,
    },
    Entity {
        req: RequestId,
        path: EntityPath,
        result: Result<EntityInfo, BackendError>,
    },
    Mutated {
        req: RequestId,
        op: MutationOp,
        path: EntityPath,
        result: Result<Option<EntityInfo>, BackendError>,
    },
}

/// A user-presentable error from a backend operation.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct BackendError {
    pub message: String,
    /// Raw server/library detail for a "show details" expander.
    pub detail: Option<String>,
}

impl BackendError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: None,
        }
    }
}

impl From<MgmtError> for BackendError {
    fn from(e: MgmtError) -> Self {
        let detail = match &e {
            MgmtError::Unauthorized { detail }
            | MgmtError::Forbidden { detail }
            | MgmtError::Conflict { detail }
            | MgmtError::BadRequest { detail }
            | MgmtError::Throttled { detail }
            | MgmtError::Server { detail, .. } => Some(detail.clone()).filter(|d| !d.is_empty()),
            _ => None,
        };
        Self {
            message: e.to_string(),
            detail,
        }
    }
}

/// Cloneable handle the UI uses to mint request ids and send commands.
#[derive(Debug, Clone)]
pub struct BackendHandle {
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>,
    next_id: Arc<AtomicU64>,
}

impl BackendHandle {
    pub(crate) fn new(cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>) -> Self {
        Self {
            cmd_tx,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    #[must_use]
    pub fn next_request(&self) -> RequestId {
        RequestId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Send a command. The backend outlives the UI, so a closed channel only
    /// happens during shutdown and is logged rather than surfaced.
    pub fn send(&self, cmd: Command) {
        if self.cmd_tx.send(cmd).is_err() {
            tracing::warn!("backend command channel is closed; command dropped");
        }
    }
}
