//! UI-side application state: plain data owned by the app, mutated only on
//! the UI thread.

use std::collections::{HashMap, HashSet};

use sift_backend::{
    Disposition, EntityInfo, EntityPath, MessageSource, NamespaceId, OpId, OpKind, ReceiveMode,
    RequestId,
};
use sift_core::message::{OutboundMessage, SiftMessage};
use sift_mgmt::{NamespaceInfo, QueueInfo, RuleInfo, SubscriptionInfo, TopicInfo};
use uuid::Uuid;

/// An entity qualified by the namespace connection it lives on, so multiple
/// simultaneous connections can hold same-named entities apart.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ScopedEntity {
    pub ns: NamespaceId,
    pub path: EntityPath,
}

impl ScopedEntity {
    #[must_use]
    pub fn new(ns: NamespaceId, path: EntityPath) -> Self {
        Self { ns, path }
    }
}

/// One namespace connection held by the UI. `info` is `None` while the
/// connection attempt is still in flight.
#[derive(Debug)]
pub struct Connection {
    pub profile_id: Uuid,
    pub name: String,
    pub info: Option<NamespaceInfo>,
    pub tree: EntityTree,
}

impl Connection {
    #[must_use]
    pub fn connecting(profile_id: Uuid, name: String) -> Self {
        Self {
            profile_id,
            name,
            info: None,
            tree: EntityTree::default(),
        }
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.info.is_some()
    }
}

/// An in-flight connect attempt, tracked so a stale response can be ignored.
#[derive(Debug, Clone)]
pub struct PendingConnect {
    pub req: RequestId,
    pub profile_id: Uuid,
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

/// The entity tree model for one connected namespace. Data-only; the tree
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

/// Case-insensitive substring filter over the tree, with a short debounce so
/// typing doesn't recompute every keystroke. One filter applies across all
/// connections.
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
    /// Forget loaded data (on disconnect or refresh-all).
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
    Sessions,
}

/// Held session and pending request state for one entity.
#[derive(Debug, Default)]
pub struct SessionsView {
    /// Optional session id to accept; empty accepts the next available.
    pub session_id_input: String,
    pub loading: bool,
    pub error: Option<String>,
    pub snapshot: Option<sift_backend::SessionSnapshot>,
    /// Responses must match this request before they can change the view.
    pub pending_request: Option<RequestId>,
    pub pending_message: Option<String>,
    pub deferred: Vec<i64>,
    pub dead_letter_reason: String,
    pub dead_letter_description: String,
    /// Messages fetched per operation.
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

    pub fn begin(&mut self, req: RequestId, token: Option<String>) {
        self.loading = true;
        self.error = None;
        self.pending_request = Some(req);
        self.pending_message = token;
    }

    /// Invalidate late responses while keeping the view's input preferences.
    pub fn release(&mut self) -> Option<Uuid> {
        self.loading = false;
        self.pending_request = None;
        self.pending_message = None;
        self.deferred.clear();
        self.snapshot.take().map(|snapshot| snapshot.lease_id)
    }

    pub fn finish(
        &mut self,
        req: RequestId,
        result: Result<sift_backend::SessionSnapshot, sift_backend::BackendError>,
    ) -> Option<Uuid> {
        if self.pending_request != Some(req) {
            return result
                .ok()
                .map(|snapshot| snapshot.lease_id)
                .filter(|lease_id| {
                    self.snapshot
                        .as_ref()
                        .is_none_or(|snapshot| snapshot.lease_id != *lease_id)
                });
        }
        self.loading = false;
        self.pending_request = None;
        match result {
            Ok(snapshot) => {
                self.deferred.retain(|sequence| {
                    !snapshot
                        .messages
                        .iter()
                        .any(|row| row.sequence_number == *sequence && row.lock_token.is_some())
                });
                self.snapshot = Some(snapshot);
                self.error = None;
            }
            Err(error) => {
                if error.session_lock_lost() {
                    self.release();
                } else if error.lock_lost()
                    && let (Some(snapshot), Some(token)) =
                        (&mut self.snapshot, &self.pending_message)
                {
                    for row in &mut snapshot.messages {
                        if row.lock_token.as_ref() == Some(token) {
                            row.lock_token = None;
                        }
                    }
                }
                self.error = Some(error.message);
            }
        }
        self.pending_message = None;
        None
    }

