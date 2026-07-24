//! Left-hand panel: connection header and the lazily-loaded entity tree.
//!
//! Rendering is intentionally widget-simple (CollapsingHeader-based); the
//! model lives in [`EntityTree`], so swapping in a richer tree widget later
//! only touches this file.

use sift_backend::EntityPath;
use sift_mgmt::{MessageCountDetails, QueueInfo, RuleInfo, SubscriptionInfo, TopicInfo};

use crate::state::{AppAction, ConnectionState, CreateKind, EntityTree, Loadable, TreeFilter};

pub fn show(
    ui: &mut egui::Ui,
    conn: &ConnectionState,
    tree: &mut EntityTree,
    actions: &mut Vec<AppAction>,
) {
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
                    .small_button(egui_phosphor::regular::ARROWS_CLOCKWISE)
                    .on_hover_text("Refresh all")
                    .clicked()
                {
                    actions.push(AppAction::RefreshTree);
                }
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
            ui.label(egui::RichText::new(subtitle).weak().small());

            filter_box(ui, &mut tree.filter);
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    queues_folder(ui, tree, actions);
                    topics_folder(ui, tree, actions);
                });
        }
    }
}

/// Filter text box with a clear button; focused on demand (Ctrl+F).
fn filter_box(ui: &mut egui::Ui, filter: &mut TreeFilter) {
    ui.horizontal(|ui| {
        ui.label(egui_phosphor::regular::MAGNIFYING_GLASS);
        let show_clear = !filter.text.is_empty();

        // Give the field an explicit finite width. Using `desired_width` of
        // `f32::INFINITY` with a trailing button inside a resizable panel
        // feeds back into the panel width and grows it without bound.
        let reserve = if show_clear { 28.0 } else { 6.0 };
        let width = (ui.available_width() - reserve).max(24.0);
        let response = ui.add_sized(
            [width, ui.spacing().interact_size.y],
            egui::TextEdit::singleline(&mut filter.text).hint_text("Filter (Ctrl+F)"),
        );
        if response.changed() {
            filter.on_edit();
        }
        if filter.focus_requested {
            response.request_focus();
            filter.focus_requested = false;
        }
        if show_clear
            && ui
                .add(egui::Button::new("✕").frame(false))
                .on_hover_text("Clear filter")
                .clicked()
        {
            filter.clear();
        }
    });
}

// ---- folders ----------------------------------------------------------------

fn queues_folder(ui: &mut egui::Ui, tree: &EntityTree, actions: &mut Vec<AppAction>) {
    let title = format!(
        "{} Queues{}",
        egui_phosphor::regular::TRAY,
        list_suffix(&tree.queues)
    );
    let header = egui::CollapsingHeader::new(title)
        .id_salt("queues-folder")
        .show(ui, |ui| match &tree.queues {
            Loadable::NotLoaded => actions.push(AppAction::LoadQueues),
            Loadable::Loading => {
                ui.spinner();
            }
            Loadable::Failed(e) => failed_row(ui, e, AppAction::LoadQueues, actions),
            Loadable::Loaded(queues) => {
                let visible: Vec<&QueueInfo> = queues
                    .iter()
                    .filter(|q| tree.filter.matches(&q.properties.name))
                    .collect();
                if visible.is_empty() {
                    ui.label(egui::RichText::new("no queues").weak());
                }
                for queue in visible {
                    queue_row(ui, queue, actions);
                }
            }
        });
    header.header_response.context_menu(|ui| {
        if ui.button("Create queue…").clicked() {
            actions.push(AppAction::OpenCreateDialog(CreateKind::Queue));
            ui.close();
        }
        if ui.button("Refresh").clicked() {
            actions.push(AppAction::LoadQueues);
            ui.close();
        }
    });
}

fn topics_folder(ui: &mut egui::Ui, tree: &EntityTree, actions: &mut Vec<AppAction>) {
    let title = format!(
        "{} Topics{}",
        egui_phosphor::regular::BROADCAST,
        list_suffix(&tree.topics)
    );
    let header = egui::CollapsingHeader::new(title)
        .id_salt("topics-folder")
        .show(ui, |ui| match &tree.topics {
            Loadable::NotLoaded => actions.push(AppAction::LoadTopics),
            Loadable::Loading => {
                ui.spinner();
            }
            Loadable::Failed(e) => failed_row(ui, e, AppAction::LoadTopics, actions),
            Loadable::Loaded(topics) => {
                let visible: Vec<&TopicInfo> =
                    topics.iter().filter(|t| topic_visible(t, tree)).collect();
                if visible.is_empty() {
                    ui.label(egui::RichText::new("no topics").weak());
                }
                for topic in visible {
                    topic_node(ui, topic, tree, actions);
                }
            }
        });
    header.header_response.context_menu(|ui| {
        if ui.button("Create topic…").clicked() {
            actions.push(AppAction::OpenCreateDialog(CreateKind::Topic));
            ui.close();
        }
        if ui.button("Refresh").clicked() {
            actions.push(AppAction::LoadTopics);
            ui.close();
        }
    });
}

