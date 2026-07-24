//! The eframe application: owns all UI state, drains backend events at the
//! top of each frame, and renders the panel layout.

use std::collections::HashMap;
use std::sync::Arc;

use egui_dock::{DockArea, DockState};
use egui_toast::{Toast, ToastKind, ToastOptions, Toasts};
use sift_backend::{
    BackendHandle, Command, Disposition, EntityDescription, EntityInfo, EntityPath, Event,
    MutationOp,
};
use sift_core::config::{AppConfig, NamespaceProfile, ThemePreference};
use sift_core::connection::NamespaceConnection;
use sift_core::secrets::{SecretKind, SecretRef, SecretStore, SecretString};
use uuid::Uuid;

use crate::logging::LogBuffer;
use crate::state::{
    AppAction, AutoRefresh, ConnectionState, DashboardState, EntityTabState, EntityTree, Loadable,
    PendingConnect, RunningOp,
};
use crate::ui::connect_dialog::{ConnectDialog, DialogAction};
use crate::ui::dialogs::{
    ConfirmAction, ConfirmDialog, CreateAction, CreateDialog, PendingConfirm,
};
use crate::ui::send_dialog::{SendAction, SendDialog};
use crate::ui::tabs::{self, TabId, TabViewerCtx};
use crate::ui::{connect_dialog, dialogs, log_panel, send_dialog, tree_panel};

pub struct SiftApp {
    backend: BackendHandle,
    evt_rx: crossbeam_channel::Receiver<Event>,
    config: AppConfig,
    secrets: Box<dyn SecretStore>,
    conn: ConnectionState,
    pending_connect: Option<PendingConnect>,
    tree: EntityTree,
    open_entities: HashMap<EntityPath, EntityTabState>,
    /// Entities detached into their own OS windows (rendered as viewports).
    popped_out: Vec<EntityPath>,
    dashboard: DashboardState,
    dock: DockState<TabId>,
    log: LogBuffer,
    log_visible: bool,
    connect_dialog: Option<ConnectDialog>,
    confirm: Option<ConfirmDialog>,
    create_dialog: Option<CreateDialog>,
    send_dialog: Option<SendDialog>,
    running_ops: Vec<RunningOp>,
    about_open: bool,
    toasts: Toasts,
}

impl SiftApp {
    pub fn new(cc: &eframe::CreationContext<'_>, log: LogBuffer) -> Self {
        crate::icons::install(&cc.egui_ctx);

        let config = AppConfig::load().unwrap_or_else(|e| {
            tracing::error!("failed to load config: {e}; starting with defaults");
            AppConfig::default()
        });
        cc.egui_ctx.set_theme(match config.ui.theme {
            ThemePreference::System => egui::ThemePreference::System,
            ThemePreference::Light => egui::ThemePreference::Light,
            ThemePreference::Dark => egui::ThemePreference::Dark,
        });

        let secrets = sift_core::secrets::open_default_store();
        tracing::info!("secret store: {}", secrets.backend_name());

        let ctx = cc.egui_ctx.clone();
        let (backend, evt_rx) = sift_backend::spawn(Arc::new(move || ctx.request_repaint()));

        Self {
            backend,
            evt_rx,
            config,
            secrets,
            conn: ConnectionState::default(),
            pending_connect: None,
            tree: EntityTree::default(),
            open_entities: HashMap::new(),
            popped_out: Vec::new(),
            dashboard: DashboardState::default(),
            dock: DockState::new(vec![TabId::Welcome]),
            log,
            log_visible: true,
            connect_dialog: None,
            confirm: None,
            create_dialog: None,
            send_dialog: None,
            running_ops: Vec::new(),
            about_open: false,
            toasts: Toasts::new()
                .anchor(egui::Align2::RIGHT_BOTTOM, (-12.0, -12.0))
                .direction(egui::Direction::BottomUp),
        }
    }

    fn namespace_id(&self) -> Option<Uuid> {
        self.conn.namespace_id()
    }

    // ---- backend events -------------------------------------------------

    fn drain_events(&mut self) {
        while let Ok(event) = self.evt_rx.try_recv() {
            self.apply_event(event);
        }
    }

