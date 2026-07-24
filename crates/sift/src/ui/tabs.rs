//! The dockable tab system. Tabs are identified by [`TabId`]; entity tab data
//! lives in the app's `open_entities` map, keyed by entity path.

use std::collections::HashMap;

use sift_backend::{EntityPath, MessageSource};

use crate::state::{AppAction, ConnectionState, EntityPage, EntityTabState, Loadable};
use crate::ui::{entity_view, messages_view};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TabId {
    Welcome,
    Entity(EntityPath),
}

/// Borrowed view of app state handed to the dock each frame.
pub struct TabViewerCtx<'a> {
    pub conn: &'a ConnectionState,
    pub entities: &'a mut HashMap<EntityPath, EntityTabState>,
    pub peek_batch: u32,
    pub actions: &'a mut Vec<AppAction>,
}

impl egui_dock::TabViewer for TabViewerCtx<'_> {
    type Tab = TabId;

    fn title(&mut self, tab: &mut TabId) -> egui::WidgetText {
        match tab {
            TabId::Welcome => "Welcome".into(),
            TabId::Entity(path) => path.name().into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut TabId) {
        match tab {
            TabId::Welcome => self.welcome(ui),
            TabId::Entity(path) => self.entity(ui, path),
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
            match self.conn {
                ConnectionState::Disconnected => {
                    let label =
                        format!("{} Connect to a namespace…", egui_phosphor::regular::PLUGS);
                    if ui.button(label).clicked() {
                        self.actions.push(AppAction::OpenConnectDialog);
                    }
                }
                ConnectionState::Connecting { name, .. } => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Connecting to {name}…"));
                    });
                }
                ConnectionState::Connected { info, .. } => {
                    ui.label(format!("Connected to {}.", info.name));
                    ui.label(
                        egui::RichText::new(
                            "Browse entities in the tree on the left; click one to open it here.",
                        )
                        .weak(),
                    );
                }
            }
        });
    }

    fn entity(&mut self, ui: &mut egui::Ui, path: &EntityPath) {
        if !matches!(self.conn, ConnectionState::Connected { .. }) {
            ui.add_space(16.0);
            ui.label(egui::RichText::new("Not connected.").weak());
            return;
        }
        let state = self
            .entities
            .entry(path.clone())
            .or_insert_with(|| EntityTabState::new(self.peek_batch));

        // Queues and subscriptions get message-browsing pages.
        if matches!(path, EntityPath::Queue(_) | EntityPath::Subscription { .. }) {
            let dlq_count = match &state.info {
                Loadable::Loaded(sift_backend::EntityInfo::Queue(q)) => {
                    Some(q.runtime.count_details.dead_letter)
                }
                Loadable::Loaded(sift_backend::EntityInfo::Subscription(s)) => {
                    Some(s.runtime.count_details.dead_letter)
                }
                _ => None,
            };
            ui.horizontal(|ui| {
                ui.selectable_value(&mut state.page, EntityPage::Overview, "Overview");
                ui.selectable_value(&mut state.page, EntityPage::Messages, "Messages");
                let dlq_label = dlq_count.map_or_else(
                    || "Dead-letter".to_owned(),
                    |n| format!("Dead-letter ({n})"),
                );
                ui.selectable_value(&mut state.page, EntityPage::DeadLetter, dlq_label);
            });
            ui.separator();
        }

        match state.page {
            EntityPage::Overview => Self::overview(ui, path, state, self.actions),
            EntityPage::Messages => {
                let source = MessageSource {
                    entity: path.clone(),
                    dead_letter: false,
                };
                messages_view::show(ui, &source, &mut state.main, self.actions);
            }
            EntityPage::DeadLetter => {
                let source = MessageSource {
                    entity: path.clone(),
                    dead_letter: true,
                };
                messages_view::show(ui, &source, &mut state.dead_letter, self.actions);
            }
        }
    }

    fn overview(
        ui: &mut egui::Ui,
        path: &EntityPath,
        state: &EntityTabState,
        actions: &mut Vec<AppAction>,
    ) {
        match &state.info {
            Loadable::NotLoaded => {
                actions.push(AppAction::RefreshEntity(path.clone()));
            }
            Loadable::Loading => {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.spinner();
                    ui.label(format!("Loading {}…", path.name()));
                });
            }
            Loadable::Failed(error) => {
                ui.add_space(16.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
                if ui.button("Retry").clicked() {
                    actions.push(AppAction::RefreshEntity(path.clone()));
                }
            }
            Loadable::Loaded(info) => {
                entity_view::show(ui, info, actions);
            }
        }
    }
}
