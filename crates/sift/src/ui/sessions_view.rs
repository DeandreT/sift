//! Session workspace: retained receiver, lock renewal and message settlement.

use sift_backend::{Disposition, MessageSource, NamespaceId};

use crate::icons::{Icon, icon};
use crate::state::{AppAction, SessionsView};

#[allow(clippy::too_many_lines)] // toolbar + state view + message list read best together
pub fn show(
    ui: &mut egui::Ui,
    ns: NamespaceId,
    source: &MessageSource,
    view: &mut SessionsView,
    actions: &mut Vec<AppAction>,
) {
    let now = time::OffsetDateTime::now_utc();
    if view
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.lock_expired(now))
    {
        view.error =
            Some("The session lock expired. Accept the session again to continue.".to_owned());
        actions.push(AppAction::ReleaseSession {
            ns,
            source: source.clone(),
        });
    }
    // Repaint the countdown even when no backend event arrives.
    if view.snapshot.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
    }
    ui.horizontal_wrapped(|ui| {
        if view.loading {
            ui.spinner();
        } else if ui
            .button(format!("{} Accept next session", icon(Icon::MailOpen)))
            .on_hover_text("Accept and hold the next session, then peek its messages")
            .clicked()
        {
            actions.push(AppAction::BrowseSession {
                ns,
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
        if (view.snapshot.is_some() || view.loading) && ui.button("Release session").clicked() {
            actions.push(AppAction::ReleaseSession {
                ns,
                source: source.clone(),
            });
        }
        let named = view.session_id_input.trim().to_owned();
        if !view.loading && !named.is_empty() && ui.button("Accept this session").clicked() {
            actions.push(AppAction::BrowseSession {
                ns,
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
                "Accept a session to inspect it. Receive messages to settle them while holding the session lock.",
            )
            .weak(),
        );
        return;
    };

    ui.horizontal_wrapped(|ui| {
        ui.label("Session:");
        ui.monospace(&snapshot.session_id);
        if ui
            .small_button(icon(Icon::Copy))
            .on_hover_text("Copy id")
            .clicked()
        {
            ui.ctx().copy_text(snapshot.session_id.clone());
        }
        let remaining = (snapshot.locked_until - now).whole_seconds().max(0);
        ui.label(format!("Session lock: {remaining}s remaining"));
        let active = !snapshot.lock_expired(now) && !view.loading;
        if ui
            .add_enabled(active, egui::Button::new("Renew session lock"))
            .clicked()
        {
            actions.push(AppAction::RenewSession {
                ns,
                source: source.clone(),
                lease_id: snapshot.lease_id,
                lock_token: None,
            });
        }
        if ui
            .add_enabled(active, egui::Button::new("Receive (peek-lock)"))
            .clicked()
        {
            actions.push(AppAction::ReceiveSession {
                ns,
                source: source.clone(),
                lease_id: snapshot.lease_id,
                count: view.fetch_count,
                sequence_numbers: Vec::new(),
            });
        }
    });
    match &snapshot.state {
        Some(state) => {
            ui.collapsing(format!("Session state ({})", state.format.label()), |ui| {
                let hex;
                let mut text = if let Some(text) = &state.text {
                    let preview =
                        sift_core::body::bounded_text(text, sift_core::body::MAX_INLINE_TEXT_BYTES);
                    if preview.len() < text.len() {
                        ui.label("Showing the first 32 KiB of session state.");
                    }
                    preview
                } else {
                    hex = sift_core::body::hex_dump(&state.bytes, 4096);
                    hex.as_str()
                };
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
            });
        }
        None => {
            ui.label(egui::RichText::new("No session state set.").weak());
        }
    }
    if !view.deferred.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Deferred sequence numbers: {:?}", view.deferred));
            if ui
                .add_enabled(
                    !view.loading && !snapshot.lock_expired(now),
                    egui::Button::new("Receive deferred"),
                )
                .clicked()
            {
                actions.push(AppAction::ReceiveSession {
                    ns,
                    source: source.clone(),
                    lease_id: snapshot.lease_id,
                    count: view.fetch_count,
                    sequence_numbers: view.deferred.clone(),
                });
            }
        });
    }
    ui.collapsing("Dead-letter details", |ui| {
        ui.horizontal(|ui| {
            ui.label("Reason:");
            ui.text_edit_singleline(&mut view.dead_letter_reason);
        });
        ui.horizontal(|ui| {
            ui.label("Description:");
            ui.text_edit_singleline(&mut view.dead_letter_description);
        });
    });
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
                        if let Some(token) = &message.lock_token {
                            let lock_active = !snapshot.lock_expired(now)
                                && message.locked_until.is_none_or(|until| until > now);
                            ui.horizontal_wrapped(|ui| {
                                if let Some(until) = message.locked_until {
                                    ui.label(format!(
                                        "Shared session lock: {}s remaining",
                                        (until - now).whole_seconds().max(0)
                                    ));
                                }
                                if !lock_active {
                                    ui.colored_label(
                                        ui.visuals().error_fg_color,
                                        "Lock expired. Receive again.",
                                    );
                                }
                                for (label, disposition) in [
                                    ("Complete", Disposition::Complete),
                                    ("Abandon", Disposition::Abandon),
                                    ("Defer", Disposition::Defer),
                                    (
                                        "Dead-letter",
                                        Disposition::DeadLetter {
                                            reason: (!view.dead_letter_reason.is_empty())
                                                .then(|| view.dead_letter_reason.clone()),
                                            description: (!view.dead_letter_description.is_empty())
                                                .then(|| view.dead_letter_description.clone()),
                                        },
                                    ),
                                ] {
                                    if ui
                                        .add_enabled(
                                            lock_active && !view.loading,
                                            egui::Button::new(label),
                                        )
                                        .clicked()
                                    {
                                        actions.push(AppAction::SettleSessionMessage {
                                            ns,
                                            source: source.clone(),
                                            lease_id: snapshot.lease_id,
                                            lock_token: token.clone(),
                                            disposition,
                                        });
                                    }
                                }
                                if ui
                                    .add_enabled(
                                        lock_active && !view.loading,
                                        egui::Button::new("Renew lock"),
                                    )
                                    .on_hover_text("Session messages share one lock. Renewal keeps every received message locked.")
                                    .clicked()
                                {
                                    actions.push(AppAction::RenewSession {
                                        ns,
                                        source: source.clone(),
                                        lease_id: snapshot.lease_id,
                                        lock_token: Some(token.clone()),
                                    });
                                }
                            });
                        } else {
                            ui.label(
                                egui::RichText::new(
                                    "Peeked. Receive this message before settling it.",
                                )
                                .weak(),
                            );
                        }
                    });
            }
        });
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use sift_backend::{EntityPath, SessionSnapshot};

    #[test]
    fn expired_session_requests_release_without_waiting_for_backend_error() {
        let ns = NamespaceId::new_v4();
        let source = MessageSource {
            entity: EntityPath::Queue("orders".into()),
            dead_letter: false,
        };
        let mut view = SessionsView::new(10);
        view.snapshot = Some(SessionSnapshot {
            lease_id: uuid::Uuid::new_v4(),
            session_id: "customer".into(),
            state: None,
            messages: Vec::new(),
            locked_until: time::OffsetDateTime::now_utc() - time::Duration::seconds(1),
        });
        let mut actions = Vec::new();
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            show(ui, ns, &source, &mut view, &mut actions);
        });
        assert!(actions.iter().any(|action| matches!(action, AppAction::ReleaseSession { ns: action_ns, source: action_source } if *action_ns == ns && *action_source == source)));
        assert!(
            view.error
                .as_deref()
                .expect("visible lock expiry")
                .contains("expired")
        );
    }
}