    #[allow(clippy::too_many_lines)] // one arm per event variant
    fn apply_event(&mut self, event: Event) {
        match event {
            Event::Connected { req, ns, result } => {
                let Some(pending) = self.pending_connect.take_if(|p| p.req == req) else {
                    tracing::debug!("ignoring stale connect response");
                    return;
                };
                match result {
                    Ok(info) => {
                        self.toast(ToastKind::Success, format!("Connected to {}", info.name));
                        self.conn = ConnectionState::Connected {
                            profile_id: ns,
                            name: pending.name,
                            info,
                        };
                        self.tree.clear();
                        self.open_entities.clear();
                        self.popped_out.clear();
                        self.running_ops.clear();
                    }
                    Err(e) => {
                        if let Some(detail) = &e.detail {
                            tracing::debug!("connect failure detail: {detail}");
                        }
                        self.toast(ToastKind::Error, e.message);
                        self.conn = ConnectionState::Disconnected;
                    }
                }
            }
            Event::Disconnected { ns } => {
                if self.namespace_id() == Some(ns) {
                    self.conn = ConnectionState::Disconnected;
                    self.tree.clear();
                    self.open_entities.clear();
                    self.popped_out.clear();
                    self.running_ops.clear();
                    self.close_entity_tabs();
                }
            }
            Event::Queues { result, .. } => {
                self.tree.queues = load_result(result, &mut self.toasts);
            }
            Event::Topics { result, .. } => {
                self.tree.topics = load_result(result, &mut self.toasts);
                // A dashboard refresh needs every subscription's counts, so
                // fan out subscription loads once the topic list is in.
                if self.dashboard.wants_subscriptions
                    && let Loadable::Loaded(topics) = &self.tree.topics
                {
                    self.dashboard.wants_subscriptions = false;
                    let names: Vec<String> =
                        topics.iter().map(|t| t.properties.name.clone()).collect();
                    for topic in names {
                        self.run_action(AppAction::LoadSubscriptions(topic));
                    }
                }
            }
            Event::Subscriptions { topic, result, .. } => {
                self.tree
                    .subscriptions
                    .insert(topic, load_result(result, &mut self.toasts));
            }
            Event::Rules {
                topic,
                subscription,
                result,
                ..
            } => {
                self.tree
                    .rules
                    .insert((topic, subscription), load_result(result, &mut self.toasts));
            }
            Event::Entity { path, result, .. } => {
                let info = match result {
                    Ok(info) => Loadable::Loaded(info),
                    Err(e) => Loadable::Failed(e.message),
                };
                self.tab_state(&path).info = info;
            }
            Event::Mutated {
                op, path, result, ..
            } => self.apply_mutation_event(op, path, result),
            Event::Messages {
                source,
                from_seq,
                result,
                ..
            } => {
                let view = self.tab_state(&source.entity).view_mut(source.dead_letter);
                view.loading = false;
                match result {
                    Ok(mut messages) => {
                        view.error = None;
                        if from_seq.is_some() {
                            view.rows.append(&mut messages);
                        } else {
                            view.rows = messages;
                            view.selected = None;
                        }
                    }
                    Err(e) => view.error = Some(e.message),
                }
            }
            Event::Settled {
                source,
                lock_token,
                disposition,
                result,
                ..
            } => match result {
                Ok(()) => {
                    let view = self.tab_state(&source.entity).view_mut(source.dead_letter);
                    if disposition == Disposition::Abandon {
                        // The message stays; our lock is just gone.
                        if let Some(row) = view
                            .rows
                            .iter_mut()
                            .find(|m| m.lock_token.as_deref() == Some(lock_token.as_str()))
                        {
                            row.lock_token = None;
                        }
                    } else {
                        view.remove_by_lock_token(&lock_token);
                    }
                    self.toast(
                        ToastKind::Success,
                        format!("{} the message", disposition.verb()),
                    );
                    // Counts changed; refresh the overview.
                    self.run_action(AppAction::RefreshEntity(source.entity));
                }
                Err(e) => self.toast(ToastKind::Error, e.message),
            },
            Event::Sent {
                target,
                count,
                result,
                ..
            } => match result {
                Ok(seqs) => {
                    let verb = if seqs.is_empty() { "Sent" } else { "Scheduled" };
                    self.toast(
                        ToastKind::Success,
                        format!("{verb} {count} message(s) to '{target}'"),
                    );
                    if self.open_entities.contains_key(&target) {
                        self.run_action(AppAction::RefreshEntity(target));
                    }
                }
                Err(e) => self.toast(ToastKind::Error, e.message),
            },
            Event::ScheduledCancelled { target, result, .. } => match result {
                Ok(()) => {
                    self.toast(ToastKind::Success, "Cancelled the scheduled message");
                    if self.open_entities.contains_key(&target) {
                        self.run_action(AppAction::RefreshEntity(target));
                    }
                }
                Err(e) => self.toast(ToastKind::Error, e.message),
            },
            Event::NamespaceTransfer { result, .. } => match result {
                Ok(summary) => {
                    tracing::info!("{summary}");
                    self.toast(ToastKind::Success, summary);
                    self.tree.clear(); // imported entities may have appeared
                }
                Err(e) => self.toast(ToastKind::Error, e.message),
            },
            Event::Session { source, result, .. } => {
                let view = &mut self.tab_state(&source.entity).sessions;
                view.loading = false;
                match result {
                    Ok(snapshot) => {
                        view.error = None;
                        view.snapshot = Some(snapshot);
                    }
                    Err(e) => view.error = Some(e.message),
                }
            }
            Event::OpProgress {
                op,
                kind,
                done,
                target,
            } => {
                if let Some(running) = self.running_ops.iter_mut().find(|o| o.op == op) {
                    running.done = done;
                } else {
                    self.running_ops.push(RunningOp {
                        op,
                        kind,
                        done,
                        target,
                    });
                }
            }
            Event::OpFinished {
                op,
                result,
                cancelled,
            } => {
                let entity = self.running_ops.iter().find(|o| o.op == op).and_then(|o| {
                    // Re-derive which entity to refresh from the target label.
                    self.open_entities
                        .keys()
                        .find(|p| o.target.starts_with(p.name()))
                        .cloned()
                });
                self.running_ops.retain(|o| o.op != op);
                match result {
                    Ok(summary) => {
                        let verb = summary.kind.verb();
                        let msg = if cancelled {
                            format!(
                                "{verb} cancelled after {} message(s) on '{}'",
                                summary.processed, summary.target
                            )
                        } else {
                            format!(
                                "{verb} finished: {} message(s) on '{}'",
                                summary.processed, summary.target
                            )
                        };
                        self.toast(ToastKind::Success, msg);
                    }
                    Err(e) => self.toast(ToastKind::Error, e.message),
                }
                if let Some(path) = entity {
                    self.run_action(AppAction::RefreshEntity(path));
                }
            }
        }
    }

