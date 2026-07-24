//! The eframe application: owns all UI state, drains backend events at the
//! top of each frame, and renders the panel layout.

use std::collections::HashMap;
use std::sync::Arc;

use egui_dock::{DockArea, DockState};
use egui_toast::{Toast, ToastKind, ToastOptions, Toasts};
use sift_backend::{
    BackendHandle, Command, EntityDescription, EntityInfo, EntityPath, Event, MutationOp,
};
use sift_core::config::{AppConfig, NamespaceProfile, ThemePreference};
use sift_core::connection::NamespaceConnection;
use sift_core::secrets::{SecretKind, SecretRef, SecretStore, SecretString};
use uuid::Uuid;

use crate::logging::LogBuffer;
use crate::state::{AppAction, ConnectionState, EntityTree, Loadable, PendingConnect};
use crate::ui::connect_dialog::{ConnectDialog, DialogAction};
use crate::ui::dialogs::{ConfirmDeleteAction, ConfirmDeleteDialog, CreateAction, CreateDialog};
use crate::ui::tabs::{TabId, TabViewerCtx};
use crate::ui::{connect_dialog, dialogs, log_panel, tree_panel};

pub struct SiftApp {
    backend: BackendHandle,
    evt_rx: crossbeam_channel::Receiver<Event>,
    config: AppConfig,
    secrets: Box<dyn SecretStore>,
    conn: ConnectionState,
    pending_connect: Option<PendingConnect>,
    tree: EntityTree,
    open_entities: HashMap<EntityPath, Loadable<EntityInfo>>,
    dock: DockState<TabId>,
    log: LogBuffer,
    log_visible: bool,
    connect_dialog: Option<ConnectDialog>,
    confirm_delete: Option<ConfirmDeleteDialog>,
    create_dialog: Option<CreateDialog>,
    about_open: bool,
    toasts: Toasts,
}

impl SiftApp {
    pub fn new(cc: &eframe::CreationContext<'_>, log: LogBuffer) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        cc.egui_ctx.set_fonts(fonts);

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
            dock: DockState::new(vec![TabId::Welcome]),
            log,
            log_visible: true,
            connect_dialog: None,
            confirm_delete: None,
            create_dialog: None,
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
                    self.close_entity_tabs();
                }
            }
            Event::Queues { result, .. } => {
                self.tree.queues = load_result(result, &mut self.toasts);
            }
            Event::Topics { result, .. } => {
                self.tree.topics = load_result(result, &mut self.toasts);
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
                let state = match result {
                    Ok(info) => Loadable::Loaded(info),
                    Err(e) => Loadable::Failed(e.message),
                };
                self.open_entities.insert(path, state);
            }
            Event::Mutated {
                op, path, result, ..
            } => self.apply_mutation_event(op, path, result),
        }
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
                            self.open_entities
                                .insert(path.clone(), Loadable::Loaded(info));
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
            AppAction::OpenEntity(path) => self.open_entity_tab(path),
            AppAction::RefreshEntity(path) => {
                if let Some(ns) = ns {
                    self.open_entities.insert(path.clone(), Loadable::Loading);
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
                self.confirm_delete = Some(ConfirmDeleteDialog::new(
                    path,
                    self.config.ui.confirm_delete_typed_name,
                ));
            }
        }
    }

    fn open_entity_tab(&mut self, path: EntityPath) {
        let tab = TabId::Entity(path.clone());
        if let Some(location) = self.dock.find_tab(&tab) {
            if let Err(e) = self.dock.set_active_tab(location) {
                tracing::debug!("could not focus tab: {e:?}");
            }
        } else {
            self.open_entities.entry(path).or_default();
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
            .set_title("Select a legacy explorer tool config")
            .add_filter("Config files", &["config", "xml"]);
        if let Some(default) = sift_core::legacy_import::default_user_config_path()
            && let Some(dir) = default.parent()
        {
            picker = picker.set_directory(dir);
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
                tracing::info!("imported legacy explorer tool profiles: {report}");
                self.toast(ToastKind::Success, format!("Import finished: {report}"));
            }
            Err(e) => {
                tracing::error!("import failed: {e}");
                self.toast(ToastKind::Error, e.to_string());
            }
        }
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
                if ui.button("Import from legacy explorer tool…").clicked() {
                    actions.push(AppAction::ImportLegacyProfiles);
                    ui.close();
                }
                ui.separator();
                if ui.button("Exit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
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

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.about_open;
        egui::Window::new("About sift")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("sift v{}", env!("CARGO_PKG_VERSION")));
                ui.label("A cross-platform Azure Service Bus explorer.");
                ui.label(
                    egui::RichText::new("Rust rewrite of legacy explorer tool, built on egui.")
                        .weak(),
                );
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

        if let Some(mut dialog) = self.confirm_delete.take() {
            match dialogs::show_confirm_delete(ctx, &mut dialog) {
                Some(ConfirmDeleteAction::Delete) => {
                    if let Some(ns) = self.namespace_id() {
                        let req = self.backend.next_request();
                        self.backend.send(Command::DeleteEntity {
                            req,
                            ns,
                            path: dialog.path,
                        });
                    }
                }
                Some(ConfirmDeleteAction::Close) => {}
                None => self.confirm_delete = Some(dialog),
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

        if self.about_open {
            self.about_window(ctx);
        }
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

        egui::Panel::top("menubar").show(ui, |ui| {
            self.menu_bar(ui, &mut actions);
        });
        egui::Panel::bottom("statusbar").show(ui, |ui| {
            self.status_bar(ui);
        });
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
            .min_size(180.0)
            .show(ui, |ui| {
                tree_panel::show(ui, &self.conn, &self.tree, &mut actions);
            });

        // The dock fills the remaining central space.
        let mut viewer = TabViewerCtx {
            conn: &self.conn,
            entities: &self.open_entities,
            actions: &mut actions,
        };
        DockArea::new(&mut self.dock).show_inside(ui, &mut viewer);

        self.show_dialogs(&ctx);

        for action in actions {
            self.run_action(action);
        }
        self.toasts.show(ui);
    }
}