    /// Apply a settlement only to the receiver/request that issued it.
    /// Returns true when the entity counts should be refreshed.
    pub fn finish_settlement(
        &mut self,
        req: RequestId,
        lease_id: Uuid,
        token: &str,
        disposition: &Disposition,
        result: Result<(), sift_backend::BackendError>,
    ) -> bool {
        if self.pending_request != Some(req)
            || self
                .snapshot
                .as_ref()
                .is_none_or(|snapshot| snapshot.lease_id != lease_id)
        {
            return false;
        }
        self.loading = false;
        self.pending_request = None;
        self.pending_message = None;
        match result {
            Ok(()) => {
                if let Some(snapshot) = &mut self.snapshot {
                    if *disposition == Disposition::Defer
                        && let Some(row) = snapshot
                            .messages
                            .iter()
                            .find(|row| row.lock_token.as_deref() == Some(token))
                        && !self.deferred.contains(&row.sequence_number)
                    {
                        self.deferred.push(row.sequence_number);
                    }
                    snapshot
                        .messages
                        .retain(|row| row.lock_token.as_deref() != Some(token));
                }
                self.error = None;
                true
            }
            Err(error) => {
                if error.session_lock_lost() {
                    self.release();
                } else if error.lock_lost()
                    && let Some(snapshot) = &mut self.snapshot
                {
                    for row in &mut snapshot.messages {
                        if row.lock_token.as_deref() == Some(token) {
                            row.lock_token = None;
                        }
                    }
                }
                self.error = Some(error.message);
                false
            }
        }
    }
}

/// UI state for one message browsing surface (main queue or DLQ).
#[derive(Debug)]
pub struct MessagesView {
    pub rows: Vec<SiftMessage>,
    /// Changes whenever a payload is replaced, even if its allocation is reused.
    pub body_generation: Uuid,
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
            body_generation: Uuid::new_v4(),
            selected: None,
            loading: false,
            error: None,
            fetch_count,
            show_hex: false,
            show_base64: false,
            deferred_seqs: Vec::new(),
        }
    }

    /// Invalidate preview state without scanning or hashing the full payload.
    pub fn invalidate_body_cache(&mut self) {
        self.body_generation = Uuid::new_v4();
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
    Disconnect(NamespaceId),
    ImportLegacyProfiles,
    LoadQueues(NamespaceId),
    LoadTopics(NamespaceId),
    LoadSubscriptions {
        ns: NamespaceId,
        topic: String,
    },
    LoadRules {
        ns: NamespaceId,
        topic: String,
        subscription: String,
    },
    RefreshTree(NamespaceId),
    OpenEntity(ScopedEntity),
    RefreshEntity(ScopedEntity),
    /// Apply a modified description (e.g. status change) to the service.
    UpdateEntity {
        ns: NamespaceId,
        info: Box<EntityInfo>,
    },
    OpenEditDialog {
        ns: NamespaceId,
        info: Box<EntityInfo>,
    },
    OpenCreateDialog {
        ns: NamespaceId,
        kind: CreateKind,
    },
    RequestDelete(ScopedEntity),
    PeekMessages {
        ns: NamespaceId,
        source: MessageSource,
        from_seq: Option<i64>,
        count: u32,
    },
    ReceiveMessages {
        ns: NamespaceId,
        source: MessageSource,
        mode: ReceiveMode,
        count: u32,
    },
    Settle {
        ns: NamespaceId,
        source: MessageSource,
        lock_token: String,
        disposition: Disposition,
    },
    OpenSendDialog {
        ns: NamespaceId,
        target: EntityPath,
        prefill: Option<Box<OutboundMessage>>,
    },
    /// Detach an entity from the dock into its own OS window.
    PopOutEntity(ScopedEntity),
    /// Return a popped-out entity to the dock.
    DockEntity(ScopedEntity),
    /// Ask for confirmation before draining a source.
    RequestPurge {
        ns: NamespaceId,
        source: MessageSource,
    },
    /// Move every dead-letter message back onto its parent entity.
    ResubmitAll {
        ns: NamespaceId,
        source: MessageSource,
        target: EntityPath,
    },
    CancelOp(OpId),
    OpenDashboard,
    RefreshDashboard,
    SetDashboardAutoRefresh(AutoRefresh),
    /// Cancel a scheduled message by sequence number.
    CancelScheduled {
        ns: NamespaceId,
        target: EntityPath,
        sequence_number: i64,
    },
    /// Retrieve deferred messages (by tracked sequence numbers) into a view.
    ReceiveDeferred {
        ns: NamespaceId,
        source: MessageSource,
        sequence_numbers: Vec<i64>,
    },
    ExportNamespace(NamespaceId),
    ImportNamespace {
        ns: NamespaceId,
        overwrite: bool,
    },
    /// Accept and retain a session receiver.
    BrowseSession {
        ns: NamespaceId,
        source: MessageSource,
        session_id: Option<String>,
        count: u32,
    },
    ReceiveSession {
        ns: NamespaceId,
        source: MessageSource,
        lease_id: Uuid,
        count: u32,
        sequence_numbers: Vec<i64>,
    },
    RenewSession {
        ns: NamespaceId,
        source: MessageSource,
        lease_id: Uuid,
        lock_token: Option<String>,
    },
    SettleSessionMessage {
        ns: NamespaceId,
        source: MessageSource,
        lease_id: Uuid,
        lock_token: String,
        disposition: Disposition,
    },
    ReleaseSession {
        ns: NamespaceId,
        source: MessageSource,
    },
    /// Save the selected message's exact body bytes to a local file.
    SaveMessageBody(Box<SiftMessage>),
    /// Save the selected message as a reusable `.sift-message.json` template.
    SaveMessageTemplate(Box<SiftMessage>),
}