    /// Tab state for an entity, created on demand.
    fn tab_state(&mut self, path: &EntityPath) -> &mut EntityTabState {
        self.open_entities
            .entry(path.clone())
            .or_insert_with(|| EntityTabState::new(self.config.ui.peek_batch))
    }

    fn apply_mutation_event(
        &mut self,
        op: MutationOp,
        path: EntityPath,
        result: Result<Option<EntityInfo>, sift_backend::BackendError>,
    ) {
        match result {
            Ok(info) => {
                let verb = match op {
                    MutationOp::Created => "Created",
                    MutationOp::Updated => "Updated",
                    MutationOp::Deleted => "Deleted",
                };
                self.toast(
                    ToastKind::Success,
                    format!("{verb} {} '{path}'", path.kind()),
                );

                match op {
                    MutationOp::Deleted => {
                        self.open_entities.remove(&path);
                        if let Some(location) = self.dock.find_tab(&TabId::Entity(path.clone())) {
                            self.dock.remove_tab(location);
                        }
                    }
                    MutationOp::Created | MutationOp::Updated => {
                        if let Some(info) = info {
                            self.tab_state(&path).info = Loadable::Loaded(info);
                        }
                    }
                }
                // Reload the list that contains the entity so the tree agrees.
                self.tree.invalidate_list_for(&path);
                self.reload_list_for(&path);
            }
            Err(e) => {
                if let Some(detail) = &e.detail {
                    tracing::debug!("mutation failure detail: {detail}");
                }
                self.toast(ToastKind::Error, e.message);
                // The service state is unknown; refresh an open detail tab.
                if self.open_entities.contains_key(&path) {
                    self.run_action(AppAction::RefreshEntity(path));
                }
            }
        }
    }

    fn reload_list_for(&mut self, path: &EntityPath) {
        let action = match path {
            EntityPath::Queue(_) => AppAction::LoadQueues,
            EntityPath::Topic(_) => AppAction::LoadTopics,
            EntityPath::Subscription { topic, .. } => AppAction::LoadSubscriptions(topic.clone()),
            EntityPath::Rule {
                topic,
                subscription,
                ..
            } => AppAction::LoadRules(topic.clone(), subscription.clone()),
        };
        self.run_action(action);
    }

    fn close_entity_tabs(&mut self) {
        let entity_tabs: Vec<TabId> = self
            .dock
            .iter_all_tabs()
            .map(|(_, tab)| tab.clone())
            .filter(|tab| matches!(tab, TabId::Entity(_)))
            .collect();
        for tab in entity_tabs {
            if let Some(location) = self.dock.find_tab(&tab) {
                self.dock.remove_tab(location);
            }
        }
    }

    // ---- actions ---------------------------------------------------------

