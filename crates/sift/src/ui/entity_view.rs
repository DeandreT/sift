//! Read-only entity detail rendering with count chips, a status editor, and
//! refresh/delete actions. Full property editing arrives with the create/edit
//! form work later in Phase 1.

use std::time::Duration;

use sift_backend::{EntityInfo, EntityPath};
use sift_mgmt::{
    EntityRuntimeInfo, EntityStatus, MessageCountDetails, QueueInfo, RuleFilter, RuleInfo,
    SubscriptionInfo, TopicInfo, is_unlimited,
};

use crate::state::AppAction;

pub fn show(ui: &mut egui::Ui, info: &EntityInfo, actions: &mut Vec<AppAction>) {
    let path = info.path();
    header(ui, &path, info, actions);
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match info {
            EntityInfo::Queue(q) => queue_view(ui, q),
            EntityInfo::Topic(t) => topic_view(ui, t),
            EntityInfo::Subscription(s) => subscription_view(ui, s),
            EntityInfo::Rule(r) => rule_view(ui, r),
        });
}

fn queue_view(ui: &mut egui::Ui, q: &QueueInfo) {
    counts_row(ui, &q.runtime.count_details);
    ui.add_space(8.0);
    runtime_grid(ui, &q.runtime);
    ui.add_space(8.0);
    let p = &q.properties;
    grid(ui, "queue-props", |ui| {
        row(ui, "Status", &p.status.to_string());
        row(ui, "Lock duration", &fmt_duration(p.lock_duration));
        row(ui, "Max size (MB)", &p.max_size_in_megabytes.to_string());
        row(
            ui,
            "Default TTL",
            &fmt_duration(p.default_message_time_to_live),
        );
        row(ui, "Max delivery count", &p.max_delivery_count.to_string());
        row_bool(ui, "Requires session", p.requires_session);
        row_bool(ui, "Duplicate detection", p.requires_duplicate_detection);
        row(
            ui,
            "Dedup window",
            &fmt_duration(p.duplicate_detection_history_time_window),
        );
        row_bool(
            ui,
            "Dead-letter on expiration",
            p.dead_lettering_on_message_expiration,
        );
        row_bool(ui, "Batched operations", p.enable_batched_operations);
        row_bool(ui, "Partitioning", p.enable_partitioning);
        row_bool(ui, "Express", p.enable_express);
        row(
            ui,
            "Auto-delete on idle",
            &fmt_duration(p.auto_delete_on_idle),
        );
        row_opt(ui, "Forward to", p.forward_to.as_deref());
        row_opt(
            ui,
            "Forward DLQ to",
            p.forward_dead_lettered_messages_to.as_deref(),
        );
        row_opt(ui, "User metadata", p.user_metadata.as_deref());
    });
}

fn topic_view(ui: &mut egui::Ui, t: &TopicInfo) {
    let p = &t.properties;
    grid(ui, "topic-props", |ui| {
        row(ui, "Status", &p.status.to_string());
        row(ui, "Subscriptions", &t.subscription_count.to_string());
        row(ui, "Size (bytes)", &t.size_in_bytes.to_string());
        row(
            ui,
            "Scheduled messages",
            &t.scheduled_message_count.to_string(),
        );
        row(ui, "Max size (MB)", &p.max_size_in_megabytes.to_string());
        row(
            ui,
            "Default TTL",
            &fmt_duration(p.default_message_time_to_live),
        );
        row_bool(ui, "Duplicate detection", p.requires_duplicate_detection);
        row(
            ui,
            "Dedup window",
            &fmt_duration(p.duplicate_detection_history_time_window),
        );
        row_bool(ui, "Support ordering", p.support_ordering);
        row_bool(ui, "Batched operations", p.enable_batched_operations);
        row_bool(ui, "Partitioning", p.enable_partitioning);
        row_bool(ui, "Express", p.enable_express);
        row(
            ui,
            "Auto-delete on idle",
            &fmt_duration(p.auto_delete_on_idle),
        );
        row_opt(ui, "User metadata", p.user_metadata.as_deref());
        row_time(ui, "Created", t.created_at);
        row_time(ui, "Updated", t.updated_at);
        row_time(ui, "Accessed", t.accessed_at);
    });
}

fn subscription_view(ui: &mut egui::Ui, s: &SubscriptionInfo) {
    counts_row(ui, &s.runtime.count_details);
    ui.add_space(8.0);
    runtime_grid(ui, &s.runtime);
    ui.add_space(8.0);
    let p = &s.properties;
    grid(ui, "subscription-props", |ui| {
        row(ui, "Status", &p.status.to_string());
        row(ui, "Topic", &p.topic);
        row(ui, "Lock duration", &fmt_duration(p.lock_duration));
        row(
            ui,
            "Default TTL",
            &fmt_duration(p.default_message_time_to_live),
        );
        row(ui, "Max delivery count", &p.max_delivery_count.to_string());
        row_bool(ui, "Requires session", p.requires_session);
        row_bool(
            ui,
            "Dead-letter on expiration",
            p.dead_lettering_on_message_expiration,
        );
        row_bool(
            ui,
            "Dead-letter on filter errors",
            p.dead_lettering_on_filter_evaluation_exceptions,
        );
        row_bool(ui, "Batched operations", p.enable_batched_operations);
        row(
            ui,
            "Auto-delete on idle",
            &fmt_duration(p.auto_delete_on_idle),
        );
        row_opt(ui, "Forward to", p.forward_to.as_deref());
        row_opt(
            ui,
            "Forward DLQ to",
            p.forward_dead_lettered_messages_to.as_deref(),
        );
        row_opt(ui, "User metadata", p.user_metadata.as_deref());
    });
}

