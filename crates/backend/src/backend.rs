//! The backend runtime: a dedicated thread running tokio, processing
//! [`Command`]s and emitting [`Event`]s.

use std::collections::HashMap;
use std::sync::Arc;

use sift_core::config::{AuthMethod, NamespaceProfile};
use sift_core::connection::{NamespaceConnection, TransportType};
use sift_mgmt::ManagementClient;
use tokio::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::bridge::{
    BackendError, BackendHandle, Command, Disposition, EntityDescription, EntityInfo, EntityPath,
    Event, MessageSource, MutationOp, NamespaceId, OpId, OpKind, OpSummary, ReceiveMode, RequestId,
};
use crate::sb_runtime::SbRuntime;

/// Batch size for purge/resubmit receive loops.
const OP_BATCH: u32 = 100;
/// Wait for each op batch; a full empty wait signals the queue is drained.
const OP_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
/// Consecutive empty batches before an op concludes the source is drained.
const OP_EMPTY_STREAK: u32 = 2;

/// Called after every event so the UI repaints promptly; the GUI passes
/// `egui::Context::request_repaint` without this crate depending on egui.
pub type RepaintFn = Arc<dyn Fn() + Send + Sync>;

/// How long a receive waits for messages before returning what it has.
const RECEIVE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
/// Includes waiting for an in-flight receiver operation to yield its mutex.
const CLEANUP_WAIT: std::time::Duration = std::time::Duration::from_secs(8);

/// Start the backend thread. Returns the command handle and the event
/// receiver the UI drains each frame.
#[must_use]
pub fn spawn(repaint: RepaintFn) -> (BackendHandle, crossbeam_channel::Receiver<Event>) {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (evt_tx, evt_rx) = crossbeam_channel::unbounded();

    std::thread::Builder::new()
        .name("sift-backend".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("failed to build the tokio runtime");
            runtime.block_on(run(cmd_rx, EventSink { evt_tx, repaint }));
        })
        .expect("failed to spawn the backend thread");

    (BackendHandle::new(cmd_tx), evt_rx)
}

/// Sends events to the UI and wakes it up.
#[derive(Clone)]
struct EventSink {
    evt_tx: crossbeam_channel::Sender<Event>,
    repaint: RepaintFn,
}

impl EventSink {
    fn send(&self, event: Event) {
        if self.evt_tx.send(event).is_ok() {
            (self.repaint)();
        }
    }
}

/// Everything held for one connected namespace.
struct NamespaceState {
    mgmt: Arc<ManagementClient>,
    conn: MessagingConnection,
    /// AMQP runtime, created lazily on the first messaging operation.
    sb: Option<Arc<Mutex<SbRuntime>>>,
}

#[derive(Clone)]
enum MessagingConnection {
    Sas(Box<NamespaceConnection>),
    Entra {
        namespace: String,
        transport: TransportType,
        credential: Arc<dyn azure_core::credentials::TokenCredential>,
    },
}

impl MessagingConnection {
    async fn connect(&self) -> Result<SbRuntime, BackendError> {
        match self {
            Self::Sas(conn) => SbRuntime::connect(conn).await,
            Self::Entra {
                namespace,
                transport,
                credential,
            } => SbRuntime::connect_entra(namespace, *transport, Arc::clone(credential)).await,
        }
    }
}

#[derive(Default)]
struct State {
    namespaces: HashMap<NamespaceId, NamespaceState>,
    pending_connections: HashMap<NamespaceId, (RequestId, CancellationToken)>,
    retired_cleanup: Vec<tokio::task::JoinHandle<()>>,
    /// Cancellation handles for in-flight long-running operations.
    ops: HashMap<OpId, CancellationToken>,
}

type SharedState = Arc<Mutex<State>>;

