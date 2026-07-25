//! The Command/Event vocabulary shared between the UI and the backend, plus
//! the handle the UI uses to talk to the backend.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU64, Ordering};

use sift_core::body::DecodedBody;
use sift_core::config::NamespaceProfile;
use sift_core::message::{OutboundMessage, SiftMessage};
use sift_core::secrets::SecretString;
use sift_mgmt::{
    MgmtError, NamespaceInfo, QueueInfo, QueueProperties, RuleInfo, RuleProperties,
    SubscriptionInfo, SubscriptionProperties, TopicInfo, TopicProperties,
};
use uuid::Uuid;

/// Correlates a one-shot request with its response event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

/// Identifies a long-running, cancellable operation (purge, resubmit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OpId(pub u64);

/// Namespaces are identified by their profile id.
pub type NamespaceId = Uuid;

/// The kind of long-running operation, for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Purge,
    Resubmit,
}

impl OpKind {
    #[must_use]
    pub fn verb(self) -> &'static str {
        match self {
            Self::Purge => "Purge",
            Self::Resubmit => "Resubmit",
        }
    }
}

/// Final tally of a completed operation.
#[derive(Debug, Clone)]
pub struct OpSummary {
    pub kind: OpKind,
    pub processed: u64,
    /// What was purged/resubmitted, for the toast/log.
    pub target: String,
}

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

/// A read-only snapshot of one message session: the accepted session's id,
/// its custom state (decoded for display), and its peeked messages.
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub session_id: String,
    /// The session's custom state; `None` when unset.
    pub state: Option<DecodedBody>,
    pub messages: Vec<SiftMessage>,
}

/// Where messages are browsed from: an entity's main queue or its
/// dead-letter sub-queue. `entity` is a queue or subscription.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MessageSource {
    pub entity: EntityPath,
    pub dead_letter: bool,
}

impl std::fmt::Display for MessageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.dead_letter {
            write!(f, "{}/$DeadLetterQueue", self.entity)
        } else {
            self.entity.fmt(f)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReceiveMode {
    PeekLock,
    ReceiveAndDelete,
}

/// How to settle a peek-locked message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    Complete,
    Abandon,
    Defer,
    DeadLetter {
        reason: Option<String>,
        description: Option<String>,
    },
}

