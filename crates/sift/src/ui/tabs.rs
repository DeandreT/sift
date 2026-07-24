//! The dockable tab system. Tabs are identified by [`TabId`]; entity tab data
//! lives in the app's `open_entities` map, keyed by entity path.

use std::collections::HashMap;

use sift_backend::{EntityInfo, EntityPath};

use crate::state::{AppAction, ConnectionState, Loadable};
use crate::ui::entity_view;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TabId {
    Welcome,
    Entity(EntityPath),
}

/// Borrowed view of app state handed to the dock each frame.
pub struct TabViewerCtx<'a> {
    pub conn: &'a ConnectionState,
    pub entities: &'a HashMap<EntityPath, Loadable<EntityInfo>>,
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
        match self.entities.get(path) {
            None | Some(Loadable::NotLoaded) => {
                self.actions.push(AppAction::RefreshEntity(path.clone()));
            }
            Some(Loadable::Loading) => {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.spinner();
                    ui.label(format!("Loading {}…", path.name()));
                });
            }
            Some(Loadable::Failed(error)) => {
                ui.add_space(16.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
                if ui.button("Retry").clicked() {
                    self.actions.push(AppAction::RefreshEntity(path.clone()));
                }
            }
            Some(Loadable::Loaded(info)) => {
                entity_view::show(ui, info, self.actions);
            }
        }
    }
}
