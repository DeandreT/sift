//! Message browsing surface: toolbar, virtualized message grid, and the
//! body/properties viewer for the selected message.

use egui_extras::{Column, TableBuilder};
use sift_backend::{Disposition, MessageSource, NamespaceId, ReceiveMode};
use sift_core::body::{BodyFormat, DecodedBody, MAX_INLINE_TEXT_BYTES, hex_dump};
use sift_core::message::{MessageState, SiftMessage};

use crate::icons::{Icon, icon};
use crate::state::{AppAction, MessagesView};
use crate::ui::payload_text;

#[derive(Clone)]
struct Base64Preview {
    key: (uuid::Uuid, i64, usize, usize),
    decoded: Option<std::sync::Arc<DecodedBody>>,
    attempted: bool,
    error: Option<String>,
}

pub fn show(
    ui: &mut egui::Ui,
    ns: NamespaceId,
    source: &MessageSource,
    view: &mut MessagesView,
    actions: &mut Vec<AppAction>,
) {
    toolbar(ui, ns, source, view, actions);
    if let Some(error) = &view.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.separator();

    // Split the remaining space: grid on top, viewer below.
    let viewer_height = if view.selected.is_some() {
        ui.available_height() * 0.5
    } else {
        0.0
    };
    let grid_height = (ui.available_height() - viewer_height).max(120.0);

    ui.scope(|ui| {
        ui.set_max_height(grid_height);
        message_table(ui, view, grid_height);
    });

    if view.selected.is_some() {
        ui.separator();
        message_viewer(ui, view, actions);
    }
}

