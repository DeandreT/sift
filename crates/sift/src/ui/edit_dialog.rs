//! Entity property editor. Start from the complete server description so
//! settings outside the form survive a save unchanged.

use std::time::Duration;

use sift_backend::{BackendError, EntityDescription, EntityInfo, NamespaceId, RequestId};
use sift_mgmt::{CorrelationPropertyType, EntityStatus, RuleFilter, is_unlimited, unlimited};

#[derive(Debug)]
struct DurationField {
    original: Duration,
    initial: String,
    text: String,
}

impl DurationField {
    fn new(value: Duration) -> Self {
        let text = if is_unlimited(value) {
            "unlimited".into()
        } else {
            value.as_secs_f64().to_string()
        };
        Self {
            original: value,
            initial: text.clone(),
            text,
        }
    }

    fn parse(&self, label: &str, allow_unlimited: bool) -> Result<Duration, String> {
        // Keep the exact server value, including subsecond precision.
        if self.text == self.initial {
            return Ok(self.original);
        }
        let text = self.text.trim();
        if allow_unlimited && text.eq_ignore_ascii_case("unlimited") {
            return Ok(unlimited());
        }
        let seconds = text.parse::<f64>().map_err(|_| {
            format!(
                "{label}: enter seconds{}.",
                if allow_unlimited { " or unlimited" } else { "" }
            )
        })?;
        let duration = Duration::try_from_secs_f64(seconds)
            .map_err(|_| format!("{label}: enter a positive, finite duration."))?;
        if duration.is_zero() || duration > unlimited() {
            return Err(format!(
                "{label}: enter a positive duration within the Service Bus limit."
            ));
        }
        Ok(duration)
    }
}

#[derive(Debug)]
pub struct EditDialog {
    pub ns: NamespaceId,
    pub description: EntityDescription,
    pub saving: Option<RequestId>,
    pub error: Option<String>,
    ttl: DurationField,
    auto_delete: DurationField,
    lock: DurationField,
    dedup: DurationField,
    premium: bool,
    correlation_property_types: Vec<CorrelationPropertyType>,
    pub confirm_rule_replacement: bool,
}