// ---- rows ---------------------------------------------------------------------

fn queue_row(ui: &mut egui::Ui, queue: &QueueInfo, actions: &mut Vec<AppAction>) {
    let path = EntityPath::Queue(queue.properties.name.clone());
    let label = entity_label(
        ui,
        egui_phosphor::regular::TRAY,
        &queue.properties.name,
        Some(&queue.runtime.count_details),
    );
    let response = ui.selectable_label(false, label);
    if response.clicked() {
        actions.push(AppAction::OpenEntity(path.clone()));
    }
    response.context_menu(|ui| {
        entity_context_menu(ui, &path, AppAction::LoadQueues, actions);
    });
}

fn topic_node(
    ui: &mut egui::Ui,
    topic: &TopicInfo,
    tree: &EntityTree,
    actions: &mut Vec<AppAction>,
) {
    let name = &topic.properties.name;
    let path = EntityPath::Topic(name.clone());
    let subs = tree.subscriptions.get(name);
    let title = format!(
        "{} {name}{}",
        egui_phosphor::regular::BROADCAST,
        subs.map_or_else(|| format!(" ({})", topic.subscription_count), list_suffix)
    );

    // With an active filter, expand so matching subscriptions are visible; a
    // topic that matches by name shows all its subscriptions.
    let topic_matches = tree.filter.matches(name);
    let mut header = egui::CollapsingHeader::new(title).id_salt(("topic", name));
    if tree.filter.is_active() {
        header = header.open(Some(true));
    }
    let header = header.show(ui, |ui| match subs {
        None | Some(Loadable::NotLoaded) => {
            actions.push(AppAction::LoadSubscriptions(name.clone()));
        }
        Some(Loadable::Loading) => {
            ui.spinner();
        }
        Some(Loadable::Failed(e)) => {
            failed_row(ui, e, AppAction::LoadSubscriptions(name.clone()), actions);
        }
        Some(Loadable::Loaded(subscriptions)) => {
            let visible: Vec<&SubscriptionInfo> = subscriptions
                .iter()
                .filter(|s| topic_matches || tree.filter.matches(&s.properties.name))
                .collect();
            if visible.is_empty() {
                ui.label(egui::RichText::new("no subscriptions").weak());
            }
            for subscription in visible {
                subscription_node(ui, subscription, tree, actions);
            }
        }
    });
    header.header_response.context_menu(|ui| {
        if ui.button("Open").clicked() {
            actions.push(AppAction::OpenEntity(path.clone()));
            ui.close();
        }
        if ui.button("Create subscription…").clicked() {
            actions.push(AppAction::OpenCreateDialog(CreateKind::Subscription {
                topic: name.clone(),
            }));
            ui.close();
        }
        if ui.button("Refresh").clicked() {
            actions.push(AppAction::LoadSubscriptions(name.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button("Delete…").clicked() {
            actions.push(AppAction::RequestDelete(path.clone()));
            ui.close();
        }
    });
}

fn subscription_node(
    ui: &mut egui::Ui,
    subscription: &SubscriptionInfo,
    tree: &EntityTree,
    actions: &mut Vec<AppAction>,
) {
    let topic = subscription.properties.topic.clone();
    let name = subscription.properties.name.clone();
    let path = EntityPath::Subscription {
        topic: topic.clone(),
        name: name.clone(),
    };
    let key = (topic.clone(), name.clone());
    let rules = tree.rules.get(&key);

    let label = entity_label(
        ui,
        egui_phosphor::regular::ENVELOPE,
        &name,
        Some(&subscription.runtime.count_details),
    );

    // Custom header so the disclosure triangle expands the rules while a
    // click on the name opens the subscription (matches queue behavior).
    let id = ui.make_persistent_id(("subscription", &topic, &name));
    let state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let header = state.show_header(ui, |ui| {
        let response = ui
            .selectable_label(false, label)
            .on_hover_text("Click to open");
        if response.clicked() {
            actions.push(AppAction::OpenEntity(path.clone()));
        }
        response.context_menu(|ui| {
            if ui.button("Open").clicked() {
                actions.push(AppAction::OpenEntity(path.clone()));
                ui.close();
            }
            if ui.button("Add rule…").clicked() {
                actions.push(AppAction::OpenCreateDialog(CreateKind::Rule {
                    topic: topic.clone(),
                    subscription: name.clone(),
                }));
                ui.close();
            }
            if ui.button("Refresh rules").clicked() {
                actions.push(AppAction::LoadRules(topic.clone(), name.clone()));
                ui.close();
            }
            ui.separator();
            if ui.button("Delete…").clicked() {
                actions.push(AppAction::RequestDelete(path.clone()));
                ui.close();
            }
        });
    });
    header.body(|ui| match rules {
        None | Some(Loadable::NotLoaded) => {
            actions.push(AppAction::LoadRules(topic.clone(), name.clone()));
        }
        Some(Loadable::Loading) => {
            ui.spinner();
        }
        Some(Loadable::Failed(e)) => {
            failed_row(
                ui,
                e,
                AppAction::LoadRules(topic.clone(), name.clone()),
                actions,
            );
        }
        Some(Loadable::Loaded(rules)) => {
            for rule in rules {
                rule_row(ui, rule, actions);
            }
        }
    });
}

fn rule_row(ui: &mut egui::Ui, rule: &RuleInfo, actions: &mut Vec<AppAction>) {
    let path = EntityPath::Rule {
        topic: rule.properties.topic.clone(),
        subscription: rule.properties.subscription.clone(),
        name: rule.properties.name.clone(),
    };
    let label = format!(
        "{} {}",
        egui_phosphor::regular::FUNNEL,
        rule.properties.name
    );
    let response = ui
        .selectable_label(false, label)
        .on_hover_text(rule.properties.filter.summary());
    if response.clicked() {
        actions.push(AppAction::OpenEntity(path.clone()));
    }
    response.context_menu(|ui| {
        if ui.button("Open").clicked() {
            actions.push(AppAction::OpenEntity(path.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button("Delete…").clicked() {
            actions.push(AppAction::RequestDelete(path.clone()));
            ui.close();
        }
    });
}

// ---- shared helpers -------------------------------------------------------------

/// `name (active, dead-letter, scheduled)`, tinted when the DLQ is non-empty.
fn entity_label(
    ui: &egui::Ui,
    icon: &str,
    name: &str,
    counts: Option<&MessageCountDetails>,
) -> egui::RichText {
    match counts {
        Some(c) => {
            let text = format!(
                "{icon} {name} ({}, {}, {})",
                c.active, c.dead_letter, c.scheduled
            );
            if c.dead_letter > 0 {
                egui::RichText::new(text).color(ui.visuals().warn_fg_color)
            } else {
                egui::RichText::new(text)
            }
        }
        None => egui::RichText::new(format!("{icon} {name}")),
    }
}

fn entity_context_menu(
    ui: &mut egui::Ui,
    path: &EntityPath,
    refresh: AppAction,
    actions: &mut Vec<AppAction>,
) {
    if ui.button("Open").clicked() {
        actions.push(AppAction::OpenEntity(path.clone()));
        ui.close();
    }
    if ui.button("Refresh").clicked() {
        actions.push(refresh);
        ui.close();
    }
    ui.separator();
    if ui.button("Delete…").clicked() {
        actions.push(AppAction::RequestDelete(path.clone()));
        ui.close();
    }
}

fn failed_row(ui: &mut egui::Ui, error: &str, retry: AppAction, actions: &mut Vec<AppAction>) {
    ui.colored_label(ui.visuals().error_fg_color, error);
    if ui.small_button("Retry").clicked() {
        actions.push(retry);
    }
}

fn list_suffix<T>(loadable: &Loadable<Vec<T>>) -> String {
    match loadable {
        Loadable::Loaded(items) => format!(" ({})", items.len()),
        _ => String::new(),
    }
}

/// A topic shows when its own name matches, or (once its subscriptions are
/// loaded) when any subscription matches. Unloaded subscriptions can't be
/// tested, so the topic stays visible until they arrive.
fn topic_visible(topic: &TopicInfo, tree: &EntityTree) -> bool {
    if !tree.filter.is_active() || tree.filter.matches(&topic.properties.name) {
        return true;
    }
    match tree.subscriptions.get(&topic.properties.name) {
        Some(Loadable::Loaded(subs)) => {
            subs.iter().any(|s| tree.filter.matches(&s.properties.name))
        }
        _ => true,
    }
}