fn rule_view(ui: &mut egui::Ui, r: &RuleInfo) {
    let p = &r.properties;
    grid(ui, "rule-props", |ui| {
        row(ui, "Topic", &p.topic);
        row(ui, "Subscription", &p.subscription);
        let kind = match &p.filter {
            RuleFilter::Sql { .. } => "SQL",
            RuleFilter::Correlation { .. } => "Correlation",
            RuleFilter::True => "True",
            RuleFilter::False => "False",
        };
        row(ui, "Filter type", kind);
        row(ui, "Filter", &p.filter.summary());
        row_opt(ui, "Action", p.action.as_deref());
        row_time(ui, "Created", r.created_at);
    });
    if let RuleFilter::Correlation { properties, .. } = &p.filter
        && !properties.is_empty()
    {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Correlation properties").strong());
        grid(ui, "rule-correlation-props", |ui| {
            for (key, value) in properties {
                row(ui, key, value);
            }
        });
    }
}

fn header(ui: &mut egui::Ui, path: &EntityPath, info: &EntityInfo, actions: &mut Vec<AppAction>) {
    ui.horizontal(|ui| {
        ui.heading(path.name());
        ui.label(egui::RichText::new(path.kind()).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Delete…").clicked() {
                actions.push(AppAction::RequestDelete(path.clone()));
            }
            if ui
                .button(format!(
                    "{} Refresh",
                    egui_phosphor::regular::ARROWS_CLOCKWISE
                ))
                .clicked()
            {
                actions.push(AppAction::RefreshEntity(path.clone()));
            }
            status_selector(ui, info, actions);
        });
    });
}

/// Status dropdown that immediately applies a change (parity with the ref
/// app's Change Status action).
fn status_selector(ui: &mut egui::Ui, info: &EntityInfo, actions: &mut Vec<AppAction>) {
    let current = match info {
        EntityInfo::Queue(q) => q.properties.status,
        EntityInfo::Topic(t) => t.properties.status,
        EntityInfo::Subscription(s) => s.properties.status,
        EntityInfo::Rule(_) => return, // rules have no status
    };
    let mut selected = current;
    egui::ComboBox::from_id_salt("entity-status")
        .selected_text(current.to_string())
        .show_ui(ui, |ui| {
            for status in EntityStatus::ALL {
                ui.selectable_value(&mut selected, status, status.to_string());
            }
        });
    if selected != current {
        let mut updated = info.clone();
        match &mut updated {
            EntityInfo::Queue(q) => q.properties.status = selected,
            EntityInfo::Topic(t) => t.properties.status = selected,
            EntityInfo::Subscription(s) => s.properties.status = selected,
            EntityInfo::Rule(_) => unreachable!(),
        }
        actions.push(AppAction::UpdateEntity(Box::new(updated)));
    }
}

fn counts_row(ui: &mut egui::Ui, counts: &MessageCountDetails) {
    ui.horizontal(|ui| {
        count_chip(ui, "Active", counts.active, None);
        count_chip(
            ui,
            "Dead-letter",
            counts.dead_letter,
            (counts.dead_letter > 0).then(|| ui.visuals().warn_fg_color),
        );
        count_chip(ui, "Scheduled", counts.scheduled, None);
        count_chip(ui, "Transfer", counts.transfer, None);
        count_chip(ui, "Total", counts.total(), None);
    });
}

fn count_chip(ui: &mut egui::Ui, label: &str, value: i64, color: Option<egui::Color32>) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                let value_text = egui::RichText::new(value.to_string()).heading();
                ui.label(color.map_or(value_text.clone(), |c| value_text.color(c)));
                ui.label(egui::RichText::new(label).weak().small());
            });
        });
}

fn runtime_grid(ui: &mut egui::Ui, runtime: &EntityRuntimeInfo) {
    grid(ui, "entity-runtime", |ui| {
        row(ui, "Message count", &runtime.message_count.to_string());
        row(ui, "Size (bytes)", &runtime.size_in_bytes.to_string());
        row_time(ui, "Created", runtime.created_at);
        row_time(ui, "Updated", runtime.updated_at);
        row_time(ui, "Accessed", runtime.accessed_at);
    });
}

fn grid(ui: &mut egui::Ui, id: &str, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([16.0, 4.0])
        .striped(true)
        .show(ui, contents);
}

fn row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).weak());
    let response = ui.monospace(value);
    if response.on_hover_text("Click to copy").clicked() {
        ui.ctx().copy_text(value.to_owned());
    }
    ui.end_row();
}

fn row_bool(ui: &mut egui::Ui, label: &str, value: bool) {
    row(ui, label, if value { "yes" } else { "no" });
}

fn row_opt(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    row(ui, label, value.unwrap_or("—"));
}

fn row_time(ui: &mut egui::Ui, label: &str, value: Option<time::OffsetDateTime>) {
    let text = value.map_or("—".to_owned(), |t| {
        t.format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| t.to_string())
    });
    row(ui, label, &text);
}

/// Human-friendly duration: `1m`, `30s`, `14d`, or `unlimited`.
pub fn fmt_duration(d: Duration) -> String {
    if is_unlimited(d) {
        return "unlimited".to_owned();
    }
    let secs = d.as_secs();
    if secs > 0 && secs.is_multiple_of(86_400) {
        format!("{}d", secs / 86_400)
    } else if secs > 0 && secs.is_multiple_of(3_600) {
        format!("{}h", secs / 3_600)
    } else if secs > 0 && secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}
