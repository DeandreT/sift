//! Modal dialog for managing saved namespace profiles and connecting.
//!
//! Secrets policy: the pasted connection string goes to the OS secret store
//! at save/connect time and is never written to the config file.

use sift_core::config::{AppConfig, AuthMethod, NamespaceProfile};
use sift_core::connection::TransportType;
use uuid::Uuid;

use crate::icons::{Icon, icon};

/// State of the open dialog (the dialog is open iff the app holds `Some`).
#[derive(Default)]
pub struct ConnectDialog {
    /// Currently selected saved profile, if any.
    pub selected: Option<Uuid>,
    pub name: String,
    /// Pasted connection string; empty means "use the stored secret".
    pub connection_string: String,
    pub show_secret: bool,
    /// Connect this profile automatically when the app starts.
    pub auto_connect: bool,
    pub entra: bool,
    pub namespace: String,
    pub tenant_id: String,
    pub transport: TransportType,
    pub error: Option<String>,
}

impl std::fmt::Debug for ConnectDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectDialog")
            .field("selected", &self.selected)
            .field("entra", &self.entra)
            .finish_non_exhaustive()
    }
}

impl ConnectDialog {
    #[must_use]
    pub fn from_profile(profile: &NamespaceProfile) -> Self {
        let mut dialog = Self::for_profile(profile.id, profile.name.clone(), profile.auto_connect);
        if let AuthMethod::AzureAd { tenant_id } = &profile.auth {
            dialog.entra = true;
            dialog.tenant_id = tenant_id.clone().unwrap_or_default();
        }
        profile
            .endpoint
            .as_ref()
            .and_then(|endpoint| endpoint.host_str())
            .unwrap_or_default()
            .clone_into(&mut dialog.namespace);
        dialog.transport = profile.transport;
        dialog
    }
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
    SignIn,
    Delete(Uuid),
    Close,
}

#[allow(clippy::too_many_lines)] // Saved profiles, authentication fields, and actions share one modal.
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
                        *dialog = ConnectDialog::from_profile(profile);
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

                ui.label("Authentication");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut dialog.entra, false, "SAS connection string");
                    ui.selectable_value(&mut dialog.entra, true, "Microsoft Entra ID");
                });
                ui.end_row();

                if dialog.entra {
                    ui.label("Namespace");
                    ui.add(egui::TextEdit::singleline(&mut dialog.namespace).hint_text("orders.servicebus.windows.net").desired_width(f32::INFINITY));
                    ui.end_row();
                    ui.label("Tenant");
                    ui.add(egui::TextEdit::singleline(&mut dialog.tenant_id).hint_text("Tenant ID or domain; blank uses the signed-in account").desired_width(f32::INFINITY));
                    ui.end_row();
                    ui.label("Transport");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut dialog.transport, TransportType::AmqpTcp, "AMQP TCP");
                        ui.selectable_value(&mut dialog.transport, TransportType::AmqpWebSockets, "AMQP WebSockets");
                    });
                    ui.end_row();
                } else {
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
                }
            });

        if dialog.entra {
            ui.add_space(6.0);
            ui.label("Connect uses your existing Azure sign-in. Sign in opens your browser. Azure CLI 2.54 or newer is required.");
        }
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
            if dialog.entra && ui.button("Sign in and connect").clicked() {
                action = Some(DialogAction::SignIn);
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