    #[allow(clippy::too_many_lines)] // one arm per action variant
    fn run_action(&mut self, action: AppAction) {
        let ns = self.namespace_id();
        match action {
            AppAction::OpenConnectDialog => {
                if self.connect_dialog.is_none() {
                    self.connect_dialog = Some(match &self.conn {
                        ConnectionState::Connected {
                            profile_id, name, ..
                        } => ConnectDialog::for_profile(*profile_id, name.clone()),
                        _ => self
                            .config
                            .profiles
                            .first()
                            .map(|p| ConnectDialog::for_profile(p.id, p.name.clone()))
                            .unwrap_or_default(),
                    });
                }
            }
            AppAction::Disconnect => {
                if let Some(ns) = ns {
                    self.backend.send(Command::Disconnect { ns });
                }
            }
            AppAction::ImportLegacyProfiles => self.import_legacy_profiles(),
            AppAction::LoadQueues => {
                if let Some(ns) = ns {
                    self.tree.queues = Loadable::Loading;
                    let req = self.backend.next_request();
                    self.backend.send(Command::ListQueues { req, ns });
                }
            }
            AppAction::LoadTopics => {
                if let Some(ns) = ns {
                    self.tree.topics = Loadable::Loading;
                    let req = self.backend.next_request();
                    self.backend.send(Command::ListTopics { req, ns });
                }
            }
            AppAction::LoadSubscriptions(topic) => {
                if let Some(ns) = ns {
                    self.tree
                        .subscriptions
                        .insert(topic.clone(), Loadable::Loading);
                    let req = self.backend.next_request();
                    self.backend
                        .send(Command::ListSubscriptions { req, ns, topic });
                }
            }
            AppAction::LoadRules(topic, subscription) => {
                if let Some(ns) = ns {
                    self.tree
                        .rules
                        .insert((topic.clone(), subscription.clone()), Loadable::Loading);
                    let req = self.backend.next_request();
                    self.backend.send(Command::ListRules {
                        req,
                        ns,
                        topic,
                        subscription,
                    });
                }
            }
            AppAction::RefreshTree => self.tree.clear(),
            // Docking a popped-out entity is the same as opening it.
            AppAction::OpenEntity(path) | AppAction::DockEntity(path) => {
                self.open_entity_tab(&path);
            }
            AppAction::RefreshEntity(path) => {
                if let Some(ns) = ns {
                    self.tab_state(&path).info = Loadable::Loading;
                    let req = self.backend.next_request();
                    self.backend.send(Command::GetEntity { req, ns, path });
                }
            }
            AppAction::UpdateEntity(info) => {
                if let Some(ns) = ns {
                    let desc = description_of(&info);
                    let req = self.backend.next_request();
                    self.backend.send(Command::UpdateEntity { req, ns, desc });
                }
            }
            AppAction::OpenCreateDialog(kind) => {
                self.create_dialog = Some(CreateDialog::new(kind));
            }
            AppAction::RequestDelete(path) => {
                self.confirm = Some(ConfirmDialog::new(
                    PendingConfirm::Delete(path),
                    self.config.ui.confirm_delete_typed_name,
                ));
            }
            AppAction::RequestPurge(source) => {
                self.confirm = Some(ConfirmDialog::new(
                    PendingConfirm::Purge(source),
                    self.config.ui.confirm_delete_typed_name,
                ));
            }
            AppAction::ResubmitAll { source, target } => {
                if let Some(ns) = ns {
                    let op = self.backend.next_op();
                    self.backend.send(Command::StartResubmit {
                        op,
                        ns,
                        source,
                        target,
                    });
                }
            }
            AppAction::CancelOp(op) => {
                self.backend.send(Command::CancelOp(op));
            }
            AppAction::OpenDashboard => {
                let tab = TabId::Dashboard;
                if let Some(location) = self.dock.find_tab(&tab) {
                    let _ = self.dock.set_active_tab(location);
                } else {
                    self.dock.push_to_focused_leaf(tab);
                }
                self.run_action(AppAction::RefreshDashboard);
            }
            AppAction::RefreshDashboard => {
                // Load queues + topics now; subscriptions follow once topics
                // arrive (see the Topics event handler).
                self.dashboard.wants_subscriptions = true;
                self.run_action(AppAction::LoadQueues);
                self.run_action(AppAction::LoadTopics);
                self.schedule_dashboard_refresh();
            }
            AppAction::SetDashboardAutoRefresh(mode) => {
                self.dashboard.auto_refresh = mode;
                self.schedule_dashboard_refresh();
            }
            AppAction::CancelScheduled {
                target,
                sequence_number,
            } => {
                if let Some(ns) = ns {
                    let req = self.backend.next_request();
                    self.backend.send(Command::CancelScheduled {
                        req,
                        ns,
                        target,
                        sequence_number,
                    });
                }
            }
            AppAction::ReceiveDeferred {
                source,
                sequence_numbers,
            } => {
                if let Some(ns) = ns {
                    let view = self.tab_state(&source.entity).view_mut(source.dead_letter);
                    view.loading = true;
                    let req = self.backend.next_request();
                    self.backend.send(Command::ReceiveDeferred {
                        req,
                        ns,
                        source,
                        sequence_numbers,
                    });
                }
            }
            AppAction::ExportNamespace => self.export_namespace(),
            AppAction::ImportNamespace { overwrite } => self.import_namespace(overwrite),
            AppAction::BrowseSession {
                source,
                session_id,
                count,
            } => {
                if let Some(ns) = ns {
                    let view = &mut self.tab_state(&source.entity).sessions;
                    view.loading = true;
                    view.error = None;
                    let req = self.backend.next_request();
                    self.backend.send(Command::BrowseSession {
                        req,
                        ns,
                        source,
                        session_id,
                        count,
                    });
                }
            }
            AppAction::PeekMessages {
                source,
                from_seq,
                count,
            } => {
                if let Some(ns) = ns {
                    let view = self.tab_state(&source.entity).view_mut(source.dead_letter);
                    view.loading = true;
                    view.error = None;
                    let req = self.backend.next_request();
                    self.backend.send(Command::PeekMessages {
                        req,
                        ns,
                        source,
                        from_seq,
                        count,
                    });
                }
            }
            AppAction::ReceiveMessages {
                source,
                mode,
                count,
            } => {
                if let Some(ns) = ns {
                    let view = self.tab_state(&source.entity).view_mut(source.dead_letter);
                    view.loading = true;
                    view.error = None;
                    let req = self.backend.next_request();
                    self.backend.send(Command::ReceiveMessages {
                        req,
                        ns,
                        source,
                        mode,
                        count,
                    });
                }
            }
            AppAction::Settle {
                source,
                lock_token,
                disposition,
            } => {
                if let Some(ns) = ns {
                    let req = self.backend.next_request();
                    self.backend.send(Command::SettleMessage {
                        req,
                        ns,
                        source,
                        lock_token,
                        disposition,
                    });
                }
            }
            AppAction::OpenSendDialog { target, prefill } => {
                self.send_dialog = Some(match prefill {
                    Some(outbound) => SendDialog::prefilled(target, *outbound),
                    None => SendDialog::new(target),
                });
            }
            AppAction::PopOutEntity(path) => {
                if let Some(location) = self.dock.find_tab(&TabId::Entity(path.clone())) {
                    self.dock.remove_tab(location);
                }
                if !self.popped_out.contains(&path) {
                    self.popped_out.push(path);
                }
            }
        }
    }