/// A running long-operation, tracked for the operations strip.
#[derive(Debug, Clone)]
pub struct RunningOp {
    pub op: OpId,
    pub ns: NamespaceId,
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
    /// Namespaces whose subscriptions should be fanned out once their topic
    /// list arrives (set by a dashboard refresh).
    pub wants_subscriptions: HashSet<Uuid>,
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use sift_backend::{BackendError, SessionSnapshot};
    use sift_core::body::decode;
    use sift_core::message::MessageState;

    fn snapshot() -> SessionSnapshot {
        let until = time::OffsetDateTime::now_utc() + time::Duration::minutes(1);
        SessionSnapshot {
            lease_id: Uuid::new_v4(),
            session_id: "orders".into(),
            locked_until: until,
            state: None,
            messages: vec![SiftMessage {
                sequence_number: 42,
                message_id: None,
                subject: None,
                content_type: None,
                correlation_id: None,
                session_id: Some("orders".into()),
                reply_to: None,
                to: None,
                enqueued_time: None,
                expires_at: None,
                time_to_live: None,
                delivery_count: None,
                state: MessageState::Active,
                lock_token: Some("delivery".into()),
                locked_until: Some(until),
                dead_letter_reason: None,
                dead_letter_error_description: None,
                dead_letter_source: None,
                application_properties: Vec::new(),
                body: decode(b"payload".to_vec()),
            }],
        }
    }

    #[test]
    fn late_accept_after_view_close_releases_its_receiver() {
        let mut view = SessionsView::new(10);
        view.begin(RequestId(1), None);
        assert_eq!(view.release(), None);
        let late = snapshot();
        let lease_id = late.lease_id;
        assert_eq!(view.finish(RequestId(1), Ok(late)), Some(lease_id));
        assert!(view.snapshot.is_none());
        assert!(!view.loading);
    }

    #[test]
    fn stale_snapshot_does_not_release_current_receiver() {
        let mut view = SessionsView::new(10);
        let current = snapshot();
        view.snapshot = Some(current.clone());
        view.begin(RequestId(2), None);
        assert_eq!(view.finish(RequestId(1), Ok(current.clone())), None);
        assert_eq!(view.pending_request, Some(RequestId(2)));
        let old = snapshot();
        let old_id = old.lease_id;
        assert_eq!(view.finish(RequestId(1), Ok(old)), Some(old_id));
        assert_eq!(
            view.snapshot.as_ref().expect("current retained").lease_id,
            current.lease_id
        );
    }

    #[test]
    fn defer_tracks_sequence_and_removes_settled_delivery() {
        let mut view = SessionsView::new(10);
        let current = snapshot();
        let lease_id = current.lease_id;
        view.snapshot = Some(current);
        view.begin(RequestId(1), Some("delivery".into()));
        assert!(view.finish_settlement(
            RequestId(1),
            lease_id,
            "delivery",
            &Disposition::Defer,
            Ok(())
        ));
        assert_eq!(view.deferred, vec![42]);
        assert!(
            view.snapshot
                .as_ref()
                .expect("receiver retained")
                .messages
                .is_empty()
        );
        assert!(!view.finish_settlement(
            RequestId(1),
            lease_id,
            "delivery",
            &Disposition::Defer,
            Ok(())
        ));
        assert_eq!(view.deferred, vec![42]);
    }

    #[test]
    fn retryable_settlement_failure_keeps_delivery_but_lock_loss_clears_it() {
        let mut view = SessionsView::new(10);
        let current = snapshot();
        let lease_id = current.lease_id;
        view.snapshot = Some(current);
        view.begin(RequestId(1), Some("delivery".into()));
        assert!(!view.finish_settlement(
            RequestId(1),
            lease_id,
            "delivery",
            &Disposition::Complete,
            Err(BackendError::new("temporary network failure"))
        ));
        assert_eq!(
            view.snapshot.as_ref().expect("retained").messages[0]
                .lock_token
                .as_deref(),
            Some("delivery")
        );
        view.begin(RequestId(2), Some("delivery".into()));
        assert!(!view.finish_settlement(
            RequestId(2),
            lease_id,
            "delivery",
            &Disposition::Complete,
            Err(BackendError::new("com.microsoft:message-lock-lost"))
        ));
        assert!(
            view.snapshot.as_ref().expect("session retained").messages[0]
                .lock_token
                .is_none()
        );
        view.begin(RequestId(3), None);
        view.finish(
            RequestId(3),
            Err(BackendError::new("com.microsoft:session-lock-lost")),
        );
        assert!(view.snapshot.is_none());
        assert!(
            view.error
                .as_deref()
                .expect("visible failure")
                .contains("session-lock-lost")
        );
    }
}
