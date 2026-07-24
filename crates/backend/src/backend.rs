//! The backend runtime: a dedicated thread running tokio, processing
//! [`Command`]s and emitting [`Event`]s.

use std::collections::HashMap;
use std::sync::Arc;

use sift_core::config::AuthMethod;
use sift_core::connection::NamespaceConnection;
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
    conn: NamespaceConnection,
    /// AMQP runtime, created lazily on the first messaging operation.
    sb: Option<Arc<Mutex<SbRuntime>>>,
}

#[derive(Default)]
struct State {
    namespaces: HashMap<NamespaceId, NamespaceState>,
    /// Cancellation handles for in-flight long-running operations.
    ops: HashMap<OpId, CancellationToken>,
}

type SharedState = Arc<Mutex<State>>;

#[allow(clippy::too_many_lines)] // one match arm per command; splitting hurts readability
async fn run(mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Command>, sink: EventSink) {
    let state: SharedState = Arc::default();
    tracing::debug!("backend runtime started");

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Command::Connect {
                req,
                profile,
                secret,
            } => {
                let (sink, state) = (sink.clone(), Arc::clone(&state));
                tokio::spawn(async move {
                    let ns = profile.id;
                    let result = connect(&profile.auth, &secret, &state, ns).await;
                    match &result {
                        Ok(info) => {
                            tracing::info!(namespace = %info.name, profile = %profile.name, "connected");
                        }
                        Err(e) => {
                            tracing::error!(profile = %profile.name, error = %e, "connection failed");
                        }
                    }
                    sink.send(Event::Connected { req, ns, result });
                });
            }
            Command::Disconnect { ns } => {
                let removed = state.lock().await.namespaces.remove(&ns);
                if let Some(NamespaceState { sb: Some(sb), .. }) = removed {
                    tokio::spawn(async move { sb.lock().await.shutdown().await });
                }
                tracing::info!(%ns, "disconnected");
                sink.send(Event::Disconnected { ns });
            }
            Command::ListQueues { req, ns } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = client.list_queues().await.map_err(Into::into);
                    sink.send(Event::Queues { req, result });
                });
            }
            Command::ListTopics { req, ns } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = client.list_topics().await.map_err(Into::into);
                    sink.send(Event::Topics { req, result });
                });
            }
            Command::ListSubscriptions { req, ns, topic } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = client.list_subscriptions(&topic).await.map_err(Into::into);
                    sink.send(Event::Subscriptions { req, topic, result });
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
                        topic,
                        subscription,
                        result,
                    });
                });
            }
            Command::GetEntity { req, ns, path } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = get_entity(&client, &path).await;
                    sink.send(Event::Entity { req, path, result });
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
                        source,
                        from_seq: None,
                        received: true,
                        result,
                    });
                });
            }
            Command::ExportNamespace { req, ns, path } => {
                spawn_op(&sink, &state, ns, move |client, sink| async move {
                    let result = export_namespace(&client, &path).await;
                    sink.send(Event::NamespaceTransfer { req, result });
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
                    sink.send(Event::NamespaceTransfer { req, result });
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
    tracing::debug!("backend runtime stopped");
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
        let result = run_op(&conn, op, kind, &source, target.as_ref(), &token, &sink).await;
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
            result,
            cancelled,
        });
    });
}

async fn run_op(
    conn: &NamespaceConnection,
    op: OpId,
    kind: OpKind,
    source: &MessageSource,
    target: Option<&EntityPath>,
    token: &CancellationToken,
    sink: &EventSink,
) -> Result<OpSummary, BackendError> {
    let mut rt = SbRuntime::connect(conn).await?;
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
    let conn = {
        let guard = state.lock().await;
        let Some(nss) = guard.namespaces.get(&ns) else {
            return Err(BackendError::new("not connected to this namespace"));
        };
        if let Some(sb) = &nss.sb {
            return Ok(Arc::clone(sb));
        }
        nss.conn.clone()
    };

    let runtime = Arc::new(Mutex::new(SbRuntime::connect(&conn).await?));
    let mut guard = state.lock().await;
    let Some(nss) = guard.namespaces.get_mut(&ns) else {
        return Err(BackendError::new("not connected to this namespace"));
    };
    // Another task may have connected the runtime while we awaited; prefer it.
    if let Some(existing) = &nss.sb {
        return Ok(Arc::clone(existing));
    }
    nss.sb = Some(Arc::clone(&runtime));
    Ok(runtime)
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
            if update {
                // Rules have no update: recreate.
                client
                    .delete_rule(&p.topic, &p.subscription, &p.name)
                    .await
                    .ok();
            }
            EntityInfo::Rule(client.create_rule(&p).await?)
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
    auth: &AuthMethod,
    secret: &sift_core::secrets::SecretString,
    state: &Mutex<State>,
    ns: NamespaceId,
) -> Result<sift_mgmt::NamespaceInfo, BackendError> {
    let AuthMethod::ConnectionString = auth else {
        return Err(BackendError::new(
            "Microsoft Entra ID authentication is not implemented yet",
        ));
    };

    let conn = NamespaceConnection::parse(secret.expose())
        .map_err(|e| BackendError::new(e.to_string()))?;
    for warning in &conn.warnings {
        tracing::warn!("{warning}");
    }

    let client = ManagementClient::new(&conn)?;
    let info = client.get_namespace_info().await?;
    state.lock().await.namespaces.insert(
        ns,
        NamespaceState {
            mgmt: Arc::new(client),
            conn,
            sb: None,
        },
    );
    Ok(info)
}
