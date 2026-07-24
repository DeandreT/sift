//! UI-side application state: plain data owned by the app, mutated only on
//! the UI thread.

use std::collections::HashMap;

use sift_backend::{EntityInfo, EntityPath, RequestId};
use sift_mgmt::{NamespaceInfo, QueueInfo, RuleInfo, SubscriptionInfo, TopicInfo};
use uuid::Uuid;

/// Where we are with the (single, for now) namespace connection.
#[derive(Debug, Clone, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting {
        profile_id: Uuid,
        name: String,
    },
    Connected {
        profile_id: Uuid,
        name: String,
        info: NamespaceInfo,
    },
}

impl ConnectionState {
    #[must_use]
    pub fn namespace_id(&self) -> Option<Uuid> {
        match self {
            Self::Disconnected => None,
            Self::Connecting { profile_id, .. } | Self::Connected { profile_id, .. } => {
                Some(*profile_id)
            }
        }
    }
}

/// An in-flight connect request, used to match the response event.
#[derive(Debug)]
pub struct PendingConnect {
    pub req: RequestId,
    pub name: String,
}

/// Lifecycle of lazily-fetched data.
#[derive(Debug, Clone, Default)]
pub enum Loadable<T> {
    #[default]
    NotLoaded,
    Loading,
    Loaded(T),
    Failed(String),
}

/// The entity tree model for the connected namespace. Data-only; the tree
/// panel renders whatever is here and emits load actions for missing pieces.
#[derive(Debug, Default)]
pub struct EntityTree {
    pub queues: Loadable<Vec<QueueInfo>>,
    pub topics: Loadable<Vec<TopicInfo>>,
    /// Keyed by topic path.
    pub subscriptions: HashMap<String, Loadable<Vec<SubscriptionInfo>>>,
    /// Keyed by (topic, subscription).
    pub rules: HashMap<(String, String), Loadable<Vec<RuleInfo>>>,
}

impl EntityTree {
    /// Forget everything (on disconnect or refresh-all).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Drop the cached list that contains `path`, forcing a reload.
    pub fn invalidate_list_for(&mut self, path: &EntityPath) {
        match path {
            EntityPath::Queue(_) => self.queues = Loadable::NotLoaded,
            EntityPath::Topic(_) => {
                self.topics = Loadable::NotLoaded;
            }
            EntityPath::Subscription { topic, .. } => {
                self.subscriptions.remove(topic);
            }
            EntityPath::Rule {
                topic,
                subscription,
                ..
            } => {
                self.rules.remove(&(topic.clone(), subscription.clone()));
            }
        }
    }
}

/// What kind of entity a create dialog is building.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateKind {
    Queue,
    Topic,
    Subscription { topic: String },
    Rule { topic: String, subscription: String },
}

/// Intents emitted by UI widgets during draw and executed by the app
/// afterwards, so widgets never mutate app state mid-frame.
#[derive(Debug, Clone)]
pub enum AppAction {
    OpenConnectDialog,
    Disconnect,
    ImportLegacyProfiles,
    LoadQueues,
    LoadTopics,
    LoadSubscriptions(String),
    LoadRules(String, String),
    RefreshTree,
    OpenEntity(EntityPath),
    RefreshEntity(EntityPath),
    /// Apply a modified description (e.g. status change) to the service.
    UpdateEntity(Box<EntityInfo>),
    OpenCreateDialog(CreateKind),
    RequestDelete(EntityPath),
}
