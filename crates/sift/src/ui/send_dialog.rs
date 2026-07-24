//! Compose-and-send dialog, also used prefilled for resend/resubmit.

use sift_backend::{EntityPath, NamespaceId};
use sift_core::message::OutboundMessage;

use crate::icons::{Icon, icon};

#[derive(Debug)]
pub struct SendDialog {
    /// Namespace the message will be sent to.
    pub ns: NamespaceId,
    pub target: EntityPath,
    pub body: String,
    /// Original bytes when resending a non-text body verbatim.
    pub raw_bytes: Option<Vec<u8>>,
    pub message_id: String,
    pub subject: String,
    pub content_type: String,
    pub correlation_id: String,
    pub session_id: String,
    /// Seconds; empty = entity default.
    pub ttl_seconds: String,
    pub properties: Vec<(String, String)>,
    pub repeat: u32,
    /// Minutes from now to schedule delivery; empty/0 sends immediately.
    pub schedule_in_minutes: String,
    pub error: Option<String>,
}

impl SendDialog {
    #[must_use]
    pub fn new(ns: NamespaceId, target: EntityPath) -> Self {
        Self {
            ns,
            target,
            body: String::new(),
            raw_bytes: None,
            message_id: String::new(),
            subject: String::new(),
            content_type: String::new(),
            correlation_id: String::new(),
            session_id: String::new(),
            ttl_seconds: String::new(),
            properties: Vec::new(),
            repeat: 1,
            schedule_in_minutes: String::new(),
            error: None,
        }
    }

    /// When set, delivery is scheduled this many minutes from now. Returns an
    /// error string if the field is present but not a positive number.
    pub fn schedule_minutes(&self) -> Result<Option<i64>, String> {
        match self.schedule_in_minutes.trim() {
            "" | "0" => Ok(None),
            text => text
                .parse::<i64>()
                .ok()
                .filter(|m| *m > 0)
                .map(Some)
                .ok_or_else(|| "Schedule must be a positive number of minutes.".to_owned()),
        }
    }

    #[must_use]
    pub fn prefilled(ns: NamespaceId, target: EntityPath, from: OutboundMessage) -> Self {
        Self {
            body: from.body,
            raw_bytes: from.raw_bytes,
            subject: from.subject.unwrap_or_default(),
            content_type: from.content_type.unwrap_or_default(),
            correlation_id: from.correlation_id.unwrap_or_default(),
            session_id: from.session_id.unwrap_or_default(),
            ttl_seconds: from
                .time_to_live
                .map(|d| d.as_secs().to_string())
                .unwrap_or_default(),
            properties: from.application_properties,
            ..Self::new(ns, target)
        }
    }

    /// Validate into `repeat` outbound messages; sets `error` on failure.
    pub fn build(&mut self) -> Option<Vec<OutboundMessage>> {
        let ttl = match self.ttl_seconds.trim() {
            "" => None,
            text => match text.parse::<u64>() {
                Ok(secs) if secs > 0 => Some(std::time::Duration::from_secs(secs)),
                _ => {
                    self.error = Some("TTL must be a positive number of seconds.".into());
                    return None;
                }
            },
        };
        let non_empty = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_owned())
        };

        let template = OutboundMessage {
            body: self.body.clone(),
            raw_bytes: self.raw_bytes.clone(),
            message_id: non_empty(&self.message_id),
            subject: non_empty(&self.subject),
            content_type: non_empty(&self.content_type),
            correlation_id: non_empty(&self.correlation_id),
            session_id: non_empty(&self.session_id),
            to: None,
            reply_to: None,
            time_to_live: ttl,
            application_properties: self
                .properties
                .iter()
                .filter(|(k, _)| !k.trim().is_empty())
                .map(|(k, v)| (k.trim().to_owned(), v.clone()))
                .collect(),
        };
        Some(vec![template; self.repeat as usize])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAction {
    Send,
    Close,
}

#[allow(clippy::too_many_lines)] // a form with many fields; splitting adds indirection
pub fn show(ctx: &egui::Context, dialog: &mut SendDialog) -> Option<SendAction> {
    let mut action = None;

    let modal = egui::Modal::new(egui::Id::new("send-message")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading(format!(
            "Send to {} '{}'",
            dialog.target.kind(),
            dialog.target
        ));
        ui.add_space(8.0);

        if dialog.raw_bytes.is_some() {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Resending the original binary body unchanged.").weak(),
                );
                if ui.small_button("Discard and edit as text").clicked() {
                    dialog.raw_bytes = None;
                }
            });
        } else {
            // Cap the editor height so a large payload scrolls in place
            // rather than growing the dialog off-screen.
            let max_body = (ctx.content_rect().height() * 0.4).clamp(140.0, 420.0);
            egui::ScrollArea::vertical()
                .id_salt("send-body")
                .max_height(max_body)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut dialog.body)
                            .code_editor()
                            .hint_text("message body")
                            .desired_rows(8)
                            .desired_width(f32::INFINITY),
                    );
                });
        }
        ui.add_space(8.0);

        egui::Grid::new("send-fields")
            .num_columns(4)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                let field = |ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str| {
                    ui.label(label);
                    ui.add(
                        egui::TextEdit::singleline(value)
                            .hint_text(hint)
                            .desired_width(160.0),
                    );
                };
                field(ui, "Message id", &mut dialog.message_id, "auto");
                field(ui, "Subject", &mut dialog.subject, "");
                ui.end_row();
                field(
                    ui,
                    "Content type",
                    &mut dialog.content_type,
                    "e.g. application/json",
                );
                field(ui, "Correlation id", &mut dialog.correlation_id, "");
                ui.end_row();
                field(
                    ui,
                    "Session id",
                    &mut dialog.session_id,
                    "required for session entities",
                );
                field(ui, "TTL (s)", &mut dialog.ttl_seconds, "entity default");
                ui.end_row();
                field(
                    ui,
                    "Schedule (min)",
                    &mut dialog.schedule_in_minutes,
                    "send now if blank",
                );
                ui.end_row();
            });

        ui.add_space(4.0);
        ui.label(egui::RichText::new("Custom properties").weak());
        let mut remove: Option<usize> = None;
        for (i, (key, value)) in dialog.properties.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(key)
                        .hint_text("key")
                        .desired_width(180.0),
                );
                ui.add(
                    egui::TextEdit::singleline(value)
                        .hint_text("value")
                        .desired_width(240.0),
                );
                if ui.small_button(icon(Icon::X)).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            dialog.properties.remove(i);
        }
        if ui.button("Add property").clicked() {
            dialog.properties.push((String::new(), String::new()));
        }

        if let Some(error) = &dialog.error {
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.add_space(12.0);

        ui.horizontal(|ui| {
            let send = format!("{} Send", icon(Icon::Send));
            if ui.button(send).clicked() {
                action = Some(SendAction::Send);
            }
            ui.add(
                egui::DragValue::new(&mut dialog.repeat)
                    .range(1..=1000)
                    .prefix("copies: "),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    action = Some(SendAction::Close);
                }
            });
        });
    });

    if modal.should_close() && action.is_none() {
        action = Some(SendAction::Close);
    }
    action
}
