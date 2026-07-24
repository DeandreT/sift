//! Read-only session browser: accept a session, peek its messages and read
//! its state, then release it. Settling within a held session is a future
//! enhancement.

use sift_backend::MessageSource;

use crate::icons::{Icon, icon};
use crate::state::{AppAction, SessionsView};

#[allow(clippy::too_many_lines)] // toolbar + state view + message list read best together
pub fn show(
    ui: &mut egui::Ui,
    source: &MessageSource,
    view: &mut SessionsView,
    actions: &mut Vec<AppAction>,
) {
    ui.horizontal_wrapped(|ui| {
        if view.loading {
            ui.spinner();
        } else if ui
            .button(format!("{} Accept next session", icon(Icon::MailOpen)))
            .on_hover_text("Accept the next available session and peek its messages")
            .clicked()
        {
            actions.push(AppAction::BrowseSession {
                source: source.clone(),
                session_id: None,
                count: view.fetch_count,
            });
        }
        ui.separator();
        ui.label("Session id:");
        ui.add(
            egui::TextEdit::singleline(&mut view.session_id_input)
                .hint_text("named session")
                .desired_width(160.0),
        );
        let named = view.session_id_input.trim().to_owned();
        if !view.loading && !named.is_empty() && ui.button("Accept this session").clicked() {
            actions.push(AppAction::BrowseSession {
                source: source.clone(),
                session_id: Some(named),
                count: view.fetch_count,
            });
        }
        ui.add(
            egui::DragValue::new(&mut view.fetch_count)
                .range(1..=1000)
                .prefix("count: "),
        );
    });

    if let Some(error) = &view.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.separator();

    let Some(snapshot) = &view.snapshot else {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "Accept a session to peek its messages. The session lock is released immediately.",
            )
            .weak(),
        );
        return;
    };

    ui.horizontal(|ui| {
        ui.label("Session:");
        ui.monospace(&snapshot.session_id);
        if ui
            .small_button(icon(Icon::Copy))
            .on_hover_text("Copy id")
            .clicked()
        {
            ui.ctx().copy_text(snapshot.session_id.clone());
        }
    });
    match &snapshot.state {
        Some(state) => {
            ui.collapsing(format!("Session state ({})", state.format.label()), |ui| {
                let text = state
                    .text
                    .clone()
                    .unwrap_or_else(|| sift_core::body::hex_dump(&state.bytes, 4096));
                ui.add(
                    egui::TextEdit::multiline(&mut text.as_str())
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
            });
        }
        None => {
            ui.label(egui::RichText::new("No session state set.").weak());
        }
    }
    ui.separator();

    ui.label(
        egui::RichText::new(format!("{} message(s) in session", snapshot.messages.len())).strong(),
    );
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for message in &snapshot.messages {
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.monospace(format!("#{}", message.sequence_number));
                            if let Some(subject) = &message.subject {
                                ui.label(subject);
                            }
                            ui.label(
                                egui::RichText::new(message.body.format.label())
                                    .weak()
                                    .small(),
                            );
                        });
                        if let Some(text) = &message.body.text {
                            let preview: String = text.chars().take(400).collect();
                            ui.monospace(preview);
                        }
                    });
            }
        });
}
