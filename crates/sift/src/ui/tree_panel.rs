//! Left-hand panel: connection header and (from Phase 1) the entity tree.

use crate::state::{AppAction, ConnectionState};

pub fn show(ui: &mut egui::Ui, conn: &ConnectionState, actions: &mut Vec<AppAction>) {
    match conn {
        ConnectionState::Disconnected => {
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Not connected").weak());
                ui.add_space(4.0);
                let label = format!("{} Connect…", egui_phosphor::regular::PLUGS);
                if ui.button(label).clicked() {
                    actions.push(AppAction::OpenConnectDialog);
                }
            });
        }
        ConnectionState::Connecting { name, .. } => {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Connecting to {name}…"));
            });
        }
        ConnectionState::Connected { name, info, .. } => {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(name).strong());
                if ui
                    .small_button(egui_phosphor::regular::PLUGS)
                    .on_hover_text("Disconnect")
                    .clicked()
                {
                    actions.push(AppAction::Disconnect);
                }
            });
            let mut subtitle = info.name.clone();
            if let Some(sku) = &info.messaging_sku {
                subtitle.push_str(" · ");
                subtitle.push_str(sku);
            }
            if let Some(kind) = &info.namespace_type {
                subtitle.push_str(" · ");
                subtitle.push_str(kind);
            }
            ui.label(egui::RichText::new(subtitle).weak().small());
            ui.separator();

            // Placeholder folders until entity listing lands in Phase 1.
            for (icon, title) in [
                (egui_phosphor::regular::TRAY, "Queues"),
                (egui_phosphor::regular::BROADCAST, "Topics"),
                (egui_phosphor::regular::LIGHTNING, "Event Hubs"),
                (egui_phosphor::regular::ARROWS_LEFT_RIGHT, "Relays"),
                (egui_phosphor::regular::BELL, "Notification Hubs"),
            ] {
                egui::CollapsingHeader::new(format!("{icon} {title}"))
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Entity listing lands in Phase 1").weak());
                    });
            }
        }
    }
}