#[allow(clippy::too_many_lines)] // toolbar with settle actions reads best inline
fn toolbar(
    ui: &mut egui::Ui,
    ns: NamespaceId,
    source: &MessageSource,
    view: &mut MessagesView,
    actions: &mut Vec<AppAction>,
) {
    ui.horizontal_wrapped(|ui| {
        if view.loading {
            ui.spinner();
        } else if ui
            .button(format!("{} Peek", icon(Icon::Search)))
            .on_hover_text("Browse without consuming, from the front")
            .clicked()
        {
            actions.push(AppAction::PeekMessages {
                ns,
                source: source.clone(),
                from_seq: None,
                count: view.fetch_count,
            });
        }
        if !view.loading
            && !view.rows.is_empty()
            && ui
                .button("Load more")
                .on_hover_text("Peek the next page")
                .clicked()
        {
            actions.push(AppAction::PeekMessages {
                ns,
                source: source.clone(),
                from_seq: view.next_seq(),
                count: view.fetch_count,
            });
        }
        ui.add(
            egui::DragValue::new(&mut view.fetch_count)
                .range(1..=1000)
                .prefix("count: "),
        );

        ui.menu_button(format!("{} Receive", icon(Icon::Download)), |ui| {
            if ui.button("Receive with lock (settle afterwards)").clicked() {
                actions.push(AppAction::ReceiveMessages {
                    ns,
                    source: source.clone(),
                    mode: ReceiveMode::PeekLock,
                    count: view.fetch_count,
                });
                ui.close();
            }
            let destructive = egui::RichText::new("Receive and delete (destructive)")
                .color(ui.visuals().error_fg_color);
            if ui.button(destructive).clicked() {
                actions.push(AppAction::ReceiveMessages {
                    ns,
                    source: source.clone(),
                    mode: ReceiveMode::ReceiveAndDelete,
                    count: view.fetch_count,
                });
                ui.close();
            }
        });

        // Settlement actions for the selected locked message.
        let selected = view
            .selected_message()
            .map(|m| (m.lock_token.clone(), m.sequence_number, m.state));
        let mut newly_deferred = None;
        if let Some((Some(token), seq, _)) = &selected {
            let (token, seq) = (token.clone(), *seq);
            ui.separator();
            let settle = |disposition: Disposition| AppAction::Settle {
                ns,
                source: source.clone(),
                lock_token: token.clone(),
                disposition,
            };
            if ui
                .button("Complete")
                .on_hover_text("Remove the message")
                .clicked()
            {
                actions.push(settle(Disposition::Complete));
            }
            if ui
                .button("Abandon")
                .on_hover_text("Release the lock; the message stays")
                .clicked()
            {
                actions.push(settle(Disposition::Abandon));
            }
            if ui
                .button("Defer")
                .on_hover_text("Set aside; retrievable by sequence number")
                .clicked()
            {
                actions.push(settle(Disposition::Defer));
                newly_deferred = Some(seq);
            }
            if !source.dead_letter && ui.button("Dead-letter").clicked() {
                actions.push(settle(Disposition::DeadLetter {
                    reason: Some("sift".into()),
                    description: None,
                }));
            }
        }
        // Cancel a selected scheduled message by its sequence number.
        if let Some((_, seq, MessageState::Scheduled)) = &selected {
            ui.separator();
            if ui
                .button("Cancel scheduled")
                .on_hover_text("Remove this scheduled message")
                .clicked()
            {
                actions.push(AppAction::CancelScheduled {
                    ns,
                    target: send_target(source),
                    sequence_number: *seq,
                });
            }
        }
        if let Some(seq) = newly_deferred {
            view.deferred_seqs.push(seq);
        }
        // Retrieve messages deferred from this view during the session.
        if !view.deferred_seqs.is_empty() {
            ui.separator();
            if ui
                .button(format!(
                    "{} Retrieve deferred ({})",
                    icon(Icon::ArchiveRestore),
                    view.deferred_seqs.len()
                ))
                .clicked()
            {
                actions.push(AppAction::ReceiveDeferred {
                    ns,
                    source: source.clone(),
                    sequence_numbers: view.deferred_seqs.clone(),
                });
                view.deferred_seqs.clear();
            }
        }

        // Resend the selected message as a new one.
        if let Some(message) = view.selected_message() {
            ui.separator();
            if ui
                .button(format!("{} Resend…", icon(Icon::Send)))
                .on_hover_text("Compose a new message from this one")
                .clicked()
            {
                let target = send_target(source);
                actions.push(AppAction::OpenSendDialog {
                    ns,
                    target,
                    prefill: Some(Box::new(message.to_outbound())),
                });
            }
        }

        // Bulk operations, pinned to the right.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let purge = egui::Button::new(
                egui::RichText::new(format!("{} Purge", icon(Icon::Trash2)))
                    .color(ui.visuals().error_fg_color),
            );
            if ui
                .add(purge)
                .on_hover_text("Delete every message in this view")
                .clicked()
            {
                actions.push(AppAction::RequestPurge {
                    ns,
                    source: source.clone(),
                });
            }
            if source.dead_letter
                && ui
                    .button(format!("{} Resubmit all", icon(Icon::Undo2)))
                    .on_hover_text("Move every dead-letter message back to the entity")
                    .clicked()
            {
                actions.push(AppAction::ResubmitAll {
                    ns,
                    source: source.clone(),
                    target: send_target(source),
                });
            }
        });
    });
}

/// Where a resend from this source should go: queues to themselves,
/// subscriptions (and their DLQs) back to the topic.
fn send_target(source: &MessageSource) -> sift_backend::EntityPath {
    match &source.entity {
        sift_backend::EntityPath::Subscription { topic, .. } => {
            sift_backend::EntityPath::Topic(topic.clone())
        }
        other => other.clone(),
    }
}

/// A single-line, truncated table cell so every row stays one line tall and
/// the selection highlight matches the row. Returns the response for hover
/// tooltips on columns that may be clipped.
fn cell(ui: &mut egui::Ui, text: egui::RichText) -> egui::Response {
    ui.add(egui::Label::new(text).truncate().selectable(false))
}

