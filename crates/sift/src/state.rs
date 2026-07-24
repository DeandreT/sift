//! UI-side application state: plain data owned by the app, mutated only on
//! the UI thread.

use std::collections::HashMap;

use sift_backend::{Disposition, EntityInfo, EntityPath, MessageSource, ReceiveMode, RequestId};
use sift_core::message::{OutboundMessage, SiftMessage};
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

/// Inner page of an entity tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntityPage {
    #[default]
    Overview,
    Messages,
    DeadLetter,
}

/// UI state for one message browsing surface (main queue or DLQ).
#[derive(Debug)]
pub struct MessagesView {
    pub rows: Vec<SiftMessage>,
    pub selected: Option<usize>,
    pub loading: bool,
    pub error: Option<String>,
    /// Messages fetched per peek/receive.
    pub fetch_count: u32,
    /// Body viewer: show hex instead of text.
    pub show_hex: bool,
}

impl MessagesView {
    #[must_use]
    pub fn new(fetch_count: u32) -> Self {
        Self {
            rows: Vec::new(),
            selected: None,
            loading: false,
            error: None,
            fetch_count,
            show_hex: false,
        }
    }

    /// Sequence number to continue peeking from.
    #[must_use]
    pub fn next_seq(&self) -> Option<i64> {
        self.rows.last().map(|m| m.sequence_number + 1)
    }

    #[must_use]
    pub fn selected_message(&self) -> Option<&SiftMessage> {
        self.selected.and_then(|i| self.rows.get(i))
    }

    pub fn remove_by_lock_token(&mut self, token: &str) {
        if let Some(pos) = self
            .rows
            .iter()
            .position(|m| m.lock_token.as_deref() == Some(token))
        {
            self.rows.remove(pos);
            match self.selected {
                Some(s) if s == pos => self.selected = None,
                Some(s) if s > pos => self.selected = Some(s - 1),
                _ => {}
            }
        }
    }
}

/// Everything an open entity tab owns.
#[derive(Debug)]
pub struct EntityTabState {
    pub info: Loadable<EntityInfo>,
    pub page: EntityPage,
    pub main: MessagesView,
    pub dead_letter: MessagesView,
}

impl EntityTabState {
    #[must_use]
    pub fn new(fetch_count: u32) -> Self {
        Self {
            info: Loadable::NotLoaded,
            page: EntityPage::default(),
            main: MessagesView::new(fetch_count),
            dead_letter: MessagesView::new(fetch_count),
        }
    }

    pub fn view_mut(&mut self, dead_letter: bool) -> &mut MessagesView {
        if dead_letter {
            &mut self.dead_letter
        } else {
            &mut self.main
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
    PeekMessages {
        source: MessageSource,
        from_seq: Option<i64>,
        count: u32,
    },
    ReceiveMessages {
        source: MessageSource,
        mode: ReceiveMode,
        count: u32,
    },
    Settle {
        source: MessageSource,
        lock_token: String,
        disposition: Disposition,
    },
    OpenSendDialog {
        target: EntityPath,
        prefill: Option<Box<OutboundMessage>>,
    },
    /// Detach an entity from the dock into its own OS window.
    PopOutEntity(EntityPath),
    /// Return a popped-out entity to the dock.
    DockEntity(EntityPath),
}