#[allow(clippy::too_many_lines)] // one match arm per command; splitting hurts readability
async fn run(mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Command>, sink: EventSink) {
    let state: SharedState = Arc::default();
    let mut disconnect_cleanup = Vec::new();
    tracing::debug!("backend runtime started");

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Command::Connect {
                req,
                profile,
                secret,
            } => {
                begin_connect(&sink, &state, req, profile, secret, false).await;
            }
            Command::SignIn { req, profile } => {
                begin_connect(
                    &sink,
                    &state,
                    req,
                    profile,
                    sift_core::secrets::SecretString::default(),
                    true,
                )
                .await;
            }
            Command::Disconnect { ns } => {
                let removed = {
                    let mut guard = state.lock().await;
                    if let Some((_, token)) = guard.pending_connections.remove(&ns) {
                        token.cancel();
                    }
                    guard.namespaces.remove(&ns)
                };
                if let Some(NamespaceState { sb: Some(sb), .. }) = removed {
                    disconnect_cleanup
                        .retain(|task: &tokio::task::JoinHandle<()>| !task.is_finished());
                    disconnect_cleanup.push(tokio::spawn(close_runtime(sb)));
                }
                tracing::info!(%ns, "disconnected");
                sink.send(Event::Disconnected { ns });
            }
            Command::ListQueues { req, ns } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = client.list_queues().await.map_err(Into::into);
                    sink.send(Event::Queues { req, ns, result });
                });
            }
            Command::ListTopics { req, ns } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = client.list_topics().await.map_err(Into::into);
                    sink.send(Event::Topics { req, ns, result });
                });
            }
            Command::ListSubscriptions { req, ns, topic } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = client.list_subscriptions(&topic).await.map_err(Into::into);
                    sink.send(Event::Subscriptions {
                        req,
                        ns,
                        topic,
                        result,
                    });
                });
            }
            Command::ListRules {
                req,
                ns,
                topic,
                subscription,
            } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = client
                        .list_rules(&topic, &subscription)
                        .await
                        .map_err(Into::into);
                    sink.send(Event::Rules {
                        req,
                        ns,
                        topic,
                        subscription,
                        result,
                    });
                });
            }
            Command::GetEntity { req, ns, path } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = get_entity(&client, &path).await;
                    sink.send(Event::Entity {
                        req,
                        ns,
                        path,
                        result,
                    });
                });
            }
            Command::CreateEntity { req, ns, desc } => {
                mutate(&sink, &state, ns, req, MutationOp::Created, desc);
            }
            Command::UpdateEntity { req, ns, desc } => {
                mutate(&sink, &state, ns, req, MutationOp::Updated, desc);
            }
            Command::DeleteEntity { req, ns, path } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = delete_entity(&client, &path).await.map(|()| None);
                    log_mutation(MutationOp::Deleted, &path, &result);
                    sink.send(Event::Mutated {
                        req,
                        ns,
                        op: MutationOp::Deleted,
                        path,
                        result,
                    });
                });
            }
            Command::PeekMessages {
                req,
                ns,
                source,
                from_seq,
                count,
            } => {
                let (sink, state) = (sink.clone(), Arc::clone(&state));
                tokio::spawn(async move {
                    let result = match runtime_for(&state, ns).await {
                        Ok(rt) => rt.lock().await.peek(&source, from_seq, count).await,
                        Err(e) => Err(e),
                    };
                    if let Err(e) = &result {
                        tracing::error!("peek from '{source}' failed: {e}");
                    }
                    sink.send(Event::Messages {
                        req,
                        ns,
                        source,
                        from_seq,
                        received: false,
                        result,
                    });
                });
            }
            Command::ReceiveMessages {
                req,
                ns,
                source,
                mode,
                count,
            } => {
                let (sink, state) = (sink.clone(), Arc::clone(&state));
                tokio::spawn(async move {
                    let result = match runtime_for(&state, ns).await {
                        Ok(rt) => {
                            rt.lock()
                                .await
                                .receive(&source, mode, count, RECEIVE_WAIT)
                                .await
                        }
                        Err(e) => Err(e),
                    };
                    match &result {
                        Ok(messages) => {
                            tracing::info!("received {} messages from '{source}'", messages.len());
                        }
                        Err(e) => tracing::error!("receive from '{source}' failed: {e}"),
                    }
                    sink.send(Event::Messages {
                        req,
                        ns,
                        source,
                        from_seq: None,
                        received: true,
                        result,
                    });
                });
            }
            Command::SettleMessage {
                req,
                ns,
                source,
                lock_token,
                disposition,
            } => {
                let (sink, state) = (sink.clone(), Arc::clone(&state));
                tokio::spawn(async move {
                    let result = match runtime_for(&state, ns).await {
                        Ok(rt) => {
                            rt.lock()
                                .await
                                .settle(&lock_token, disposition.clone())
                                .await
                        }
                        Err(e) => Err(e),
                    };
                    match &result {
                        Ok(()) => tracing::info!("{} message on '{source}'", disposition.verb()),
                        Err(e) => tracing::error!("settle on '{source}' failed: {e}"),
                    }
                    sink.send(Event::Settled {
                        req,
                        ns,
                        source,
                        lock_token,
                        disposition,
                        result,
                    });
                });
            }
            Command::SendMessages {
                req,
                ns,
                target,
                messages,
            } => {
                let (sink, state) = (sink.clone(), Arc::clone(&state));
                tokio::spawn(async move {
                    let count = messages.len();
                    let result = match runtime_for(&state, ns).await {
                        Ok(rt) => rt
                            .lock()
                            .await
                            .send(&target, messages)
                            .await
                            .map(|_| Vec::new()),
                        Err(e) => Err(e),
                    };
                    match &result {
                        Ok(_) => tracing::info!("sent {count} message(s) to '{target}'"),
                        Err(e) => tracing::error!("send to '{target}' failed: {e}"),
                    }
                    sink.send(Event::Sent {
                        req,
                        ns,
                        target,
                        count,
                        result,
                    });
                });
            }
            Command::ScheduleMessages {
                req,
                ns,
                target,
                messages,
                enqueue_at,
            } => {
                let (sink, state) = (sink.clone(), Arc::clone(&state));
                tokio::spawn(async move {
                    let count = messages.len();
                    let result = match runtime_for(&state, ns).await {
                        Ok(rt) => {
                            rt.lock()
                                .await
                                .schedule(&target, messages, enqueue_at)
                                .await
                        }
                        Err(e) => Err(e),
                    };
                    match &result {
                        Ok(seqs) => {
                            tracing::info!("scheduled {} message(s) on '{target}'", seqs.len());
                        }
                        Err(e) => tracing::error!("schedule on '{target}' failed: {e}"),
                    }
                    sink.send(Event::Sent {
                        req,
                        ns,
                        target,
                        count,
                        result,
                    });
                });
            }
            Command::CancelScheduled {
                req,
                ns,
                target,
                sequence_number,
            } => {
                let (sink, state) = (sink.clone(), Arc::clone(&state));
                tokio::spawn(async move {
                    let result = match runtime_for(&state, ns).await {
                        Ok(rt) => {
                            rt.lock()
                                .await
                                .cancel_scheduled(&target, sequence_number)
                                .await
                        }
                        Err(e) => Err(e),
                    };
                    if let Err(e) = &result {
                        tracing::error!("cancel scheduled on '{target}' failed: {e}");
                    }
                    sink.send(Event::ScheduledCancelled {
                        req,
                        ns,
                        target,
                        sequence_number,
                        result,
                    });
                });
            }
            Command::ReceiveDeferred {
                req,
                ns,
                source,
                sequence_numbers,
            } => {
                let (sink, state) = (sink.clone(), Arc::clone(&state));
                tokio::spawn(async move {
                    let result = match runtime_for(&state, ns).await {
                        Ok(rt) => {
                            rt.lock()
                                .await
                                .receive_deferred(&source, sequence_numbers)
                                .await
                        }
                        Err(e) => Err(e),
                    };
                    if let Err(e) = &result {
                        tracing::error!("receive deferred from '{source}' failed: {e}");
                    }
                    sink.send(Event::Messages {
                        req,
                        ns,
                        source,
                        from_seq: None,
                        received: true,
                        result,
                    });
                });
            }
            Command::BrowseSession {
                req,
                ns,
                source,
                session_id,
                count,
            } => {
                let (sink, state) = (sink.clone(), Arc::clone(&state));
                tokio::spawn(async move {
                    let result = match runtime_for(&state, ns).await {
                        Ok(rt) => {
                            rt.lock()
                                .await
                                .browse_session(&source, session_id, count)
                                .await
                        }
                        Err(e) => Err(e),
                    };
                    match &result {
                        Ok(s) => tracing::info!(
                            "browsed session '{}' on '{source}' ({} message(s))",
                            s.session_id,
                            s.messages.len()
                        ),
                        Err(e) => tracing::error!("browse session on '{source}' failed: {e}"),
                    }
                    sink.send(Event::Session {
                        req,
                        ns,
                        source,
                        result,
                    });
                });
            }
            Command::ReceiveSession {
                req,
                ns,
                source,
                lease_id,
                count,
                sequence_numbers,
            } => {
                let (sink, state) = (sink.clone(), Arc::clone(&state));
                tokio::spawn(async move {
                    let result = match runtime_for(&state, ns).await {
                        Ok(rt) => {
                            rt.lock()
                                .await
                                .receive_session(
                                    &source,
                                    lease_id,
                                    count,
                                    sequence_numbers,
                                    RECEIVE_WAIT,
                                )
                                .await
                        }
                        Err(e) => Err(e),
                    };
                    sink.send(Event::Session {
                        req,
                        ns,
                        source,
                        result,
                    });
                });
            }
            Command::RenewSession {
                req,
                ns,
                source,
                lease_id,
                lock_token,
            } => {
                let (sink, state) = (sink.clone(), Arc::clone(&state));
                tokio::spawn(async move {
                    let result = match runtime_for(&state, ns).await {
                        Ok(rt) => {
                            rt.lock()
                                .await
                                .renew_session(&source, lease_id, lock_token)
                                .await
                        }
                        Err(e) => Err(e),
                    };
                    sink.send(Event::Session {
                        req,
                        ns,
                        source,
                        result,
                    });
                });
            }
            Command::SettleSessionMessage {
                req,
                ns,
                source,
                lease_id,
                lock_token,
                disposition,
            } => {
                let (sink, state) = (sink.clone(), Arc::clone(&state));
                tokio::spawn(async move {
                    let result = match runtime_for(&state, ns).await {
                        Ok(rt) => {
                            rt.lock()
                                .await
                                .settle_session(&source, lease_id, &lock_token, disposition.clone())
                                .await
                        }
                        Err(e) => Err(e),
                    };
                    sink.send(Event::SessionSettled {
                        req,
                        ns,
                        source,
                        lease_id,
                        lock_token,
                        disposition,
                        result,
                    });
                });
            }
            Command::ReleaseSession {
                ns,
                source,
                lease_id,
            } => {
                let (state, sink) = (Arc::clone(&state), sink.clone());
                tokio::spawn(async move {
                    // Cleanup must not create a new AMQP connection.
                    let rt = state
                        .lock()
                        .await
                        .namespaces
                        .get(&ns)
                        .and_then(|nss| nss.sb.clone());
                    if let Some(rt) = rt {
                        rt.lock().await.release_session(&source, lease_id).await;
                        (sink.repaint)();
                    }
                });
            }
            Command::ExportNamespace { req, ns, path } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = export_namespace(&client, &path).await;
                    sink.send(Event::NamespaceTransfer { req, ns, result });
                });
            }
            Command::ImportNamespace {
                req,
                ns,
                path,
                overwrite,
            } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = import_namespace(&client, &path, overwrite).await;
                    sink.send(Event::NamespaceTransfer { req, ns, result });
                });
            }
            Command::StartPurge { op, ns, source } => {
                start_op(&sink, &state, ns, op, OpKind::Purge, source, None);
            }
            Command::StartResubmit {
                op,
                ns,
                source,
                target,
            } => {
                start_op(
                    &sink,
                    &state,
                    ns,
                    op,
                    OpKind::Resubmit,
                    source,
                    Some(target),
                );
            }
            Command::CancelOp(op) => {
                if let Some(token) = state.lock().await.ops.get(&op) {
                    tracing::info!("cancelling operation {op:?}");
                    token.cancel();
                }
            }
            Command::Shutdown => break,
        }
    }
    let runtimes = {
        let mut guard = state.lock().await;
        for token in guard.ops.values() {
            token.cancel();
        }
        for (_, token) in guard.pending_connections.values() {
            token.cancel();
        }
        guard.pending_connections.clear();
        disconnect_cleanup.append(&mut guard.retired_cleanup);
        std::mem::take(&mut guard.namespaces)
            .into_values()
            .filter_map(|ns| ns.sb)
            .collect::<Vec<_>>()
    };
    for runtime in runtimes {
        disconnect_cleanup.push(tokio::spawn(close_runtime(runtime)));
    }
    for task in disconnect_cleanup {
        let _ = task.await;
    }
    tracing::debug!("backend runtime stopped");
}