impl EditDialog {
    #[must_use]
    pub fn new(ns: NamespaceId, info: &EntityInfo, sku: Option<&str>) -> Self {
        let description = match info {
            EntityInfo::Queue(q) => EntityDescription::Queue(q.properties.clone()),
            EntityInfo::Topic(t) => EntityDescription::Topic(t.properties.clone()),
            EntityInfo::Subscription(s) => EntityDescription::Subscription(s.properties.clone()),
            EntityInfo::Rule(r) => EntityDescription::Rule(r.properties.clone()),
        };
        let (ttl, auto_delete, lock, dedup) = match &description {
            EntityDescription::Queue(p) => (
                p.default_message_time_to_live,
                p.auto_delete_on_idle,
                p.lock_duration,
                p.duplicate_detection_history_time_window,
            ),
            EntityDescription::Topic(p) => (
                p.default_message_time_to_live,
                p.auto_delete_on_idle,
                Duration::from_mins(1),
                p.duplicate_detection_history_time_window,
            ),
            EntityDescription::Subscription(p) => (
                p.default_message_time_to_live,
                p.auto_delete_on_idle,
                p.lock_duration,
                Duration::from_mins(10),
            ),
            EntityDescription::Rule(_) => (
                unlimited(),
                unlimited(),
                Duration::from_mins(1),
                Duration::from_mins(10),
            ),
        };
        let correlation_property_types = match &description {
            EntityDescription::Rule(p) => match &p.filter {
                RuleFilter::Correlation {
                    properties,
                    property_types,
                    ..
                } => properties
                    .iter()
                    .map(|(name, _)| {
                        property_types
                            .get(name)
                            .copied()
                            .unwrap_or(CorrelationPropertyType::String)
                    })
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        Self {
            ns,
            description,
            saving: None,
            error: None,
            ttl: DurationField::new(ttl),
            auto_delete: DurationField::new(auto_delete),
            lock: DurationField::new(lock),
            dedup: DurationField::new(dedup),
            premium: sku.is_some_and(|s| s.eq_ignore_ascii_case("premium")),
            correlation_property_types,
            confirm_rule_replacement: false,
        }
    }

    fn validated(&self) -> Result<EntityDescription, String> {
        let mut description = self.description.clone();
        match &mut description {
            EntityDescription::Queue(p) => {
                normalize_identifier(&mut p.forward_to);
                normalize_identifier(&mut p.forward_dead_lettered_messages_to);
                normalize_optional(&mut p.user_metadata);
                p.default_message_time_to_live = self.ttl.parse("Default TTL", true)?;
                p.auto_delete_on_idle = self.auto_delete.parse("Auto-delete on idle", true)?;
                validate_idle(p.auto_delete_on_idle)?;
                p.lock_duration = self.lock.parse("Lock duration", false)?;
                validate_lock_and_delivery(p.lock_duration, p.max_delivery_count)?;
                validate_size(p.max_size_in_megabytes, p.max_message_size_in_kilobytes)?;
                if p.requires_duplicate_detection {
                    p.duplicate_detection_history_time_window =
                        self.dedup.parse("Duplicate detection window", false)?;
                    validate_dedup(p.duplicate_detection_history_time_window)?;
                }
                validate_forward(p.forward_to.as_deref(), &p.name)?;
                validate_forward(p.forward_dead_lettered_messages_to.as_deref(), &p.name)?;
            }
            EntityDescription::Topic(p) => {
                normalize_optional(&mut p.user_metadata);
                p.default_message_time_to_live = self.ttl.parse("Default TTL", true)?;
                p.auto_delete_on_idle = self.auto_delete.parse("Auto-delete on idle", true)?;
                validate_idle(p.auto_delete_on_idle)?;
                validate_size(p.max_size_in_megabytes, p.max_message_size_in_kilobytes)?;
                if p.requires_duplicate_detection {
                    p.duplicate_detection_history_time_window =
                        self.dedup.parse("Duplicate detection window", false)?;
                    validate_dedup(p.duplicate_detection_history_time_window)?;
                }
            }
            EntityDescription::Subscription(p) => {
                normalize_identifier(&mut p.forward_to);
                normalize_identifier(&mut p.forward_dead_lettered_messages_to);
                normalize_optional(&mut p.user_metadata);
                p.default_message_time_to_live = self.ttl.parse("Default TTL", true)?;
                p.auto_delete_on_idle = self.auto_delete.parse("Auto-delete on idle", true)?;
                validate_idle(p.auto_delete_on_idle)?;
                p.lock_duration = self.lock.parse("Lock duration", false)?;
                validate_lock_and_delivery(p.lock_duration, p.max_delivery_count)?;
                validate_forward(
                    p.forward_to.as_deref(),
                    &format!("{}/subscriptions/{}", p.topic, p.name),
                )?;
                validate_forward(
                    p.forward_dead_lettered_messages_to.as_deref(),
                    &format!("{}/subscriptions/{}", p.topic, p.name),
                )?;
            }
            EntityDescription::Rule(p) => {
                normalize_optional(&mut p.action);
                if !self.confirm_rule_replacement {
                    return Err("Confirm replacement of this rule before saving.".into());
                }
                validate_rule_filter(&mut p.filter, &self.correlation_property_types)?;
            }
        }
        Ok(description)
    }

    pub fn build(&mut self) -> Option<EntityDescription> {
        match self.validated() {
            Ok(description) => {
                self.error = None;
                Some(description)
            }
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }

    /// Returns true only for the successful response to this form's request.
    /// Failed edits remain available for correction and retry.
    pub fn complete(
        &mut self,
        req: RequestId,
        result: &Result<Option<EntityInfo>, BackendError>,
    ) -> bool {
        if self.saving != Some(req) {
            return false;
        }
        self.saving = None;
        match result {
            Ok(_) => true,
            Err(error) => {
                self.error = Some(error.message.clone());
                false
            }
        }
    }
}

/// Normalize only the outgoing copy. The draft retains spaces during entry.
fn normalize_optional(value: &mut Option<String>) {
    if value.as_ref().is_some_and(|text| text.trim().is_empty()) {
        *value = None;
    }
}

fn normalize_identifier(value: &mut Option<String>) {
    if let Some(text) = value {
        *text = text.trim().to_owned();
    }
    normalize_optional(value);
}

fn validate_rule_filter(
    filter: &mut RuleFilter,
    types: &[CorrelationPropertyType],
) -> Result<(), String> {
    match filter {
        RuleFilter::Sql { expression } if expression.trim().is_empty() => {
            return Err("Enter a SQL filter expression.".into());
        }
        RuleFilter::Correlation {
            properties,
            property_types,
            ..
        } => {
            // Filter strings and property names use exact broker matching.
            // Preserve whitespace when another rule field is edited.
            let mut names = std::collections::HashSet::new();
            property_types.clear();
            for (index, (name, _)) in properties.iter_mut().enumerate() {
                if name.trim().is_empty() || !names.insert(name.clone()) {
                    return Err("Correlation property names must be nonempty and unique.".into());
                }
                let kind = types
                    .get(index)
                    .copied()
                    .unwrap_or(CorrelationPropertyType::String);
                if kind != CorrelationPropertyType::String {
                    property_types.insert(name.clone(), kind);
                }
            }
        }
        _ => {}
    }
    filter.validate()
}

fn validate_idle(value: Duration) -> Result<(), String> {
    if !is_unlimited(value) && value < Duration::from_mins(5) {
        return Err("Auto-delete on idle must be at least 300 seconds or unlimited.".into());
    }
    Ok(())
}

fn validate_lock_and_delivery(lock: Duration, count: i32) -> Result<(), String> {
    if !(Duration::from_secs(5)..=Duration::from_mins(5)).contains(&lock) {
        return Err("Lock duration must be between 5 and 300 seconds.".into());
    }
    if count < 1 {
        return Err("Max delivery count must be positive.".into());
    }
    Ok(())
}

fn validate_size(size: i64, message_size: Option<i64>) -> Result<(), String> {
    if size < 1 || message_size.is_some_and(|s| s < 1) {
        return Err("Entity and message sizes must be positive.".into());
    }
    Ok(())
}

fn validate_dedup(value: Duration) -> Result<(), String> {
    if !(Duration::from_secs(20)..=Duration::from_hours(168)).contains(&value) {
        return Err("Duplicate detection window must be between 20 seconds and 7 days.".into());
    }
    Ok(())
}

fn validate_forward(target: Option<&str>, source: &str) -> Result<(), String> {
    if target.is_some_and(|t| t.eq_ignore_ascii_case(source)) {
        return Err("An entity cannot forward messages to itself.".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    Save,
    Close,
}

pub fn show(ctx: &egui::Context, dialog: &mut EditDialog) -> Option<EditAction> {
    let mut action = None;
    let saving = dialog.saving.is_some();
    let modal = egui::Modal::new(egui::Id::new("edit-entity")).show(ctx, |ui| {
        ui.set_width(560.0);
        let path = dialog.description.path();
        ui.heading(format!("Edit {} '{}'", path.kind(), path.name()));
        ui.label(egui::RichText::new("Entity names and creation-only settings cannot be changed.").weak());
        ui.add_space(8.0);
        ui.add_enabled_ui(!saving, |ui| {
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() * 0.65).max(120.0)).show(ui, |ui| {
                egui::Grid::new("edit-fields").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    match &mut dialog.description {
                        EntityDescription::Queue(p) => {
                            creation_flags(ui, Some(p.requires_session), Some(p.requires_duplicate_detection), Some(p.enable_partitioning));
                            status(ui, &mut p.status);
                            duration(ui, "Default TTL (s)", &mut dialog.ttl, true);
                            duration(ui, "Auto-delete on idle (s)", &mut dialog.auto_delete, true);
                            duration(ui, "Lock duration (s)", &mut dialog.lock, false);
                            number(ui, "Max delivery count", &mut p.max_delivery_count);
                            number(ui, "Max size (MB)", &mut p.max_size_in_megabytes);
                            if p.requires_duplicate_detection { duration(ui, "Duplicate detection window (s)", &mut dialog.dedup, false); }
                            checkbox(ui, "Dead-letter expired messages", &mut p.dead_lettering_on_message_expiration);
                            checkbox(ui, "Batched operations", &mut p.enable_batched_operations);
                            checkbox(ui, "Express", &mut p.enable_express);
                            optional(ui, "Forward to", &mut p.forward_to);
                            optional(ui, "Forward DLQ to", &mut p.forward_dead_lettered_messages_to);
                            optional(ui, "User metadata", &mut p.user_metadata);
                            max_message_size(ui, &mut p.max_message_size_in_kilobytes, dialog.premium);
                        }
                        EntityDescription::Topic(p) => {
                            creation_flags(ui, None, Some(p.requires_duplicate_detection), Some(p.enable_partitioning));
                            status(ui, &mut p.status);
                            duration(ui, "Default TTL (s)", &mut dialog.ttl, true);
                            duration(ui, "Auto-delete on idle (s)", &mut dialog.auto_delete, true);
                            number(ui, "Max size (MB)", &mut p.max_size_in_megabytes);
                            if p.requires_duplicate_detection { duration(ui, "Duplicate detection window (s)", &mut dialog.dedup, false); }
                            checkbox(ui, "Support ordering", &mut p.support_ordering);
                            checkbox(ui, "Batched operations", &mut p.enable_batched_operations);
                            checkbox(ui, "Express", &mut p.enable_express);
                            optional(ui, "User metadata", &mut p.user_metadata);
                            max_message_size(ui, &mut p.max_message_size_in_kilobytes, dialog.premium);
                        }
                        EntityDescription::Subscription(p) => {
                            creation_flags(ui, Some(p.requires_session), None, None);
                            status(ui, &mut p.status);
                            duration(ui, "Default TTL (s)", &mut dialog.ttl, true);
                            duration(ui, "Auto-delete on idle (s)", &mut dialog.auto_delete, true);
                            duration(ui, "Lock duration (s)", &mut dialog.lock, false);
                            number(ui, "Max delivery count", &mut p.max_delivery_count);
                            checkbox(ui, "Dead-letter expired messages", &mut p.dead_lettering_on_message_expiration);
                            checkbox(ui, "Dead-letter filter errors", &mut p.dead_lettering_on_filter_evaluation_exceptions);
                            checkbox(ui, "Batched operations", &mut p.enable_batched_operations);
                            optional(ui, "Forward to", &mut p.forward_to);
                            optional(ui, "Forward DLQ to", &mut p.forward_dead_lettered_messages_to);
                            optional(ui, "User metadata", &mut p.user_metadata);
                        }
                        EntityDescription::Rule(p) => {
                            rule_filter(ui, &mut p.filter, &mut dialog.correlation_property_types);
                            optional(ui, "SQL action", &mut p.action);
                        }
                    }
                });
            });
            if matches!(dialog.description, EntityDescription::Rule(_)) {
                ui.add_space(8.0);
                ui.label("Saving replaces the existing rule. Routing may change while it is absent. If the replacement fails, sift will attempt to restore the original rule.");
                ui.checkbox(&mut dialog.confirm_rule_replacement, "Replace this rule");
            }
        });
        if let Some(error) = &dialog.error {
            egui::ScrollArea::vertical().id_salt("edit-error").max_height(80.0).show(ui, |ui| {
                ui.colored_label(ui.visuals().error_fg_color, error);
            });
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if saving { ui.spinner(); ui.label("Saving…"); }
            let confirmed = !matches!(dialog.description, EntityDescription::Rule(_)) || dialog.confirm_rule_replacement;
            if ui.add_enabled(!saving && confirmed, egui::Button::new("Save")).clicked() { action = Some(EditAction::Save); }
            if ui.add_enabled(!saving, egui::Button::new("Cancel")).clicked() { action = Some(EditAction::Close); }
        });
    });
    if modal.should_close() && !saving && action.is_none() {
        action = Some(EditAction::Close);
    }
    action
}