    fn open_entity_tab(&mut self, path: &EntityPath) {
        // If it's currently a separate window, reattaching brings it back.
        self.popped_out.retain(|p| p != path);
        let tab = TabId::Entity(path.clone());
        if let Some(location) = self.dock.find_tab(&tab) {
            if let Err(e) = self.dock.set_active_tab(location) {
                tracing::debug!("could not focus tab: {e:?}");
            }
        } else {
            self.tab_state(path);
            self.dock.push_to_focused_leaf(tab);
        }
    }

    // ---- dialogs -----------------------------------------------------------

    fn run_dialog_action(&mut self, action: DialogAction) {
        match action {
            DialogAction::Close => self.connect_dialog = None,
            DialogAction::Save => {
                self.save_profile();
            }
            DialogAction::Connect => {
                if let Some((profile, secret)) = self.save_profile() {
                    self.connect_dialog = None;
                    self.start_connect(profile, secret);
                }
            }
            DialogAction::Delete(id) => {
                self.config.remove_profile(id);
                if let Err(e) = self
                    .secrets
                    .delete(&SecretRef::new(id, SecretKind::ConnectionString))
                {
                    tracing::warn!("failed to delete stored secret: {e}");
                }
                self.persist_config();
                if let Some(dialog) = &mut self.connect_dialog {
                    *dialog = ConnectDialog::default();
                }
            }
        }
    }

    /// Validate the dialog input, persist the profile + secret, and return
    /// them. On failure the error is shown inside the dialog.
    fn save_profile(&mut self) -> Option<(NamespaceProfile, SecretString)> {
        let dialog = self.connect_dialog.as_mut()?;
        let typed = dialog.connection_string.trim();

        let (secret, newly_typed) = if typed.is_empty() {
            let Some(id) = dialog.selected else {
                dialog.error = Some("Paste a connection string.".into());
                return None;
            };
            match self
                .secrets
                .get(&SecretRef::new(id, SecretKind::ConnectionString))
            {
                Ok(Some(secret)) => (secret, false),
                Ok(None) => {
                    dialog.error = Some(format!(
                        "No stored connection string ({}); paste one.",
                        self.secrets.backend_name()
                    ));
                    return None;
                }
                Err(e) => {
                    dialog.error = Some(e.to_string());
                    return None;
                }
            }
        } else {
            (SecretString::from(typed), true)
        };

        let conn = match NamespaceConnection::parse(secret.expose()) {
            Ok(conn) => conn,
            Err(e) => {
                dialog.error = Some(e.to_string());
                return None;
            }
        };

        let mut profile = dialog
            .selected
            .and_then(|id| self.config.profile(id).cloned())
            .unwrap_or_else(|| NamespaceProfile::new_connection_string(String::new()));
        profile.name = if dialog.name.trim().is_empty() {
            conn.namespace.clone()
        } else {
            dialog.name.trim().to_owned()
        };
        profile.endpoint = Some(conn.endpoint.clone());
        profile.transport = conn.transport;

        if newly_typed
            && let Err(e) = self.secrets.set(
                &SecretRef::new(profile.id, SecretKind::ConnectionString),
                &secret,
            )
        {
            dialog.error = Some(format!("Could not store the secret: {e}"));
            return None;
        }

        dialog.selected = Some(profile.id);
        profile.name.clone_into(&mut dialog.name);
        dialog.connection_string.clear();
        dialog.error = None;
        self.config.upsert_profile(profile.clone());
        self.persist_config();
        Some((profile, secret))
    }

