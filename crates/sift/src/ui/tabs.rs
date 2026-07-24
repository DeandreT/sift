//! The dockable tab system. Tabs are identified by [`TabId`]; entity tab data
//! lives in the app's `open_entities` map, keyed by [`ScopedEntity`].

use std::collections::HashMap;

use sift_backend::{EntityPath, MessageSource};

use crate::icons::{Icon, icon};
use crate::state::{
    AppAction, Connection, DashboardState, EntityPage, EntityTabState, Loadable, ScopedEntity,
};
use crate::ui::{dashboard, entity_view, messages_view, sessions_view};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TabId {
    Welcome,
    Dashboard,
    Entity(ScopedEntity),
}

/// Borrowed view of app state handed to the dock each frame.
pub struct TabViewerCtx<'a> {
    pub connections: &'a [Connection],
    pub dashboard: &'a mut DashboardState,
    pub entities: &'a mut HashMap<ScopedEntity, EntityTabState>,
    pub peek_batch: u32,
    pub actions: &'a mut Vec<AppAction>,
}

impl egui_dock::TabViewer for TabViewerCtx<'_> {
    type Tab = TabId;

    fn title(&mut self, tab: &mut TabId) -> egui::WidgetText {
        match tab {
            TabId::Welcome => "Welcome".into(),
            TabId::Dashboard => "Dashboard".into(),
            // Disambiguate same-named entities across connections when more
            // than one is open.
            TabId::Entity(scoped) => {
                if self.connections.len() > 1
                    && let Some(conn) = self.connections.iter().find(|c| c.profile_id == scoped.ns)
                {
                    format!("{} · {}", scoped.path.name(), conn.name).into()
                } else {
                    scoped.path.name().into()
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut TabId) {
        match tab {
            TabId::Welcome => self.welcome(ui),
            TabId::Dashboard => {
                dashboard::show(ui, self.connections, self.dashboard, self.actions);
            }
            TabId::Entity(scoped) => {
                let connected = self
                    .connections
                    .iter()
                    .any(|c| c.profile_id == scoped.ns && c.is_connected());
                render_entity(
                    ui,
                    connected,
                    self.entities,
                    self.peek_batch,
                    scoped,
                    false,
                    self.actions,
                );
            }
        }
    }

    fn closeable(&mut self, tab: &mut TabId) -> bool {
        !matches!(tab, TabId::Welcome)
    }
}

impl TabViewerCtx<'_> {
    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.heading("sift");
            ui.label(egui::RichText::new("Azure Service Bus explorer").weak());
            ui.add_space(16.0);
            if self.connections.is_empty() {
                let label = format!("{} Connect to a namespace…", icon(Icon::Plug));
                if ui.button(label).clicked() {
                    self.actions.push(AppAction::OpenConnectDialog);
                }
            } else {
                let names: Vec<&str> = self.connections.iter().map(|c| c.name.as_str()).collect();
                ui.label(format!("Connected to {}.", names.join(", ")));
                ui.label(
                    egui::RichText::new(
                        "Browse entities in the tree on the left; click one to open it here.",
                    )
                    .weak(),
                );
            }
        });
    }
}

/// Render one entity's content (overview + message pages). Shared by the
/// docked tab and by a popped-out viewport, so `popped` selects whether the
/// toolbar offers "Pop out" (dock → window) or "Dock" (window → dock).
pub fn render_entity(
    ui: &mut egui::Ui,
    connected: bool,
    entities: &mut HashMap<ScopedEntity, EntityTabState>,
    peek_batch: u32,
    scoped: &ScopedEntity,
    popped: bool,
    actions: &mut Vec<AppAction>,
) {
    if !connected {
        ui.add_space(16.0);
        ui.label(egui::RichText::new("Not connected.").weak());
        return;
    }
    // Salt the ID space per entity so two tabs visible at once (dock splits,
    // pop-outs) don't clash on fixed widget IDs inside the viewers.
    ui.push_id(scoped, |ui| {
        render_entity_inner(ui, entities, peek_batch, scoped, popped, actions);
    });
}