#[allow(clippy::too_many_lines)] // one column definition + cell per grid column
fn message_table(ui: &mut egui::Ui, view: &mut MessagesView, height: f32) {
    let dlq_column = view.rows.iter().any(|m| m.dead_letter_reason.is_some());
    let mut builder = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .max_scroll_height(height)
        .column(Column::auto().at_least(56.0)) // seq
        .column(Column::auto().at_least(64.0)) // state / lock
        .column(Column::remainder().at_least(120.0)) // message id
        .column(Column::remainder().at_least(100.0)) // subject
        .column(Column::auto().at_least(130.0)) // enqueued
        .column(Column::auto().at_least(52.0)) // size
        .column(Column::auto().at_least(28.0)); // deliveries
    if dlq_column {
        builder = builder.column(Column::remainder().at_least(100.0));
    }

    builder
        .header(20.0, |mut header| {
            for title in [
                "Seq",
                "State",
                "Message id",
                "Subject",
                "Enqueued (UTC)",
                "Size",
                "Dlv",
            ] {
                header.col(|ui| {
                    ui.label(egui::RichText::new(title).strong());
                });
            }
            if dlq_column {
                header.col(|ui| {
                    ui.label(egui::RichText::new("DLQ reason").strong());
                });
            }
        })
        .body(|body| {
            body.rows(18.0, view.rows.len(), |mut row| {
                let index = row.index();
                let message = &view.rows[index];
                row.set_selected(view.selected == Some(index));

                row.col(|ui| {
                    cell(
                        ui,
                        egui::RichText::new(message.sequence_number.to_string()).monospace(),
                    );
                });
                row.col(|ui| {
                    let text = if message.lock_token.is_some() {
                        egui::RichText::new("locked").color(ui.visuals().warn_fg_color)
                    } else {
                        egui::RichText::new(message.state.label()).weak()
                    };
                    cell(ui, text);
                });
                row.col(|ui| {
                    let id = message.message_id.as_deref().unwrap_or("—");
                    cell(ui, egui::RichText::new(id).monospace()).on_hover_text(id);
                });
                row.col(|ui| {
                    let subject = message.subject.as_deref().unwrap_or("—");
                    cell(ui, egui::RichText::new(subject)).on_hover_text(subject);
                });
                row.col(|ui| {
                    let text = message
                        .enqueued_time
                        .map_or_else(|| "—".into(), format_time);
                    cell(ui, egui::RichText::new(text).monospace());
                });
                row.col(|ui| {
                    cell(
                        ui,
                        egui::RichText::new(format_size(message.body.size())).monospace(),
                    );
                });
                row.col(|ui| {
                    let count = message
                        .delivery_count
                        .map_or_else(|| "—".into(), |c| c.to_string());
                    cell(ui, egui::RichText::new(count).monospace());
                });
                if dlq_column {
                    row.col(|ui| {
                        let reason = message.dead_letter_reason.as_deref().unwrap_or("—");
                        // Full reason (often a multi-line stack trace) is in the
                        // detail pane; keep the grid row single-line.
                        cell(ui, egui::RichText::new(reason.replace('\n', " ")))
                            .on_hover_text(reason);
                    });
                }

                if row.response().clicked() {
                    view.selected = if view.selected == Some(index) {
                        None
                    } else {
                        Some(index)
                    };
                }
            });
        });

    if view.rows.is_empty() && !view.loading {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("No messages loaded — use Peek.").weak());
    }
}