    fn start_connect(&mut self, profile: NamespaceProfile, secret: SecretString) {
        let req = self.backend.next_request();
        self.conn = ConnectionState::Connecting {
            profile_id: profile.id,
            name: profile.name.clone(),
        };
        self.pending_connect = Some(PendingConnect {
            req,
            name: profile.name.clone(),
        });
        tracing::info!("connecting to {}…", profile.name);
        self.backend.send(Command::Connect {
            req,
            profile,
            secret,
        });
    }

    fn import_legacy_profiles(&mut self) {
        let mut picker = rfd::FileDialog::new()
            .set_title("Select a namespaces config file")
            .add_filter("Config files", &["config", "xml"]);
        if let Some(appdata) = std::env::var_os("APPDATA") {
            picker = picker.set_directory(appdata);
        }
        let Some(path) = picker.pick_file() else {
            return;
        };

        match sift_core::legacy_import::import_from_file(
            &path,
            &mut self.config,
            self.secrets.as_ref(),
        ) {
            Ok(report) => {
                for warning in &report.warnings {
                    tracing::warn!("{warning}");
                }
                for (name, reason) in &report.skipped {
                    tracing::warn!("skipped '{name}': {reason}");
                }
                self.persist_config();
                tracing::info!("imported legacy profiles: {report}");
                self.toast(ToastKind::Success, format!("Import finished: {report}"));
            }
            Err(e) => {
                tracing::error!("import failed: {e}");
                self.toast(ToastKind::Error, e.to_string());
            }
        }
    }

