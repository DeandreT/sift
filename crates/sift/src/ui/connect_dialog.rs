//! Modal dialog for managing saved namespace profiles and connecting.
//!
//! Secrets policy: the pasted connection string goes to the OS secret store
//! at save/connect time and is never written to the config file.

use sift_core::config::AppConfig;
use uuid::Uuid;

use crate::icons::{Icon, icon};

/// State of the open dialog (the dialog is open iff the app holds `Some`).
#[derive(Debug, Default)]
pub struct ConnectDialog {
    /// Currently selected saved profile, if any.
    pub selected: Option<Uuid>,
    pub name: String,
    /// Pasted connection string; empty means "use the stored secret".
    pub connection_string: String,
    pub show_secret: bool,
    /// Connect this profile automatically when the app starts.
    pub auto_connect: bool,
    pub error: Option<String>,
}

impl ConnectDialog {
    #[must_use]
    pub fn for_profile(id: Uuid, name: String, auto_connect: bool) -> Self {
        Self {
            selected: Some(id),
            name,
            auto_connect,
            ..Self::default()
        }
    }
}

/// What the user asked the dialog to do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogAction {
    Save,
    Connect,
    Delete(Uuid),
    Close,
}

pub fn show(
    ctx: &egui::Context,
    dialog: &mut ConnectDialog,
    config: &AppConfig,
) -> Option<DialogAction> {
    let mut action = None;

    let modal = egui::Modal::new(egui::Id::new("connect-dialog")).show(ctx, |ui| {
        ui.set_width(480.0);
        ui.heading("Connect to a namespace");
        ui.add_space(8.0);

        // Saved profiles.
        let selected_label = dialog
            .selected
            .and_then(|id| config.profile(id))
            .map_or("New profile…", |p| p.name.as_str())
            .to_owned();
        egui::ComboBox::from_label("Saved profiles")
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(dialog.selected.is_none(), "New profile…")
                    .clicked()
                {
                    *dialog = ConnectDialog::default();
                }
                for profile in &config.profiles {
                    if ui
                        .selectable_label(dialog.selected == Some(profile.id), &profile.name)
                        .clicked()
                    {
                        *dialog = ConnectDialog::for_profile(
                            profile.id,
                            profile.name.clone(),
                            profile.auto_connect,
                        );
                    }
                }
            });

        if let Some(endpoint) = dialog
            .selected
            .and_then(|id| config.profile(id))
            .and_then(|p| p.endpoint.as_ref())
        {
            ui.label(egui::RichText::new(endpoint.as_str()).weak().small());
        }
        ui.add_space(8.0);

        egui::Grid::new("connect-fields")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.add(
                    egui::TextEdit::singleline(&mut dialog.name)
                        .hint_text("e.g. prod-orders")
                        .desired_width(f32::INFINITY),
                );
                ui.end_row();

                ui.label("Connection string");
                ui.vertical(|ui| {
                    let hint = if dialog.selected.is_some() {
                        "stored securely — leave blank to use the saved value"
                    } else {
                        "Endpoint=sb://…;SharedAccessKeyName=…;SharedAccessKey=…"
                    };
                    ui.add(
                        egui::TextEdit::singleline(&mut dialog.connection_string)
                            .hint_text(hint)
                            .password(!dialog.show_secret)
                            .desired_width(f32::INFINITY),
                    );
                    ui.checkbox(&mut dialog.show_secret, "Show");
                });
                ui.end_row();
            });

        ui.add_space(6.0);
        ui.checkbox(&mut dialog.auto_connect, "Connect automatically on startup")
            .on_hover_text("Open this namespace when sift launches");

        if let Some(error) = &dialog.error {
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.add_space(12.0);

        ui.horizontal(|ui| {
            let connect = format!("{} Connect", icon(Icon::Plug));
            if ui.button(connect).clicked() {
                action = Some(DialogAction::Connect);
            }
            if ui.button("Save").clicked() {
                action = Some(DialogAction::Save);
            }
            if let Some(id) = dialog.selected
                && ui.button("Delete").clicked()
            {
                action = Some(DialogAction::Delete(id));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    action = Some(DialogAction::Close);
                }
            });
        });
    });

    if modal.should_close() && action.is_none() {
        action = Some(DialogAction::Close);
    }
    action
}