impl Disposition {
    #[must_use]
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Complete => "Completed",
            Self::Abandon => "Abandoned",
            Self::Defer => "Deferred",
            Self::DeadLetter { .. } => "Dead-lettered",
        }
    }
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
    PeekMessages {
        req: RequestId,
        ns: NamespaceId,
        source: MessageSource,
        /// Peek from this sequence number onward; `None` starts at the front.
        from_seq: Option<i64>,
        count: u32,
    },
    ReceiveMessages {
        req: RequestId,
        ns: NamespaceId,
        source: MessageSource,
        mode: ReceiveMode,
        count: u32,
    },
    SettleMessage {
        req: RequestId,
        ns: NamespaceId,
        source: MessageSource,
        lock_token: String,
        disposition: Disposition,
    },
    SendMessages {
        req: RequestId,
        ns: NamespaceId,
        target: EntityPath,
        messages: Vec<OutboundMessage>,
    },
    /// Schedule messages for future enqueue; the response carries their
    /// scheduled sequence numbers.
    ScheduleMessages {
        req: RequestId,
        ns: NamespaceId,
        target: EntityPath,
        messages: Vec<OutboundMessage>,
        enqueue_at: time::OffsetDateTime,
    },
    /// Cancel a scheduled message by its sequence number.
    CancelScheduled {
        req: RequestId,
        ns: NamespaceId,
        target: EntityPath,
        sequence_number: i64,
    },
    /// Retrieve previously deferred messages by sequence number (peek-lock).
    ReceiveDeferred {
        req: RequestId,
        ns: NamespaceId,
        source: MessageSource,
        sequence_numbers: Vec<i64>,
    },
    /// Accept a session (next available, or a named one), peek its messages
    /// and read its state, then release it. Read-only browse.
    BrowseSession {
        req: RequestId,
        ns: NamespaceId,
        source: MessageSource,
        /// `None` accepts the next available session.
        session_id: Option<String>,
        count: u32,
    },
    /// Export the namespace's entity descriptions to a JSON file.
    ExportNamespace {
        req: RequestId,
        ns: NamespaceId,
        path: std::path::PathBuf,
    },
    /// Create entities from a JSON export file.
    ImportNamespace {
        req: RequestId,
        ns: NamespaceId,
        path: std::path::PathBuf,
        overwrite: bool,
    },
    /// Drain all messages from a queue/subscription (or its dead-letter
    /// sub-queue) using receive-and-delete.
    StartPurge {
        op: OpId,
        ns: NamespaceId,
        source: MessageSource,
    },
    /// Move every message from a dead-letter sub-queue back onto `target`.
    StartResubmit {
        op: OpId,
        ns: NamespaceId,
        source: MessageSource,
        target: EntityPath,
    },
    CancelOp(OpId),
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
        ns: NamespaceId,
        result: Result<Vec<QueueInfo>, BackendError>,
    },
    Topics {
        req: RequestId,
        ns: NamespaceId,
        result: Result<Vec<TopicInfo>, BackendError>,
    },
    Subscriptions {
        req: RequestId,
        ns: NamespaceId,
        topic: String,
        result: Result<Vec<SubscriptionInfo>, BackendError>,
    },
    Rules {
        req: RequestId,
        ns: NamespaceId,
        topic: String,
        subscription: String,
        result: Result<Vec<RuleInfo>, BackendError>,
    },
    Entity {
        req: RequestId,
        ns: NamespaceId,
        path: EntityPath,
        result: Result<EntityInfo, BackendError>,
    },
    Mutated {
        req: RequestId,
        ns: NamespaceId,
        op: MutationOp,
        path: EntityPath,
        result: Result<Option<EntityInfo>, BackendError>,
    },
    Messages {
        req: RequestId,
        ns: NamespaceId,
        source: MessageSource,
        /// The `from_seq` of the request; `Some` means "append to the view".
        from_seq: Option<i64>,
        /// True when these came from a receive (destructive or locking)
        /// rather than a peek.
        received: bool,
        result: Result<Vec<SiftMessage>, BackendError>,
    },
    Settled {
        req: RequestId,
        ns: NamespaceId,
        source: MessageSource,
        lock_token: String,
        disposition: Disposition,
        result: Result<(), BackendError>,
    },
    Sent {
        req: RequestId,
        ns: NamespaceId,
        target: EntityPath,
        count: usize,
        /// Sequence numbers when this was a schedule; empty for a plain send.
        result: Result<Vec<i64>, BackendError>,
    },
    ScheduledCancelled {
        req: RequestId,
        ns: NamespaceId,
        target: EntityPath,
        sequence_number: i64,
        result: Result<(), BackendError>,
    },
    /// One-line summary of an import/export, e.g. "exported 12 entities".
    NamespaceTransfer {
        req: RequestId,
        ns: NamespaceId,
        result: Result<String, BackendError>,
    },
    OpProgress {
        op: OpId,
        ns: NamespaceId,
        kind: OpKind,
        /// Items processed so far.
        done: u64,
        /// Human-readable target, e.g. `orders/$DeadLetterQueue`.
        target: String,
    },
    OpFinished {
        op: OpId,
        ns: NamespaceId,
        result: Result<OpSummary, BackendError>,
        cancelled: bool,
    },
    Session {
        req: RequestId,
        ns: NamespaceId,
        source: MessageSource,
        result: Result<SessionSnapshot, BackendError>,
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
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub struct BackendHandle {
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Command>,
    next_id: Arc<AtomicU64>,
}

#[cfg(not(target_arch = "wasm32"))]
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

    #[must_use]
    pub fn next_op(&self) -> OpId {
        OpId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Send a command. The backend outlives the UI, so a closed channel only
    /// happens during shutdown and is logged rather than surfaced.
    pub fn send(&self, cmd: Command) {
        if self.cmd_tx.send(cmd).is_err() {
            tracing::warn!("backend command channel is closed; command dropped");
        }
    }
}