fn creation_flags(
    ui: &mut egui::Ui,
    session: Option<bool>,
    duplicate: Option<bool>,
    partitioning: Option<bool>,
) {
    ui.label("Creation settings");
    ui.vertical(|ui| {
        for (label, value) in [
            ("Requires session", session),
            ("Duplicate detection", duplicate),
            ("Partitioning", partitioning),
        ] {
            if let Some(mut value) = value {
                ui.add_enabled(false, egui::Checkbox::new(&mut value, label));
            }
        }
    });
    ui.end_row();
}

fn status(ui: &mut egui::Ui, value: &mut EntityStatus) {
    ui.label("Status");
    egui::ComboBox::from_id_salt("edit-status")
        .selected_text(value.to_string())
        .show_ui(ui, |ui| {
            for status in EntityStatus::ALL {
                ui.selectable_value(value, status, status.to_string());
            }
        });
    ui.end_row();
}

fn duration(ui: &mut egui::Ui, label: &str, value: &mut DurationField, allow_unlimited: bool) {
    ui.label(label);
    ui.add(
        egui::TextEdit::singleline(&mut value.text)
            .hint_text(if allow_unlimited {
                "seconds or unlimited"
            } else {
                "seconds"
            })
            .desired_width(f32::INFINITY),
    );
    ui.end_row();
}