    fn export_namespace(&mut self) {
        let Some(ns) = self.namespace_id() else {
            return;
        };
        let default_name = match &self.conn {
            ConnectionState::Connected { info, .. } => format!("{}-export.json", info.name),
            _ => "sift-export.json".to_owned(),
        };
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export namespace entities")
            .add_filter("JSON", &["json"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        let req = self.backend.next_request();
        self.backend
            .send(Command::ExportNamespace { req, ns, path });
        self.toast(ToastKind::Info, "Exporting…");
    }

    fn import_namespace(&mut self, overwrite: bool) {
        let Some(ns) = self.namespace_id() else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import namespace entities")
            .add_filter("JSON", &["json"])
            .pick_file()
        else {
            return;
        };
        let req = self.backend.next_request();
        self.backend.send(Command::ImportNamespace {
            req,
            ns,
            path,
            overwrite,
        });
        self.toast(ToastKind::Info, "Importing…");
    }

    fn persist_config(&mut self) {
        if let Err(e) = self.config.save() {
            tracing::error!("failed to save config: {e}");
            self.toast(ToastKind::Error, format!("Failed to save config: {e}"));
        }
    }

    fn toast(&mut self, kind: ToastKind, text: impl Into<egui::WidgetText>) {
        self.toasts.add(Toast {
            kind,
            text: text.into(),
            options: ToastOptions::default()
                .duration_in_seconds(4.0)
                .show_progress(true),
            ..Default::default()
        });
    }

    // ---- layout ----------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Connect…").clicked() {
                    actions.push(AppAction::OpenConnectDialog);
                    ui.close();
                }
                let connected = matches!(self.conn, ConnectionState::Connected { .. });
                if ui
                    .add_enabled(connected, egui::Button::new("Disconnect"))
                    .clicked()
                {
                    actions.push(AppAction::Disconnect);
                    ui.close();
                }
                ui.separator();
                let connected = matches!(self.conn, ConnectionState::Connected { .. });
                if ui
                    .add_enabled(connected, egui::Button::new("Export entities…"))
                    .clicked()
                {
                    actions.push(AppAction::ExportNamespace);
                    ui.close();
                }
                ui.menu_button("Import entities", |ui| {
                    if ui
                        .add_enabled(connected, egui::Button::new("Create missing only"))
                        .clicked()
                    {
                        actions.push(AppAction::ImportNamespace { overwrite: false });
                        ui.close();
                    }
                    if ui
                        .add_enabled(connected, egui::Button::new("Create and overwrite"))
                        .clicked()
                    {
                        actions.push(AppAction::ImportNamespace { overwrite: true });
                        ui.close();
                    }
                });
                ui.separator();
                if ui.button("Import legacy profiles…").clicked() {
                    actions.push(AppAction::ImportLegacyProfiles);
                    ui.close();
                }
                ui.separator();
                if ui.button("Exit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                let connected = matches!(self.conn, ConnectionState::Connected { .. });
                if ui
                    .add_enabled(connected, egui::Button::new("Dashboard"))
                    .clicked()
                {
                    actions.push(AppAction::OpenDashboard);
                    ui.close();
                }
                if ui.checkbox(&mut self.log_visible, "Log panel").clicked() {
                    ui.close();
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About sift").clicked() {
                    self.about_open = true;
                    ui.close();
                }
            });
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let status = match &self.conn {
                ConnectionState::Disconnected => "Disconnected".to_owned(),
                ConnectionState::Connecting { name, .. } => format!("Connecting to {name}…"),
                ConnectionState::Connected { name, .. } => format!("Connected to {name}"),
            };
            ui.label(status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "secrets: {} · v{}",
                        self.secrets.backend_name(),
                        env!("CARGO_PKG_VERSION")
                    ))
                    .weak(),
                );
            });
        });
    }

    /// Validate and dispatch a send/schedule from the dialog. Returns the
    /// dialog to keep it open when validation fails, or `None` when it should
    /// close after a successful submit.
    fn submit_send_dialog(&mut self, mut dialog: SendDialog) -> Option<SendDialog> {
        let schedule = match dialog.schedule_minutes() {
            Ok(schedule) => schedule,
            Err(e) => {
                dialog.error = Some(e);
                return Some(dialog);
            }
        };
        let Some(messages) = dialog.build() else {
            return Some(dialog); // validation error shown inline
        };
        let ns = self.namespace_id()?; // not connected → close the dialog
        let target = dialog.target.clone();
        let req = self.backend.next_request();
        match schedule {
            Some(minutes) => {
                let enqueue_at = time::OffsetDateTime::now_utc() + time::Duration::minutes(minutes);
                self.backend.send(Command::ScheduleMessages {
                    req,
                    ns,
                    target,
                    messages,
                    enqueue_at,
                });
            }
            None => self.backend.send(Command::SendMessages {
                req,
                ns,
                target,
                messages,
            }),
        }
        None
    }

    /// Arm the next auto-refresh tick from the current cadence.
    fn schedule_dashboard_refresh(&mut self) {
        self.dashboard.next_refresh = self
            .dashboard
            .auto_refresh
            .interval()
            .map(|d| std::time::Instant::now() + d);
    }

    /// Handle keyboard shortcuts and time-based work (filter debounce,
    /// dashboard auto-refresh) at the top of each frame.
    fn tick(&mut self, ctx: &egui::Context, actions: &mut Vec<AppAction>) {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.tree.filter.focus_requested = true;
        }
        if self.tree.filter.tick() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }

        let dashboard_open = self.dock.find_tab(&TabId::Dashboard).is_some();
        if dashboard_open && self.dashboard.auto_refresh != AutoRefresh::Off {
            let now = std::time::Instant::now();
            match self.dashboard.next_refresh {
                Some(at) if now >= at => actions.push(AppAction::RefreshDashboard),
                Some(at) => ctx.request_repaint_after(at.saturating_duration_since(now)),
                None => self.schedule_dashboard_refresh(),
            }
        }
    }

    fn operations_strip(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        for running in &self.running_ops {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "{} '{}' — {} message(s)",
                    running.kind.verb(),
                    running.target,
                    running.done
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        actions.push(AppAction::CancelOp(running.op));
                    }
                });
            });
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.about_open;
        egui::Window::new("About sift")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("sift v{}", env!("CARGO_PKG_VERSION")));
                ui.label("A cross-platform Azure Service Bus explorer.");
                ui.label(egui::RichText::new("Built with Rust and egui.").weak());
            });
        self.about_open = open;
    }

    fn show_dialogs(&mut self, ctx: &egui::Context) {
        if let Some(mut dialog) = self.connect_dialog.take() {
            let action = connect_dialog::show(ctx, &mut dialog, &self.config);
            self.connect_dialog = Some(dialog);
            if let Some(action) = action {
                self.run_dialog_action(action);
            }
        }

        if let Some(mut dialog) = self.confirm.take() {
            match dialogs::show_confirm(ctx, &mut dialog) {
                Some(ConfirmAction::Confirm) => {
                    if let Some(ns) = self.namespace_id() {
                        match dialog.action {
                            PendingConfirm::Delete(path) => {
                                let req = self.backend.next_request();
                                self.backend.send(Command::DeleteEntity { req, ns, path });
                            }
                            PendingConfirm::Purge(source) => {
                                let op = self.backend.next_op();
                                self.backend.send(Command::StartPurge { op, ns, source });
                            }
                        }
                    }
                }
                Some(ConfirmAction::Close) => {}
                None => self.confirm = Some(dialog),
            }
        }

        if let Some(mut dialog) = self.create_dialog.take() {
            match dialogs::show_create(ctx, &mut dialog) {
                Some(CreateAction::Create) => match dialog.build() {
                    Some(desc) => {
                        if let Some(ns) = self.namespace_id() {
                            let req = self.backend.next_request();
                            self.backend.send(Command::CreateEntity { req, ns, desc });
                        }
                    }
                    None => self.create_dialog = Some(dialog), // validation error shown inline
                },
                Some(CreateAction::Close) => {}
                None => self.create_dialog = Some(dialog),
            }
        }

        if let Some(mut dialog) = self.send_dialog.take() {
            match send_dialog::show(ctx, &mut dialog) {
                Some(SendAction::Send) => {
                    self.send_dialog = self.submit_send_dialog(dialog);
                }
                Some(SendAction::Close) => {}
                None => self.send_dialog = Some(dialog),
            }
        }

        if self.about_open {
            self.about_window(ctx);
        }
    }

    /// Render each detached entity as its own OS window via an immediate
    /// viewport, so it can borrow app state directly. A native close removes
    /// the window (the entity stays cached and can be reopened from the tree).
    fn show_popped_out(&mut self, ctx: &egui::Context, actions: &mut Vec<AppAction>) {
        let popped = std::mem::take(&mut self.popped_out);
        let conn = &self.conn;
        let entities = &mut self.open_entities;
        let peek_batch = self.config.ui.peek_batch;
        let mut still_open = Vec::with_capacity(popped.len());

        for path in popped {
            let viewport_id = egui::ViewportId::from_hash_of(("sift-entity-window", &path));
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("sift — {}", path.name()))
                .with_inner_size([900.0, 640.0]);

            let closed = ctx.show_viewport_immediate(viewport_id, builder, |ui, _class| {
                tabs::render_entity(ui, conn, entities, peek_batch, &path, true, actions);
                ui.input(|i| i.viewport().close_requested())
            });
            if !closed {
                still_open.push(path);
            }
        }
        self.popped_out = still_open;
    }
}