async fn close_runtime(runtime: Arc<Mutex<SbRuntime>>) {
    if tokio::time::timeout(CLEANUP_WAIT, async {
        runtime.lock().await.shutdown().await;
    })
    .await
    .is_err()
    {
        tracing::warn!("AMQP cleanup exceeded its deadline");
    }
}

/// Spawn a purge or resubmit operation on a dedicated AMQP connection (so a
/// long drain never blocks the shared runtime's mutex), reporting progress and
/// honoring cancellation.
fn start_op(
    sink: &EventSink,
    state: &SharedState,
    ns: NamespaceId,
    op: OpId,
    kind: OpKind,
    source: MessageSource,
    target: Option<EntityPath>,
) {
    let (sink, state) = (sink.clone(), Arc::clone(state));
    let token = CancellationToken::new();

    tokio::spawn(async move {
        let conn = {
            let mut guard = state.lock().await;
            let Some(nss) = guard.namespaces.get(&ns) else {
                sink.send(Event::OpFinished {
                    op,
                    ns,
                    result: Err(BackendError::new("not connected to this namespace")),
                    cancelled: false,
                });
                return;
            };
            let conn = nss.conn.clone();
            guard.ops.insert(op, token.clone());
            conn
        };

        let target_label = source.to_string();
        let result = run_op(&conn, op, ns, kind, &source, target.as_ref(), &token, &sink).await;
        let cancelled = token.is_cancelled();
        state.lock().await.ops.remove(&op);

        match &result {
            Ok(summary) => tracing::info!(
                "{} finished: {} messages from '{target_label}'{}",
                kind.verb(),
                summary.processed,
                if cancelled { " (cancelled)" } else { "" }
            ),
            Err(e) => tracing::error!("{} of '{target_label}' failed: {e}", kind.verb()),
        }
        sink.send(Event::OpFinished {
            op,
            ns,
            result,
            cancelled,
        });
    });
}