fn number<T: egui::emath::Numeric>(ui: &mut egui::Ui, label: &str, value: &mut T) {
    ui.label(label);
    ui.add(egui::DragValue::new(value));
    ui.end_row();
}

fn checkbox(ui: &mut egui::Ui, label: &str, value: &mut bool) {
    ui.label(label);
    ui.checkbox(value, "Enabled");
    ui.end_row();
}

fn optional(ui: &mut egui::Ui, label: &str, value: &mut Option<String>) {
    ui.label(label);
    let _ = optional_input(ui, value);
    ui.end_row();
}

fn optional_input(ui: &mut egui::Ui, value: &mut Option<String>) -> egui::Response {
    let mut text = value.clone().unwrap_or_default();
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text("optional")
            .desired_width(f32::INFINITY),
    );
    if response.changed() {
        *value = Some(text);
    }
    response
}

fn max_message_size(ui: &mut egui::Ui, value: &mut Option<i64>, premium: bool) {
    ui.label("Max message size (KB)");
    ui.add_enabled_ui(premium, |ui| {
        let mut custom = value.is_some();
        if ui.checkbox(&mut custom, "Custom").changed() {
            *value = custom.then_some(1024);
        }
        if let Some(value) = value {
            ui.add(egui::DragValue::new(value));
        }
    })
    .response
    .on_hover_text("Configurable on Premium namespaces; other tiers retain their current value.");
    ui.end_row();
}

