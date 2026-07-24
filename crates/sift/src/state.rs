//! UI-side application state: plain data owned by the app, mutated only on
//! the UI thread.

use std::collections::HashMap;

use sift_backend::{
    Disposition, EntityInfo, EntityPath, MessageSource, OpId, OpKind, ReceiveMode, RequestId,
};
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
    pub filter: TreeFilter,
}

/// Case-insensitive substring filter over the tree, with a short debounce so
/// typing doesn't recompute every keystroke.
#[derive(Debug, Default)]
pub struct TreeFilter {
    /// The text currently in the filter box.
    pub text: String,
    /// The debounced text actually applied to matching.
    applied: String,
    /// Set to request focus on the next frame (from Ctrl+F).
    pub focus_requested: bool,
    last_edit: Option<std::time::Instant>,
}

/// Debounce window for the tree filter.
const FILTER_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);

impl TreeFilter {
    /// Note that the text changed; starts the debounce timer.
    pub fn on_edit(&mut self) {
        self.last_edit = Some(std::time::Instant::now());
    }

    /// Promote `text` to `applied` once the debounce elapses. Returns whether
    /// a repaint should be scheduled (debounce still pending).
    pub fn tick(&mut self) -> bool {
        if let Some(edited) = self.last_edit {
            if edited.elapsed() >= FILTER_DEBOUNCE {
                self.applied = self.text.trim().to_lowercase();
                self.last_edit = None;
            } else {
                return true;
            }
        }
        false
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.applied.clear();
        self.last_edit = None;
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.applied.is_empty()
    }

    /// Does `name` match the applied filter? An empty filter matches all.
    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        self.applied.is_empty() || name.to_lowercase().contains(&self.applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied(text: &str) -> TreeFilter {
        let mut f = TreeFilter {
            text: text.to_owned(),
            ..TreeFilter::default()
        };
        f.on_edit();
        // Force the debounce to have already elapsed so tick() applies now.
        f.last_edit = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(1));
        f.tick();
        f
    }

    #[test]
    fn empty_filter_matches_everything() {
        let f = TreeFilter::default();
        assert!(f.matches("anything"));
        assert!(!f.is_active());
    }

    #[test]
    fn matching_is_case_insensitive_substring() {
        let f = applied("Order");
        assert!(f.is_active());
        assert!(f.matches("orders"));
        assert!(f.matches("PROCESS-ORDERS"));
        assert!(!f.matches("invoices"));
    }

    #[test]
    fn clear_resets_active_state() {
        let mut f = applied("x");
        assert!(f.is_active());
        f.clear();
        assert!(!f.is_active());
        assert!(f.matches("anything"));
    }
}

impl EntityTree {
    /// Forget loaded data (on disconnect or refresh-all), keeping the filter.
    pub fn clear(&mut self) {
        self.queues = Loadable::NotLoaded;
        self.topics = Loadable::NotLoaded;
        self.subscriptions.clear();
        self.rules.clear();
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
    Sessions,
}

/// Read-only session browser state for one entity.
#[derive(Debug, Default)]
pub struct SessionsView {
    /// Optional session id to accept; empty accepts the next available.
    pub session_id_input: String,
    pub loading: bool,
    pub error: Option<String>,
    pub snapshot: Option<sift_backend::SessionSnapshot>,
    /// Messages peeked per browse.
    pub fetch_count: u32,
}

impl SessionsView {
    #[must_use]
    pub fn new(fetch_count: u32) -> Self {
        Self {
            fetch_count,
            ..Self::default()
        }
    }
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
    /// Body viewer: interpret the body as base64 and show the decoded content.
    pub show_base64: bool,
    /// Sequence numbers of messages deferred from this view, so they can be
    /// retrieved later (the service returns nothing on defer).
    pub deferred_seqs: Vec<i64>,
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
            show_base64: false,
            deferred_seqs: Vec::new(),
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
    pub sessions: SessionsView,
}

impl EntityTabState {
    #[must_use]
    pub fn new(fetch_count: u32) -> Self {
        Self {
            info: Loadable::NotLoaded,
            page: EntityPage::default(),
            main: MessagesView::new(fetch_count),
            dead_letter: MessagesView::new(fetch_count),
            sessions: SessionsView::new(fetch_count),
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
    /// Ask for confirmation before draining a source.
    RequestPurge(MessageSource),
    /// Move every dead-letter message back onto its parent entity.
    ResubmitAll {
        source: MessageSource,
        target: EntityPath,
    },
    CancelOp(OpId),
    OpenDashboard,
    RefreshDashboard,
    SetDashboardAutoRefresh(AutoRefresh),
    /// Cancel a scheduled message by sequence number.
    CancelScheduled {
        target: EntityPath,
        sequence_number: i64,
    },
    /// Retrieve deferred messages (by tracked sequence numbers) into a view.
    ReceiveDeferred {
        source: MessageSource,
        sequence_numbers: Vec<i64>,
    },
    ExportNamespace,
    ImportNamespace {
        overwrite: bool,
    },
    /// Accept and browse a session (read-only).
    BrowseSession {
        source: MessageSource,
        session_id: Option<String>,
        count: u32,
    },
}

/// A running long-operation, tracked for the operations strip.
#[derive(Debug, Clone)]
pub struct RunningOp {
    pub op: OpId,
    pub kind: OpKind,
    pub done: u64,
    pub target: String,
}

/// Dashboard auto-refresh cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoRefresh {
    #[default]
    Off,
    Secs30,
    Secs60,
    Min5,
}

impl AutoRefresh {
    pub const ALL: [Self; 4] = [Self::Off, Self::Secs30, Self::Secs60, Self::Min5];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Secs30 => "30s",
            Self::Secs60 => "60s",
            Self::Min5 => "5min",
        }
    }

    #[must_use]
    pub fn interval(self) -> Option<std::time::Duration> {
        let secs = match self {
            Self::Off => return None,
            Self::Secs30 => 30,
            Self::Secs60 => 60,
            Self::Min5 => 300,
        };
        Some(std::time::Duration::from_secs(secs))
    }
}

/// Dashboard tab state (auto-refresh cadence + scheduling).
#[derive(Debug, Default)]
pub struct DashboardState {
    pub auto_refresh: AutoRefresh,
    pub next_refresh: Option<std::time::Instant>,
    /// Set by a refresh so subscriptions are reloaded once topics arrive.
    pub wants_subscriptions: bool,
}