#[allow(clippy::too_many_arguments)] // internal orchestration helper
async fn run_op(
    conn: &MessagingConnection,
    op: OpId,
    ns: NamespaceId,
    kind: OpKind,
    source: &MessageSource,
    target: Option<&EntityPath>,
    token: &CancellationToken,
    sink: &EventSink,
) -> Result<OpSummary, BackendError> {
    let mut rt = conn.connect().await?;
    let mut processed: u64 = 0;
    let mut empty_streak = 0u32;
    let target_label = source.to_string();

    while empty_streak < OP_EMPTY_STREAK {
        if token.is_cancelled() {
            break;
        }
        // Purge consumes destructively; resubmit locks so a failed send leaves
        // the message safely in the dead-letter queue.
        let mode = match kind {
            OpKind::Purge => ReceiveMode::ReceiveAndDelete,
            OpKind::Resubmit => ReceiveMode::PeekLock,
        };
        let batch = tokio::select! {
            () = token.cancelled() => break,
            r = rt.receive(source, mode, OP_BATCH, OP_WAIT) => r?,
        };
        if batch.is_empty() {
            empty_streak += 1;
            continue;
        }
        empty_streak = 0;

        if let (OpKind::Resubmit, Some(target)) = (kind, target) {
            for message in &batch {
                let Some(token) = &message.lock_token else {
                    continue;
                };
                rt.send(target, vec![message.to_outbound()]).await?;
                rt.settle(token, Disposition::Complete).await?;
                processed += 1;
            }
        } else {
            processed += batch.len() as u64;
        }

        sink.send(Event::OpProgress {
            op,
            ns,
            kind,
            done: processed,
            target: target_label.clone(),
        });
    }

    rt.shutdown().await;
    Ok(OpSummary {
        kind,
        processed,
        target: target_label,
    })
}