/// Extract the user-settable description from a full entity snapshot.
fn description_of(info: &EntityInfo) -> EntityDescription {
    match info {
        EntityInfo::Queue(q) => EntityDescription::Queue(q.properties.clone()),
        EntityInfo::Topic(t) => EntityDescription::Topic(t.properties.clone()),
        EntityInfo::Subscription(s) => EntityDescription::Subscription(s.properties.clone()),
        EntityInfo::Rule(r) => EntityDescription::Rule(r.properties.clone()),
    }
}

fn load_result<T>(
    result: Result<T, sift_backend::BackendError>,
    toasts: &mut Toasts,
) -> Loadable<T> {
    match result {
        Ok(value) => Loadable::Loaded(value),
        Err(e) => {
            toasts.add(Toast {
                kind: ToastKind::Error,
                text: e.message.clone().into(),
                options: ToastOptions::default()
                    .duration_in_seconds(4.0)
                    .show_progress(true),
                ..Default::default()
            });
            Loadable::Failed(e.message)
        }
    }
}

impl eframe::App for SiftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events();

        let mut actions: Vec<AppAction> = Vec::new();
        self.tick(&ctx, &mut actions);

        egui::Panel::top("menubar").show(ui, |ui| {
            self.menu_bar(ui, &mut actions);
        });
        egui::Panel::bottom("statusbar").show(ui, |ui| {
            self.status_bar(ui);
        });
        if !self.running_ops.is_empty() {
            egui::Panel::bottom("operations").show(ui, |ui| {
                self.operations_strip(ui, &mut actions);
            });
        }
        if self.log_visible {
            egui::Panel::bottom("log")
                .resizable(true)
                .default_size(140.0)
                .min_size(60.0)
                .show(ui, |ui| {
                    log_panel::show(ui, &self.log);
                });
        }
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(280.0)
            .size_range(180.0..=600.0)
            .show(ui, |ui| {
                tree_panel::show(ui, &self.conn, &mut self.tree, &mut actions);
            });

        // The dock fills the remaining central space.
        let mut viewer = TabViewerCtx {
            conn: &self.conn,
            tree: &self.tree,
            dashboard: &mut self.dashboard,
            entities: &mut self.open_entities,
            peek_batch: self.config.ui.peek_batch,
            actions: &mut actions,
        };
        DockArea::new(&mut self.dock).show_inside(ui, &mut viewer);

        self.show_popped_out(&ctx, &mut actions);
        self.show_dialogs(&ctx);

        for action in actions {
            self.run_action(action);
        }
        self.toasts.show(ui);
    }
}