fn render_entity_inner(
    ui: &mut egui::Ui,
    entities: &mut HashMap<ScopedEntity, EntityTabState>,
    peek_batch: u32,
    scoped: &ScopedEntity,
    popped: bool,
    actions: &mut Vec<AppAction>,
) {
    let ns = scoped.ns;
    let path = &scoped.path;
    let state = entities
        .entry(scoped.clone())
        .or_insert_with(|| EntityTabState::new(peek_batch));

    // Top row: page selector (message-capable entities) plus a pop-out /
    // dock toggle pinned to the right.
    let message_pages = matches!(path, EntityPath::Queue(_) | EntityPath::Subscription { .. });
    let requires_session = match &state.info {
        Loadable::Loaded(sift_backend::EntityInfo::Queue(q)) => q.properties.requires_session,
        Loadable::Loaded(sift_backend::EntityInfo::Subscription(s)) => {
            s.properties.requires_session
        }
        _ => false,
    };
    ui.horizontal(|ui| {
        if message_pages {
            let dlq_count = match &state.info {
                Loadable::Loaded(sift_backend::EntityInfo::Queue(q)) => {
                    Some(q.runtime.count_details.dead_letter)
                }
                Loadable::Loaded(sift_backend::EntityInfo::Subscription(s)) => {
                    Some(s.runtime.count_details.dead_letter)
                }
                _ => None,
            };
            ui.selectable_value(&mut state.page, EntityPage::Overview, "Overview");
            ui.selectable_value(&mut state.page, EntityPage::Messages, "Messages");
            let dlq_label = dlq_count.map_or_else(
                || "Dead-letter".to_owned(),
                |n| format!("Dead-letter ({n})"),
            );
            ui.selectable_value(&mut state.page, EntityPage::DeadLetter, dlq_label);
            if requires_session {
                ui.selectable_value(&mut state.page, EntityPage::Sessions, "Sessions");
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if popped {
                if ui
                    .button(format!("{} Dock", icon(Icon::ArrowDownToLine)))
                    .on_hover_text("Return this window to the main frame")
                    .clicked()
                {
                    actions.push(AppAction::DockEntity(scoped.clone()));
                }
            } else if ui
                .button(format!("{} Pop out", icon(Icon::ExternalLink)))
                .on_hover_text("Detach into a separate window")
                .clicked()
            {
                actions.push(AppAction::PopOutEntity(scoped.clone()));
            }
        });
    });
    ui.separator();

    match state.page {
        EntityPage::Overview => overview(ui, scoped, state, actions),
        EntityPage::Messages => {
            let source = MessageSource {
                entity: path.clone(),
                dead_letter: false,
            };
            messages_view::show(ui, ns, &source, &mut state.main, actions);
        }
        EntityPage::DeadLetter => {
            let source = MessageSource {
                entity: path.clone(),
                dead_letter: true,
            };
            messages_view::show(ui, ns, &source, &mut state.dead_letter, actions);
        }
        EntityPage::Sessions => {
            let source = MessageSource {
                entity: path.clone(),
                dead_letter: false,
            };
            sessions_view::show(ui, ns, &source, &mut state.sessions, actions);
        }
    }
}

fn overview(
    ui: &mut egui::Ui,
    scoped: &ScopedEntity,
    state: &EntityTabState,
    actions: &mut Vec<AppAction>,
) {
    match &state.info {
        Loadable::NotLoaded => {
            actions.push(AppAction::RefreshEntity(scoped.clone()));
        }
        Loadable::Loading => {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.label(format!("Loading {}…", scoped.path.name()));
            });
        }
        Loadable::Failed(error) => {
            ui.add_space(16.0);
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button("Retry").clicked() {
                actions.push(AppAction::RefreshEntity(scoped.clone()));
            }
        }
        Loadable::Loaded(info) => {
            entity_view::show(ui, scoped.ns, info, actions);
        }
    }
}