/// Look up the namespace's management client and run `op` on a fresh task.
fn spawn_op<F, Fut>(sink: &EventSink, state: &SharedState, ns: NamespaceId, op: F)
where
    F: FnOnce(Arc<ManagementClient>, EventSink) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    let (sink, state) = (sink.clone(), Arc::clone(state));
    tokio::spawn(async move {
        let client = state
            .lock()
            .await
            .namespaces
            .get(&ns)
            .map(|n| Arc::clone(&n.mgmt));
        if let Some(client) = client {
            op(client, sink).await;
        } else {
            tracing::warn!(%ns, "command for a namespace that is not connected");
        }
    });
}

/// Get the namespace's AMQP runtime, establishing the connection on first use.
async fn runtime_for(
    state: &SharedState,
    ns: NamespaceId,
) -> Result<Arc<Mutex<SbRuntime>>, BackendError> {
    let (conn, management) = {
        let guard = state.lock().await;
        let Some(nss) = guard.namespaces.get(&ns) else {
            return Err(BackendError::new("not connected to this namespace"));
        };
        if let Some(sb) = &nss.sb {
            return Ok(Arc::clone(sb));
        }
        (nss.conn.clone(), Arc::clone(&nss.mgmt))
    };

    let runtime = Arc::new(Mutex::new(conn.connect().await?));
    let result = {
        let mut guard = state.lock().await;
        match guard.namespaces.get_mut(&ns) {
            Some(nss) if Arc::ptr_eq(&management, &nss.mgmt) => {
                // Another task may have created this generation's runtime.
                if let Some(existing) = &nss.sb {
                    Ok(Arc::clone(existing))
                } else {
                    nss.sb = Some(Arc::clone(&runtime));
                    Ok(Arc::clone(&runtime))
                }
            }
            _ => Err(BackendError::new(
                "The namespace connection changed. Try the operation again.",
            )),
        }
    };
    if !result
        .as_ref()
        .is_ok_and(|selected| Arc::ptr_eq(selected, &runtime))
    {
        runtime.lock().await.shutdown().await;
    }
    result
}

fn mutate(
    sink: &EventSink,
    state: &SharedState,
    ns: NamespaceId,
    req: RequestId,
    op: MutationOp,
    desc: EntityDescription,
) {
    spawn_op(sink, state, ns, move |client, sink| async move {
        let path = desc.path();
        let result = apply_mutation(&client, op, desc).await.map(Some);
        log_mutation(op, &path, &result);
        sink.send(Event::Mutated {
            req,
            ns,
            op,
            path,
            result,
        });
    });
}

fn log_mutation(
    op: MutationOp,
    path: &EntityPath,
    result: &Result<Option<EntityInfo>, BackendError>,
) {
    match result {
        Ok(_) => tracing::info!("{op:?} {} '{path}'", path.kind()),
        Err(e) => tracing::error!("{op:?} {} '{path}' failed: {e}", path.kind()),
    }
}