#[allow(clippy::too_many_lines)] // header + split panels read best in one function
fn message_viewer(ui: &mut egui::Ui, view: &mut MessagesView, actions: &mut Vec<AppAction>) {
    #[cfg(target_arch = "wasm32")]
    let _ = actions;

    let Some(index) = view.selected else { return };
    let Some(message) = view.rows.get(index) else {
        view.selected = None;
        return;
    };
    // Borrow the complete payload. Cloning it, base64 detection, and laying
    // out its full text on every frame made megabyte messages unresponsive.
    let raw_body = &message.body;
    let text = raw_body.text.as_deref().unwrap_or_default();
    let key = (
        view.body_generation,
        message.sequence_number,
        text.as_ptr() as usize,
        text.len(),
    );
    let cache_id = ui.id().with("body-base64-cache");
    let mut cache = ui
        .data_mut(|data| data.get_temp::<Base64Preview>(cache_id))
        .filter(|cache| cache.key == key)
        .unwrap_or_else(|| {
            let automatic =
                raw_body.format == BodyFormat::Text && text.len() <= MAX_INLINE_TEXT_BYTES;
            Base64Preview {
                key,
                decoded: automatic
                    .then(|| sift_core::body::detect_base64(text))
                    .flatten()
                    .map(std::sync::Arc::new),
                attempted: automatic,
                error: None,
            }
        });
    let decoded = cache.decoded.clone(); // Arc clone, independent of payload size.
    let show_base64 = &mut view.show_base64;
    let show_hex = &mut view.show_hex;
    let body = if *show_base64 {
        decoded.as_deref().unwrap_or(raw_body)
    } else {
        raw_body
    };

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(body.format.label()).strong());
        if raw_body.gzipped {
            ui.label(egui::RichText::new("gzip").weak());
        }
        ui.label(egui::RichText::new(format_size(body.size())).weak());
        if let Some(content_type) = &message.content_type {
            ui.label(egui::RichText::new(content_type).weak());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            #[cfg(not(target_arch = "wasm32"))]
            ui.menu_button(icon(Icon::Download), |ui| {
                if ui.button("Save body...").clicked() {
                    actions.push(AppAction::SaveMessageBody(Box::new((*message).clone())));
                    ui.close();
                }
                if ui.button("Save message template...").clicked() {
                    actions.push(AppAction::SaveMessageTemplate(Box::new((*message).clone())));
                    ui.close();
                }
            })
            .response
            .on_hover_text("Export message");
            // One-click copy of the (decoded) body.
            if ui
                .button(format!("{} Copy body", icon(Icon::Copy)))
                .clicked()
            {
                let text = body
                    .text
                    .clone()
                    .unwrap_or_else(|| hex_dump(&body.bytes, usize::MAX));
                ui.ctx().copy_text(text);
            }
            ui.checkbox(show_hex, "Hex");
            if cache.decoded.is_some() {
                ui.checkbox(show_base64, "Base64")
                    .on_hover_text("This body looks like base64 — show the decoded content");
            } else if raw_body.format == BodyFormat::Text
                && !cache.attempted
                && ui
                    .button("Decode Base64")
                    .on_hover_text("Decode this large body once")
                    .clicked()
            {
                cache.attempted = true;
                match sift_core::body::decode_base64(text) {
                    Ok(decoded) => {
                        cache.decoded = Some(std::sync::Arc::new(decoded));
                        *show_base64 = true;
                    }
                    Err(error) => cache.error = Some(error),
                }
            }
        });
    });
    if let Some(error) = &cache.error {
        ui.label(egui::RichText::new(error).weak());
    }
    ui.data_mut(|data| data.insert_temp(cache_id, cache));

    // Body on the left, properties on the right (resizable split). The panel
    // id must be salted per ui (panels keep their own persisted state and
    // don't inherit the id stack), or two visible viewers clash.
    egui::Panel::right(ui.id().with("message-props"))
        .resizable(true)
        .default_size(280.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("message-props-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::CollapsingHeader::new("System properties")
                        .default_open(true)
                        .show(ui, |ui| system_properties(ui, message));
                    if !message.application_properties.is_empty() {
                        egui::CollapsingHeader::new(format!(
                            "Custom properties ({})",
                            message.application_properties.len()
                        ))
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::Grid::new("custom-props")
                                .num_columns(2)
                                .striped(true)
                                .show(ui, |ui| {
                                    for (key, value) in &message.application_properties {
                                        ui.label(egui::RichText::new(key).weak());
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(value).monospace(),
                                            )
                                            .wrap(),
                                        );
                                        ui.end_row();
                                    }
                                });
                        });
                    }
                });
        });
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("message-body-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if *show_hex || body.text.is_none() {
                    ui.monospace(hex_dump(&body.bytes, 16 * 1024));
                } else if let Some(text) = &body.text {
                    if text.len() > MAX_INLINE_TEXT_BYTES {
                        let page_id = ui.id().with(("body-text-page", key, *show_base64));
                        let mut page = ui.data_mut(|data| data.get_temp::<usize>(page_id)).unwrap_or(0);
                        ui.label(format!("Large body: {} bytes. Copy body and Save body include the complete payload.", text.len()));
                        payload_text::page_controls(ui, text.len(), &mut page);
                        let range = payload_text::page_range(text, page);
                        ui.label(egui::RichText::new(format!("Bytes {}–{} of {}", range.start + 1, range.end, text.len())).weak());
                        ui.data_mut(|data| data.insert_temp(page_id, page));
                        let mut visible = &text[range];
                        ui.add(egui::TextEdit::multiline(&mut visible).code_editor().desired_width(f32::INFINITY));
                        return;
                    }
                    let language = match body.format {
                        BodyFormat::Json => "json",
                        BodyFormat::Xml => "xml",
                        _ => "txt",
                    };
                    let theme = egui_extras::syntax_highlighting::CodeTheme::from_style(ui.style());
                    egui_extras::syntax_highlighting::code_view_ui(ui, &theme, text, language);
                }
            });
    });
}

