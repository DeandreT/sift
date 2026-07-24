//! Modal dialogs: typed-name delete confirmation (the entity name must be
//! typed back to arm the delete button) and entity creation forms.

use sift_backend::{EntityDescription, EntityPath};
use sift_mgmt::{
    QueueProperties, RuleFilter, RuleProperties, SubscriptionProperties, TopicProperties,
};

use crate::state::CreateKind;

// ---- delete confirmation ----------------------------------------------------

#[derive(Debug)]
pub struct ConfirmDeleteDialog {
    pub path: EntityPath,
    pub typed: String,
    /// When the config disables the typed-name guard, a plain button suffices.
    pub require_typed_name: bool,
}

impl ConfirmDeleteDialog {
    #[must_use]
    pub fn new(path: EntityPath, require_typed_name: bool) -> Self {
        Self {
            path,
            typed: String::new(),
            require_typed_name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmDeleteAction {
    Delete,
    Close,
}

pub fn show_confirm_delete(
    ctx: &egui::Context,
    dialog: &mut ConfirmDeleteDialog,
) -> Option<ConfirmDeleteAction> {
    let mut action = None;

    let modal = egui::Modal::new(egui::Id::new("confirm-delete")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.heading(format!("Delete {}", dialog.path.kind()));
        ui.add_space(8.0);
        ui.label(format!(
            "This permanently deletes '{}' and everything in it.",
            dialog.path
        ));

        let name = dialog.path.name().to_owned();
        let confirmed = if dialog.require_typed_name {
            ui.add_space(8.0);
            ui.label(format!("Type the {} name to confirm:", dialog.path.kind()));
            ui.add(
                egui::TextEdit::singleline(&mut dialog.typed)
                    .hint_text(&name)
                    .desired_width(f32::INFINITY),
            );
            dialog.typed.trim().eq_ignore_ascii_case(&name)
        } else {
            true
        };

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let delete =
                egui::Button::new(egui::RichText::new("Delete").color(ui.visuals().error_fg_color));
            if ui.add_enabled(confirmed, delete).clicked() {
                action = Some(ConfirmDeleteAction::Delete);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    action = Some(ConfirmDeleteAction::Close);
                }
            });
        });
    });

    if modal.should_close() && action.is_none() {
        action = Some(ConfirmDeleteAction::Close);
    }
    action
}

// ---- entity creation ------------------------------------------------------------

/// Form buffer for the create dialog; validated into an [`EntityDescription`]
/// on submit.
#[derive(Debug)]
pub struct CreateDialog {
    pub kind: CreateKind,
    pub name: String,
    pub requires_session: bool,
    pub requires_duplicate_detection: bool,
    pub enable_partitioning: bool,
    pub dead_lettering_on_message_expiration: bool,
    pub support_ordering: bool,
    pub max_delivery_count: i32,
    pub lock_duration_secs: u32,
    /// SQL expression for new rules.
    pub sql_filter: String,
    pub sql_action: String,
    pub error: Option<String>,
}

impl CreateDialog {
    #[must_use]
    pub fn new(kind: CreateKind) -> Self {
        Self {
            kind,
            name: String::new(),
            requires_session: false,
            requires_duplicate_detection: false,
            enable_partitioning: false,
            dead_lettering_on_message_expiration: false,
            support_ordering: false,
            max_delivery_count: 10,
            lock_duration_secs: 60,
            sql_filter: "1=1".into(),
            sql_action: String::new(),
            error: None,
        }
    }

    fn title(&self) -> String {
        match &self.kind {
            CreateKind::Queue => "Create queue".into(),
            CreateKind::Topic => "Create topic".into(),
            CreateKind::Subscription { topic } => format!("Create subscription on '{topic}'"),
            CreateKind::Rule {
                topic,
                subscription,
            } => format!("Add rule to '{topic}/{subscription}'"),
        }
    }