async fn apply_mutation(
    client: &ManagementClient,
    op: MutationOp,
    desc: EntityDescription,
) -> Result<EntityInfo, BackendError> {
    let update = op == MutationOp::Updated;
    Ok(match desc {
        EntityDescription::Queue(p) => EntityInfo::Queue(if update {
            client.update_queue(&p).await?
        } else {
            client.create_queue(&p).await?
        }),
        EntityDescription::Topic(p) => EntityInfo::Topic(if update {
            client.update_topic(&p).await?
        } else {
            client.create_topic(&p).await?
        }),
        EntityDescription::Subscription(p) => EntityInfo::Subscription(if update {
            client.update_subscription(&p).await?
        } else {
            client.create_subscription(&p).await?
        }),
        EntityDescription::Rule(p) => {
            p.filter.validate().map_err(BackendError::new)?;
            if update {
                // Rules require replacement. Capture the current rule first,
                // and restore it if creation of the replacement fails.
                let original = client
                    .list_rules(&p.topic, &p.subscription)
                    .await?
                    .into_iter()
                    .find(|rule| rule.properties.name == p.name)
                    .ok_or_else(|| {
                        BackendError::new(format!(
                            "rule '{}' was not found; refresh before editing",
                            p.name
                        ))
                    })?;
                if original.properties == p {
                    EntityInfo::Rule(original)
                } else {
                    EntityInfo::Rule(
                        crate::rule_edit::replace(
                            original.properties,
                            p.clone(),
                            || client.delete_rule(&p.topic, &p.subscription, &p.name),
                            |properties| async move { client.create_rule(&properties).await },
                        )
                        .await?,
                    )
                }
            } else {
                EntityInfo::Rule(client.create_rule(&p).await?)
            }
        }
    })
}

async fn get_entity(
    client: &ManagementClient,
    path: &EntityPath,
) -> Result<EntityInfo, BackendError> {
    Ok(match path {
        EntityPath::Queue(name) => EntityInfo::Queue(client.get_queue(name).await?),
        EntityPath::Topic(name) => EntityInfo::Topic(client.get_topic(name).await?),
        EntityPath::Subscription { topic, name } => {
            EntityInfo::Subscription(client.get_subscription(topic, name).await?)
        }
        EntityPath::Rule {
            topic,
            subscription,
            name,
        } => {
            let rules = client.list_rules(topic, subscription).await?;
            let rule = rules
                .into_iter()
                .find(|r| &r.properties.name == name)
                .ok_or_else(|| BackendError::new(format!("rule '{name}' was not found")))?;
            EntityInfo::Rule(rule)
        }
    })
}

async fn delete_entity(client: &ManagementClient, path: &EntityPath) -> Result<(), BackendError> {
    match path {
        EntityPath::Queue(name) => client.delete_queue(name).await?,
        EntityPath::Topic(name) => client.delete_topic(name).await?,
        EntityPath::Subscription { topic, name } => {
            client.delete_subscription(topic, name).await?;
        }
        EntityPath::Rule {
            topic,
            subscription,
            name,
        } => client.delete_rule(topic, subscription, name).await?,
    }
    Ok(())
}

async fn export_namespace(
    client: &ManagementClient,
    path: &std::path::Path,
) -> Result<String, BackendError> {
    let export = sift_mgmt::transfer::export(client).await?;
    let json = serde_json::to_string_pretty(&export)
        .map_err(|e| BackendError::new(format!("could not serialize export: {e}")))?;
    tokio::fs::write(path, json)
        .await
        .map_err(|e| BackendError::new(format!("could not write {}: {e}", path.display())))?;
    Ok(format!(
        "exported {} entities to {}",
        export.entity_count(),
        path.display()
    ))
}

async fn import_namespace(
    client: &ManagementClient,
    path: &std::path::Path,
    overwrite: bool,
) -> Result<String, BackendError> {
    let json = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| BackendError::new(format!("could not read {}: {e}", path.display())))?;
    let export: sift_mgmt::NamespaceExport = serde_json::from_str(&json).map_err(|e| {
        BackendError::new(format!(
            "{} is not a valid sift export: {e}",
            path.display()
        ))
    })?;
    export
        .validate_version()
        .map_err(|e| BackendError::new(e.to_string()))?;
    export.validate_contents().map_err(BackendError::new)?;
    let policy = if overwrite {
        sift_mgmt::ImportPolicy::Overwrite
    } else {
        sift_mgmt::ImportPolicy::Skip
    };
    let outcome = sift_mgmt::transfer::import(client, &export, policy).await;
    for error in &outcome.errors {
        tracing::warn!("import: {error}");
    }
    Ok(format!("import finished: {outcome}"))
}