fn system_properties(ui: &mut egui::Ui, m: &SiftMessage) {
    egui::Grid::new("system-props")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            let mut row = |label: &str, value: String| {
                ui.label(egui::RichText::new(label).weak());
                if value.is_empty() {
                    ui.monospace("");
                } else {
                    // Click the value to copy it (e.g. the DLQ reason).
                    let resp = ui
                        .add(
                            egui::Label::new(egui::RichText::new(&value).monospace())
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text("Click to copy");
                    if resp.clicked() {
                        ui.ctx().copy_text(value.clone());
                    }
                }
                ui.end_row();
            };
            row("Sequence number", m.sequence_number.to_string());
            row("Message id", m.message_id.clone().unwrap_or_default());
            row("Subject", m.subject.clone().unwrap_or_default());
            row(
                "Correlation id",
                m.correlation_id.clone().unwrap_or_default(),
            );
            row("Session id", m.session_id.clone().unwrap_or_default());
            row("To", m.to.clone().unwrap_or_default());
            row("Reply to", m.reply_to.clone().unwrap_or_default());
            row(
                "Enqueued",
                m.enqueued_time.map(format_time).unwrap_or_default(),
            );
            row("Expires", m.expires_at.map(format_time).unwrap_or_default());
            row(
                "Delivery count",
                m.delivery_count.map(|c| c.to_string()).unwrap_or_default(),
            );
            row("State", m.state.label().to_owned());
            if let Some(token) = &m.lock_token {
                row("Lock token", token.clone());
            }
            if let Some(until) = m.locked_until {
                row("Locked until", format_time(until));
            }
            if let Some(reason) = &m.dead_letter_reason {
                row("DLQ reason", reason.clone());
            }
            if let Some(desc) = &m.dead_letter_error_description {
                row("DLQ description", desc.clone());
            }
            if let Some(source) = &m.dead_letter_source {
                row("DLQ source", source.clone());
            }
        });
}