#[allow(clippy::too_many_lines)] // The four filter forms share one type selector.
fn rule_filter(
    ui: &mut egui::Ui,
    filter: &mut RuleFilter,
    types: &mut Vec<CorrelationPropertyType>,
) {
    let current = match filter {
        RuleFilter::Sql { .. } => 0,
        RuleFilter::Correlation { .. } => 1,
        RuleFilter::True => 2,
        RuleFilter::False => 3,
    };
    let labels = ["SQL", "Correlation", "True", "False"];
    let mut selected = current;
    ui.label("Filter type");
    egui::ComboBox::from_id_salt("edit-rule-kind")
        .selected_text(labels[current])
        .show_ui(ui, |ui| {
            for (index, label) in labels.iter().enumerate() {
                ui.selectable_value(&mut selected, index, *label);
            }
        });
    ui.end_row();
    if selected != current {
        types.clear();
        *filter = match selected {
            0 => RuleFilter::Sql {
                expression: "1=1".into(),
            },
            1 => RuleFilter::Correlation {
                correlation_id: None,
                message_id: None,
                to: None,
                reply_to: None,
                subject: None,
                session_id: None,
                reply_to_session_id: None,
                content_type: None,
                properties: Vec::new(),
                property_types: std::collections::BTreeMap::default(),
            },
            2 => RuleFilter::True,
            _ => RuleFilter::False,
        };
    }
    match filter {
        RuleFilter::Sql { expression } => {
            ui.label("SQL filter");
            ui.text_edit_singleline(expression);
            ui.end_row();
        }
        RuleFilter::Correlation {
            correlation_id,
            message_id,
            to,
            reply_to,
            subject,
            session_id,
            reply_to_session_id,
            content_type,
            properties,
            property_types: _,
        } => {
            optional(ui, "Correlation id", correlation_id);
            optional(ui, "Message id", message_id);
            optional(ui, "To", to);
            optional(ui, "Reply to", reply_to);
            optional(ui, "Subject", subject);
            optional(ui, "Session id", session_id);
            optional(ui, "Reply-to session id", reply_to_session_id);
            optional(ui, "Content type", content_type);
            ui.label("Application properties");
            ui.vertical(|ui| {
                let mut remove = None;
                for (index, (key, value)) in properties.iter_mut().enumerate() {
                    if types.len() <= index {
                        types.resize(index + 1, CorrelationPropertyType::String);
                    }
                    ui.push_id(index, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(key)
                                    .hint_text("name")
                                    .desired_width(140.0),
                            );
                            egui::ComboBox::from_id_salt("property-type")
                                .selected_text(types[index].as_str())
                                .show_ui(ui, |ui| {
                                    for kind in CorrelationPropertyType::ALL {
                                        ui.selectable_value(&mut types[index], kind, kind.as_str());
                                    }
                                });
                            ui.add(
                                egui::TextEdit::singleline(value)
                                    .hint_text("value")
                                    .desired_width(140.0),
                            );
                            if ui.small_button("Remove").clicked() {
                                remove = Some(index);
                            }
                        });
                    });
                }
                if let Some(index) = remove {
                    properties.remove(index);
                    types.remove(index);
                }
                if ui.button("Add property").clicked() {
                    properties.push((String::new(), String::new()));
                    types.push(CorrelationPropertyType::String);
                }
            });
            ui.end_row();
        }
        RuleFilter::True | RuleFilter::False => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_mgmt::{QueueInfo, QueueProperties, RuleInfo, RuleProperties};

    fn queue(properties: QueueProperties) -> EditDialog {
        EditDialog::new(
            NamespaceId::nil(),
            &EntityInfo::Queue(QueueInfo {
                properties,
                runtime: sift_mgmt::EntityRuntimeInfo::default(),
            }),
            Some("Standard"),
        )
    }

    #[test]
    fn optional_input_keeps_spaces_between_text_events() {
        let mut action = None;
        let ctx = egui::Context::default();
        for (frame, text) in [None, Some("SET "), Some("seen = true")]
            .into_iter()
            .enumerate()
        {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events: text
                    .map(|text| vec![egui::Event::Text(text.into())])
                    .unwrap_or_default(),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                let response = optional_input(ui, &mut action);
                if frame == 0 {
                    response.request_focus();
                }
            });
            if frame == 1 {
                assert_eq!(action.as_deref(), Some("SET "));
            }
        }
        assert_eq!(action.as_deref(), Some("SET seen = true"));
    }

    #[test]
    fn saving_normalizes_identifiers_and_blank_fields_without_changing_the_draft() {
        let mut editor = queue(QueueProperties {
            name: "orders".into(),
            forward_to: Some(" archive ".into()),
            forward_dead_lettered_messages_to: Some(" \t ".into()),
            user_metadata: Some(" keep metadata spaces ".into()),
            ..Default::default()
        });
        let Some(EntityDescription::Queue(updated)) = editor.build() else {
            panic!("valid queue edit");
        };
        assert_eq!(updated.forward_to.as_deref(), Some("archive"));
        assert_eq!(updated.forward_dead_lettered_messages_to, None);
        assert_eq!(
            updated.user_metadata.as_deref(),
            Some(" keep metadata spaces ")
        );
        let EntityDescription::Queue(draft) = &mut editor.description else {
            panic!("queue draft remains available");
        };
        assert_eq!(draft.forward_to.as_deref(), Some(" archive "));
        assert_eq!(
            draft.forward_dead_lettered_messages_to.as_deref(),
            Some(" \t ")
        );
        draft.forward_to = Some(" ORDERS ".into());
        assert!(
            editor.build().is_none(),
            "trim before self-forward validation"
        );
    }

    #[test]
    fn editing_one_property_preserves_creation_settings_and_exact_durations() {
        let properties = QueueProperties {
            name: "orders".into(),
            requires_session: true,
            requires_duplicate_detection: true,
            enable_partitioning: true,
            enable_express: true,
            forward_to: Some("archive".into()),
            default_message_time_to_live: Duration::new(900, 123_456_789),
            max_message_size_in_kilobytes: Some(1024),
            ..Default::default()
        };
        let mut editor = queue(properties.clone());
        if let EntityDescription::Queue(p) = &mut editor.description {
            p.max_delivery_count = 14;
        }
        let Some(EntityDescription::Queue(updated)) = editor.build() else {
            panic!("queue edit validated");
        };
        let mut expected = properties;
        expected.max_delivery_count = 14;
        assert_eq!(updated, expected);
    }

    #[test]
    fn rejects_invalid_durations_and_self_forwarding_without_losing_edits() {
        let mut editor = queue(QueueProperties {
            name: "orders".into(),
            ..Default::default()
        });
        editor.lock.text = "301".into();
        assert!(editor.build().is_none());
        assert_eq!(editor.lock.text, "301");
        editor.lock.text = "60".into();
        editor.auto_delete.text = "299".into();
        assert!(editor.build().is_none());
        editor.auto_delete.text = "unlimited".into();
        if let EntityDescription::Queue(p) = &mut editor.description {
            p.forward_to = Some("ORDERS".into());
        }
        assert!(editor.build().is_none());
        editor.ttl.text = "NaN".into();
        assert!(editor.build().is_none());
    }

    #[test]
    fn failed_request_retains_fields_and_ignores_other_request_results() {
        let mut editor = queue(QueueProperties {
            name: "orders".into(),
            ..Default::default()
        });
        let req = RequestId(7);
        editor.saving = Some(req);
        editor.ttl.text = "3600".into();
        assert!(!editor.complete(RequestId(8), &Ok(None)));
        assert_eq!(editor.saving, Some(req));
        assert!(!editor.complete(req, &Err(BackendError::new("forbidden"))));
        assert_eq!(editor.ttl.text, "3600");
        assert_eq!(editor.error.as_deref(), Some("forbidden"));
        assert_eq!(editor.saving, None);
    }

    #[test]
    fn updates_topic_and_subscription_durations_without_resetting_other_values() {
        let topic = sift_mgmt::TopicProperties {
            name: "events".into(),
            requires_duplicate_detection: true,
            enable_partitioning: true,
            user_metadata: Some("keep topic metadata".into()),
            ..Default::default()
        };
        let mut editor = EditDialog::new(
            NamespaceId::nil(),
            &EntityInfo::Topic(sift_mgmt::TopicInfo {
                properties: topic.clone(),
                subscription_count: 2,
                size_in_bytes: 0,
                scheduled_message_count: 0,
                created_at: None,
                updated_at: None,
                accessed_at: None,
            }),
            None,
        );
        editor.dedup.text = "19".into();
        assert!(editor.build().is_none());
        editor.dedup.text = "20".into();
        let Some(EntityDescription::Topic(updated)) = editor.build() else {
            panic!("valid topic edit");
        };
        let mut expected = topic;
        expected.duplicate_detection_history_time_window = Duration::from_secs(20);
        assert_eq!(updated, expected);

        let subscription = sift_mgmt::SubscriptionProperties {
            topic: "events".into(),
            name: "audit".into(),
            requires_session: true,
            forward_dead_lettered_messages_to: Some("audit-errors".into()),
            ..Default::default()
        };
        let mut editor = EditDialog::new(
            NamespaceId::nil(),
            &EntityInfo::Subscription(sift_mgmt::SubscriptionInfo {
                properties: subscription.clone(),
                runtime: sift_mgmt::EntityRuntimeInfo::default(),
            }),
            None,
        );
        editor.ttl.text = "7200".into();
        let Some(EntityDescription::Subscription(updated)) = editor.build() else {
            panic!("valid subscription edit");
        };
        let mut expected = subscription;
        expected.default_message_time_to_live = Duration::from_secs(7200);
        assert_eq!(updated, expected);
    }

    #[test]
    fn rule_replacement_requires_confirmation_and_preserves_correlation_fields() {
        let properties = RuleProperties {
            topic: "events".into(),
            subscription: "audit".into(),
            name: "filter".into(),
            filter: RuleFilter::Correlation {
                correlation_id: Some(" correlation ".into()),
                message_id: None,
                to: Some("destination".into()),
                reply_to: None,
                subject: Some("subject".into()),
                session_id: Some("session".into()),
                reply_to_session_id: None,
                content_type: Some("application/json".into()),
                properties: vec![
                    ("origin".into(), "sift".into()),
                    (" count ".into(), "7".into()),
                ],
                property_types: [(" count ".into(), CorrelationPropertyType::Int)].into(),
            },
            action: Some("SET seen = true".into()),
        };
        let mut editor = EditDialog::new(
            NamespaceId::nil(),
            &EntityInfo::Rule(RuleInfo {
                properties: properties.clone(),
                created_at: None,
            }),
            None,
        );
        assert!(editor.build().is_none());
        editor.confirm_rule_replacement = true;
        let Some(EntityDescription::Rule(updated)) = editor.build() else {
            panic!("confirmed rule edit validated");
        };
        assert_eq!(updated, properties);
        if let EntityDescription::Rule(p) = &mut editor.description {
            p.action = Some(" SET seen = true ".into());
            if let RuleFilter::Correlation { properties, .. } = &mut p.filter {
                properties[1].0 = " total ".into();
            }
        }
        let Some(EntityDescription::Rule(updated)) = editor.build() else {
            panic!("renamed typed correlation property validates");
        };
        assert_eq!(updated.action.as_deref(), Some(" SET seen = true "));
        let RuleFilter::Correlation {
            properties,
            property_types,
            ..
        } = updated.filter
        else {
            panic!("correlation filter retained");
        };
        assert_eq!(properties[1], (" total ".into(), "7".into()));
        assert_eq!(
            property_types.get(" total "),
            Some(&CorrelationPropertyType::Int)
        );
        assert!(!property_types.contains_key(" count "));
    }
}