async fn connect(
    profile: &NamespaceProfile,
    secret: &sift_core::secrets::SecretString,
) -> Result<(sift_mgmt::NamespaceInfo, NamespaceState), BackendError> {
    let (client, conn) = match &profile.auth {
        AuthMethod::ConnectionString => {
            let mut conn = NamespaceConnection::parse(secret.expose())
                .map_err(|e| BackendError::new(e.to_string()))?;
            conn.transport = profile.transport;
            for warning in &conn.warnings {
                tracing::warn!("{warning}");
            }
            (
                ManagementClient::new(&conn)?,
                MessagingConnection::Sas(Box::new(conn)),
            )
        }
        AuthMethod::AzureAd { tenant_id } => {
            let endpoint = profile.endpoint.as_ref().ok_or_else(|| {
                BackendError::new("Enter a namespace host for this Entra ID profile.")
            })?;
            let endpoint = sift_core::connection::namespace_endpoint(endpoint.as_str())
                .map_err(BackendError::new)?;
            let credential = sift_mgmt::auth::azure_cli(tenant_id.clone())
                .map_err(|_| BackendError::new(sift_mgmt::auth::SIGN_IN_HELP))?;
            let client = ManagementClient::new_with_credential(&endpoint, Arc::clone(&credential))?;
            let conn = MessagingConnection::Entra {
                namespace: endpoint.host_str().unwrap_or_default().to_owned(),
                transport: profile.transport,
                credential,
            };
            (client, conn)
        }
    };
    let info = client.get_namespace_info().await?;
    Ok((
        info,
        NamespaceState {
            mgmt: Arc::new(client),
            conn,
            sb: None,
        },
    ))
}

/// Publish a validated candidate and its event in one generation-checked
/// critical section. No cancellable await follows publication, and the old
/// runtime's cleanup remains tracked through application shutdown.
async fn finish_connect(
    sink: &EventSink,
    state: &SharedState,
    req: RequestId,
    ns: NamespaceId,
    profile_name: &str,
    candidate: Result<(sift_mgmt::NamespaceInfo, NamespaceState), BackendError>,
) {
    let mut guard = state.lock().await;
    if !guard
        .pending_connections
        .get(&ns)
        .is_some_and(|(current, token)| *current == req && !token.is_cancelled())
    {
        return;
    }
    guard.pending_connections.remove(&ns);
    let result = candidate.map(|(info, namespace)| {
        if let Some(NamespaceState {
            sb: Some(runtime), ..
        }) = guard.namespaces.insert(ns, namespace)
        {
            guard.retired_cleanup.retain(|task| !task.is_finished());
            guard
                .retired_cleanup
                .push(tokio::spawn(close_runtime(runtime)));
        }
        info
    });
    match &result {
        Ok(info) => tracing::info!(namespace = %info.name, profile = %profile_name, "connected"),
        Err(error) => tracing::error!(profile = %profile_name, %error, "connection failed"),
    }
    sink.send(Event::Connected { req, ns, result });
}

async fn begin_connect(
    sink: &EventSink,
    state: &SharedState,
    req: RequestId,
    profile: NamespaceProfile,
    secret: sift_core::secrets::SecretString,
    interactive: bool,
) {
    let cancellation = CancellationToken::new();
    {
        let mut guard = state.lock().await;
        if let Some((_, previous)) = guard
            .pending_connections
            .insert(profile.id, (req, cancellation.clone()))
        {
            previous.cancel();
        }
    }
    let (sink, state) = (sink.clone(), Arc::clone(state));
    tokio::spawn(async move {
        let ns = profile.id;
        let result = tokio::select! {
            () = cancellation.cancelled() => Err(BackendError::new("Connection cancelled.")),
            result = async {
                if interactive { sign_in(&profile.auth).await?; }
                connect(&profile, &secret).await
            } => result,
        };
        finish_connect(&sink, &state, req, ns, &profile.name, result).await;
    });
}

