//! The dockable tab system. Tabs are identified by [`TabId`]; per-tab state
//! will live in the app once entity tabs arrive in Phase 1.

use crate::state::{AppAction, ConnectionState};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TabId {
    Welcome,
}

/// Borrowed view of app state handed to the dock each frame.
pub struct TabViewerCtx<'a> {
    pub conn: &'a ConnectionState,
    pub actions: &'a mut Vec<AppAction>,
}

impl egui_dock::TabViewer for TabViewerCtx<'_> {
    type Tab = TabId;

    fn title(&mut self, tab: &mut TabId) -> egui::WidgetText {
        match tab {
            TabId::Welcome => "Welcome".into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut TabId) {
        match tab {
            TabId::Welcome => self.welcome(ui),
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
                            "Entity browsing arrives in Phase 1 — watch the tree on the left.",
                        )
                        .weak(),
                    );
                }
            }
        });
    }
}