fn format_time(t: time::OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

#[allow(clippy::cast_precision_loss)] // display only; message sizes never approach 2^52 bytes
fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn largest_text_shape(shape: &egui::epaint::Shape) -> usize {
        match shape {
            egui::epaint::Shape::Text(text) => text.galley.text().len(),
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().map(largest_text_shape).max().unwrap_or(0)
            }
            _ => 0,
        }
    }

    #[test]
    fn large_json_viewer_renders_only_a_page_on_every_frame() {
        let original = format!("{{\"data\":\"{}\"}}", "x".repeat(1_400_000));
        let mut view = MessagesView::new(100);
        view.rows.push(SiftMessage {
            sequence_number: 1,
            message_id: None,
            subject: None,
            content_type: Some("application/json".into()),
            correlation_id: None,
            session_id: None,
            reply_to: None,
            to: None,
            enqueued_time: None,
            expires_at: None,
            time_to_live: None,
            delivery_count: None,
            state: MessageState::Active,
            lock_token: None,
            locked_until: None,
            dead_letter_reason: None,
            dead_letter_error_description: None,
            dead_letter_source: None,
            application_properties: Vec::new(),
            body: sift_core::body::decode(original.clone().into_bytes()),
        });
        view.selected = Some(0);
        let ctx = egui::Context::default();
        crate::icons::install(&ctx);
        let mut actions = Vec::new();
        let started = std::time::Instant::now();
        for frame in 0..20 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 900.0),
                )),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| message_viewer(ui, &mut view, &mut actions));
            let largest = output
                .shapes
                .iter()
                .map(|shape| largest_text_shape(&shape.shape))
                .max()
                .unwrap_or(0);
            assert!(
                largest <= MAX_INLINE_TEXT_BYTES + 3,
                "frame {frame} rendered {largest} bytes in one text layout"
            );
            assert_eq!(view.rows[0].body.text.as_deref(), Some(original.as_str()));
            assert_eq!(view.rows[0].body.bytes.len(), original.len());
        }
        assert!(actions.is_empty());
        eprintln!(
            "1.4 MB JSON: 20 bounded viewer frames {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn base64_cache_refreshes_when_equal_length_payload_reuses_its_allocation() {
        let mut view = MessagesView::new(100);
        view.rows.push(SiftMessage {
            sequence_number: 1,
            message_id: None,
            subject: None,
            content_type: None,
            correlation_id: None,
            session_id: None,
            reply_to: None,
            to: None,
            enqueued_time: None,
            expires_at: None,
            time_to_live: None,
            delivery_count: None,
            state: MessageState::Active,
            lock_token: None,
            locked_until: None,
            dead_letter_reason: None,
            dead_letter_error_description: None,
            dead_letter_source: None,
            application_properties: Vec::new(),
            body: sift_core::body::decode(b"eyJhIjoxfQ==".to_vec()),
        });
        view.selected = Some(0);
        let ctx = egui::Context::default();
        crate::icons::install(&ctx);
        let mut actions = Vec::new();
        for number in 1..=2 {
            if number == 2 {
                let text = view.rows[0].body.text.as_mut().expect("text body");
                let original_pointer = text.as_ptr();
                text.replace_range(.., "eyJhIjoyfQ==");
                assert_eq!(text.as_ptr(), original_pointer, "exercise allocation reuse");
                view.rows[0].body.bytes.copy_from_slice(b"eyJhIjoyfQ==");
                view.invalidate_body_cache();
            }
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 900.0),
                )),
                ..Default::default()
            };
            let mut decoded_text = String::new();
            let _ = ctx.run_ui(input, |ui| {
                message_viewer(ui, &mut view, &mut actions);
                let cache = ui
                    .data_mut(|data| {
                        data.get_temp::<Base64Preview>(ui.id().with("body-base64-cache"))
                    })
                    .expect("preview cache");
                decoded_text = cache
                    .decoded
                    .expect("base64 decoded")
                    .text
                    .clone()
                    .expect("decoded JSON");
            });
            assert!(
                decoded_text.contains(&format!("\"a\": {number}")),
                "cache must decode the current payload"
            );
        }
    }
}