/// Launch the Azure CLI's supported browser sign-in flow without exposing its
/// output (which may contain account or token data) to application logs.
async fn sign_in(auth: &AuthMethod) -> Result<(), BackendError> {
    let AuthMethod::AzureAd { tenant_id } = auth else {
        return Err(BackendError::new(
            "Select Microsoft Entra ID authentication first.",
        ));
    };
    sift_mgmt::auth::validate_tenant(tenant_id.as_deref()).map_err(BackendError::new)?;
    #[cfg(windows)]
    let mut command = {
        let mut command = tokio::process::Command::new("cmd");
        command.args(["/C", "az"]);
        command
    };
    #[cfg(not(windows))]
    let mut command = tokio::process::Command::new("az");
    command.args(["login", "--allow-no-subscriptions", "--output", "none"]);
    command
        .env("AZURE_CORE_LOGIN_EXPERIENCE_V2", "off")
        .env("AZURE_CORE_ENABLE_BROKER_ON_WINDOWS", "false");
    if let Some(tenant) = tenant_id {
        command.args(["--tenant", tenant]);
    }
    command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = tokio::time::timeout(std::time::Duration::from_secs(300), command.status())
        .await
        .map_err(|_| BackendError::new("Sign-in timed out. Choose Sign in to try again."))?
        .map_err(|_| {
            BackendError::new(
                "Azure CLI was not found. Install Azure CLI 2.54 or newer to sign in.",
            )
        })?;
    if !status.success() {
        return Err(BackendError::new(
            "Sign-in did not finish. Check the browser and your tenant, then try again.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink() -> (EventSink, crossbeam_channel::Receiver<Event>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (
            EventSink {
                evt_tx: tx,
                repaint: Arc::new(|| {}),
            },
            rx,
        )
    }

    fn candidate(name: &str) -> (sift_mgmt::NamespaceInfo, NamespaceState) {
        let connection = NamespaceConnection::parse(
            "Endpoint=sb://test.servicebus.windows.net/;SharedAccessKeyName=test;SharedAccessKey=dGVzdA==",
        )
        .expect("test connection parses");
        (
            sift_mgmt::NamespaceInfo {
                name: name.into(),
                ..Default::default()
            },
            NamespaceState {
                mgmt: Arc::new(ManagementClient::new(&connection).expect("client builds")),
                conn: MessagingConnection::Sas(Box::new(connection)),
                sb: None,
            },
        )
    }

    #[tokio::test]
    async fn validated_connection_is_published_with_its_matching_event() {
        let state: SharedState = Arc::default();
        let ns = NamespaceId::new_v4();
        let req = RequestId(1);
        state
            .lock()
            .await
            .pending_connections
            .insert(ns, (req, CancellationToken::new()));
        let (sink, events) = sink();
        let (info, namespace) = candidate("current");
        let management = Arc::clone(&namespace.mgmt);

        finish_connect(&sink, &state, req, ns, "test", Ok((info, namespace))).await;

        let guard = state.lock().await;
        assert!(!guard.pending_connections.contains_key(&ns));
        assert!(Arc::ptr_eq(&guard.namespaces[&ns].mgmt, &management));
        assert!(matches!(
            events.try_recv(),
            Ok(Event::Connected { req: actual_req, ns: actual_ns, result: Ok(info) })
                if actual_req == req && actual_ns == ns && info.name == "current"
        ));
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn superseded_success_cannot_publish_before_the_newest_failure() {
        let state: SharedState = Arc::default();
        let ns = NamespaceId::new_v4();
        let stale_req = RequestId(1);
        let current_req = RequestId(2);
        state
            .lock()
            .await
            .pending_connections
            .insert(ns, (current_req, CancellationToken::new()));
        let (sink, events) = sink();

        finish_connect(&sink, &state, stale_req, ns, "test", Ok(candidate("stale"))).await;
        {
            let guard = state.lock().await;
            assert!(!guard.namespaces.contains_key(&ns));
            assert_eq!(guard.pending_connections[&ns].0, current_req);
        }
        assert!(events.try_recv().is_err());

        finish_connect(
            &sink,
            &state,
            current_req,
            ns,
            "test",
            Err(BackendError::new("latest attempt failed")),
        )
        .await;
        let guard = state.lock().await;
        assert!(!guard.namespaces.contains_key(&ns));
        assert!(!guard.pending_connections.contains_key(&ns));
        assert!(matches!(
            events.try_recv(),
            Ok(Event::Connected { req, result: Err(_), .. }) if req == current_req
        ));
    }

    #[tokio::test]
    async fn cancelled_or_disconnected_attempt_cannot_replace_a_connection() {
        let state: SharedState = Arc::default();
        let ns = NamespaceId::new_v4();
        let req = RequestId(1);
        let token = CancellationToken::new();
        token.cancel();
        let (_, existing) = candidate("existing");
        let management = Arc::clone(&existing.mgmt);
        {
            let mut guard = state.lock().await;
            guard.namespaces.insert(ns, existing);
            guard.pending_connections.insert(ns, (req, token));
        }
        let (sink, events) = sink();

        finish_connect(&sink, &state, req, ns, "test", Ok(candidate("cancelled"))).await;
        assert!(Arc::ptr_eq(
            &state.lock().await.namespaces[&ns].mgmt,
            &management
        ));
        assert!(events.try_recv().is_err());

        {
            let mut guard = state.lock().await;
            guard.pending_connections.remove(&ns);
            guard.namespaces.remove(&ns);
        }
        finish_connect(
            &sink,
            &state,
            req,
            ns,
            "test",
            Ok(candidate("disconnected")),
        )
        .await;
        assert!(!state.lock().await.namespaces.contains_key(&ns));
        assert!(events.try_recv().is_err());
    }
}