    /// Validate the buffer into a description; sets `error` on failure.
    pub fn build(&mut self) -> Option<EntityDescription> {
        let name = self.name.trim();
        if name.is_empty() {
            self.error = Some("Enter a name.".into());
            return None;
        }
        if name.len() > 260 || name.starts_with('/') || name.ends_with('/') {
            self.error = Some(
                "Entity names must be at most 260 characters and cannot start or end with '/'."
                    .into(),
            );
            return None;
        }
        let lock_duration = std::time::Duration::from_secs(u64::from(self.lock_duration_secs));

        Some(match &self.kind {
            CreateKind::Queue => EntityDescription::Queue(QueueProperties {
                name: name.into(),
                lock_duration,
                requires_session: self.requires_session,
                requires_duplicate_detection: self.requires_duplicate_detection,
                enable_partitioning: self.enable_partitioning,
                dead_lettering_on_message_expiration: self.dead_lettering_on_message_expiration,
                max_delivery_count: self.max_delivery_count,
                ..QueueProperties::default()
            }),
            CreateKind::Topic => EntityDescription::Topic(TopicProperties {
                name: name.into(),
                requires_duplicate_detection: self.requires_duplicate_detection,
                enable_partitioning: self.enable_partitioning,
                support_ordering: self.support_ordering,
                ..TopicProperties::default()
            }),
            CreateKind::Subscription { topic } => {
                EntityDescription::Subscription(SubscriptionProperties {
                    topic: topic.clone(),
                    name: name.into(),
                    lock_duration,
                    requires_session: self.requires_session,
                    dead_lettering_on_message_expiration: self.dead_lettering_on_message_expiration,
                    max_delivery_count: self.max_delivery_count,
                    ..SubscriptionProperties::default()
                })
            }
            CreateKind::Rule {
                topic,
                subscription,
            } => {
                let expression = self.sql_filter.trim();
                if expression.is_empty() {
                    self.error = Some("Enter a SQL filter expression (e.g. 1=1).".into());
                    return None;
                }
                EntityDescription::Rule(RuleProperties {
                    topic: topic.clone(),
                    subscription: subscription.clone(),
                    name: name.into(),
                    filter: RuleFilter::Sql {
                        expression: expression.into(),
                    },
                    action: Some(self.sql_action.trim().to_owned()).filter(|a| !a.is_empty()),
                })
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateAction {
    Create,
    Close,
}

pub fn show_create(ctx: &egui::Context, dialog: &mut CreateDialog) -> Option<CreateAction> {
    let mut action = None;

    let modal = egui::Modal::new(egui::Id::new("create-entity")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.heading(dialog.title());
        ui.add_space(8.0);

        egui::Grid::new("create-fields")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut dialog.name).desired_width(f32::INFINITY));
                ui.end_row();

                match dialog.kind.clone() {
                    CreateKind::Queue => {
                        queue_like_fields(ui, dialog, true);
                    }
                    CreateKind::Topic => {
                        ui.label("Options");
                        ui.vertical(|ui| {
                            ui.checkbox(
                                &mut dialog.requires_duplicate_detection,
                                "Duplicate detection",
                            );
                            ui.checkbox(&mut dialog.enable_partitioning, "Partitioning");
                            ui.checkbox(&mut dialog.support_ordering, "Support ordering");
                        });
                        ui.end_row();
                    }
                    CreateKind::Subscription { .. } => {
                        queue_like_fields(ui, dialog, false);
                    }
                    CreateKind::Rule { .. } => {
                        ui.label("SQL filter");
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.sql_filter)
                                .hint_text("e.g. priority > 3")
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();
                        ui.label("SQL action");
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.sql_action)
                                .hint_text("optional, e.g. SET processed = 'true'")
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();
                    }
                }
            });

        if let Some(error) = &dialog.error {
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.add_space(12.0);

        ui.horizontal(|ui| {
            if ui.button("Create").clicked() {
                action = Some(CreateAction::Create);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    action = Some(CreateAction::Close);
                }
            });
        });
    });

    if modal.should_close() && action.is_none() {
        action = Some(CreateAction::Close);
    }
    action
}

/// Fields shared by queue and subscription forms; `partitioning` only applies
/// to queues.
fn queue_like_fields(ui: &mut egui::Ui, dialog: &mut CreateDialog, partitioning: bool) {
    ui.label("Options");
    ui.vertical(|ui| {
        ui.checkbox(&mut dialog.requires_session, "Requires session");
        ui.checkbox(
            &mut dialog.dead_lettering_on_message_expiration,
            "Dead-letter expired messages",
        );
        if partitioning {
            ui.checkbox(
                &mut dialog.requires_duplicate_detection,
                "Duplicate detection",
            );
            ui.checkbox(&mut dialog.enable_partitioning, "Partitioning");
        }
    });
    ui.end_row();

    ui.label("Max delivery count");
    ui.add(egui::DragValue::new(&mut dialog.max_delivery_count).range(1..=2000));
    ui.end_row();

    ui.label("Lock duration (s)");
    ui.add(egui::DragValue::new(&mut dialog.lock_duration_secs).range(5..=300));
    ui.end_row();
}
