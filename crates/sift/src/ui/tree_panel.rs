//! Left-hand panel: a filter box plus one collapsible section per connected
//! namespace, each showing its lazily-loaded entity tree.

use sift_backend::{EntityPath, NamespaceId};
use sift_mgmt::{MessageCountDetails, QueueInfo, RuleInfo, SubscriptionInfo, TopicInfo};

use crate::icons::{Icon, icon};
use crate::state::{
    AppAction, Connection, CreateKind, EntityTree, Loadable, ScopedEntity, TreeFilter,
};

pub fn show(
    ui: &mut egui::Ui,
    connections: &[Connection],
    filter: &mut TreeFilter,
    actions: &mut Vec<AppAction>,
) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let label = format!("{} Connect…", icon(Icon::Plug));
        if ui.button(label).clicked() {
            actions.push(AppAction::OpenConnectDialog);
        }
    });

    if connections.is_empty() {
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("Not connected").weak());
        });
        return;
    }

    filter_box(ui, filter);
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // A single connection defaults open; several start collapsed.
            let default_open = connections.len() == 1;
            for conn in connections {
                connection_section(ui, conn, filter, default_open, actions);
            }
        });
}

fn connection_section(
    ui: &mut egui::Ui,
    conn: &Connection,
    filter: &TreeFilter,
    default_open: bool,
    actions: &mut Vec<AppAction>,
) {
    let ns = conn.profile_id;
    let mut title = format!("{} {}", icon(Icon::Cable), conn.name);
    match &conn.info {
        Some(info) => {
            if let Some(sku) = &info.messaging_sku {
                title.push_str(" · ");
                title.push_str(sku);
            }
        }
        None => title.push_str(" · connecting…"),
    }

    let header = egui::CollapsingHeader::new(egui::RichText::new(title).strong())
        .id_salt(("connection", ns))
        .default_open(default_open)
        .show(ui, |ui| {
            if conn.is_connected() {
                queues_folder(ui, ns, &conn.tree, filter, actions);
                topics_folder(ui, ns, &conn.tree, filter, actions);
            } else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Connecting…");
                });
            }
        });

    header.header_response.context_menu(|ui| {
        if ui.button("Refresh").clicked() {
            actions.push(AppAction::RefreshTree(ns));
            ui.close();
        }
        if ui.button("Export entities…").clicked() {
            actions.push(AppAction::ExportNamespace(ns));
            ui.close();
        }
        if ui.button("Import entities (create missing)…").clicked() {
            actions.push(AppAction::ImportNamespace {
                ns,
                overwrite: false,
            });
            ui.close();
        }
        ui.separator();
        if ui.button("Disconnect").clicked() {
            actions.push(AppAction::Disconnect(ns));
            ui.close();
        }
    });
}

/// Filter text box with a clear button; focused on demand (Ctrl+F).
fn filter_box(ui: &mut egui::Ui, filter: &mut TreeFilter) {
    ui.horizontal(|ui| {
        ui.label(icon(Icon::Search));
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
                .add(egui::Button::new(icon(Icon::X)).frame(false))
                .on_hover_text("Clear filter")
                .clicked()
        {
            filter.clear();
        }
    });
}

// ---- folders ----------------------------------------------------------------

fn queues_folder(
    ui: &mut egui::Ui,
    ns: NamespaceId,
    tree: &EntityTree,
    filter: &TreeFilter,
    actions: &mut Vec<AppAction>,
) {
    let title = format!("{} Queues{}", icon(Icon::Inbox), list_suffix(&tree.queues));
    let header = egui::CollapsingHeader::new(title)
        .id_salt(("queues-folder", ns))
        .show(ui, |ui| match &tree.queues {
            Loadable::NotLoaded => actions.push(AppAction::LoadQueues(ns)),
            Loadable::Loading => {
                ui.spinner();
            }
            Loadable::Failed(e) => failed_row(ui, e, AppAction::LoadQueues(ns), actions),
            Loadable::Loaded(queues) => {
                let visible: Vec<&QueueInfo> = queues
                    .iter()
                    .filter(|q| filter.matches(&q.properties.name))
                    .collect();
                if visible.is_empty() {
                    ui.label(egui::RichText::new("no queues").weak());
                }
                for queue in visible {
                    queue_row(ui, ns, queue, actions);
                }
            }
        });
    header.header_response.context_menu(|ui| {
        if ui.button("Create queue…").clicked() {
            actions.push(AppAction::OpenCreateDialog {
                ns,
                kind: CreateKind::Queue,
            });
            ui.close();
        }
        if ui.button("Refresh").clicked() {
            actions.push(AppAction::LoadQueues(ns));
            ui.close();
        }
    });
}

fn topics_folder(
    ui: &mut egui::Ui,
    ns: NamespaceId,
    tree: &EntityTree,
    filter: &TreeFilter,
    actions: &mut Vec<AppAction>,
) {
    let title = format!("{} Topics{}", icon(Icon::Radio), list_suffix(&tree.topics));
    let header = egui::CollapsingHeader::new(title)
        .id_salt(("topics-folder", ns))
        .show(ui, |ui| match &tree.topics {
            Loadable::NotLoaded => actions.push(AppAction::LoadTopics(ns)),
            Loadable::Loading => {
                ui.spinner();
            }
            Loadable::Failed(e) => failed_row(ui, e, AppAction::LoadTopics(ns), actions),
            Loadable::Loaded(topics) => {
                let visible: Vec<&TopicInfo> = topics
                    .iter()
                    .filter(|t| topic_visible(t, tree, filter))
                    .collect();
                if visible.is_empty() {
                    ui.label(egui::RichText::new("no topics").weak());
                }
                for topic in visible {
                    topic_node(ui, ns, topic, tree, filter, actions);
                }
            }
        });
    header.header_response.context_menu(|ui| {
        if ui.button("Create topic…").clicked() {
            actions.push(AppAction::OpenCreateDialog {
                ns,
                kind: CreateKind::Topic,
            });
            ui.close();
        }
        if ui.button("Refresh").clicked() {
            actions.push(AppAction::LoadTopics(ns));
            ui.close();
        }
    });
}

// ---- rows ---------------------------------------------------------------------

fn queue_row(ui: &mut egui::Ui, ns: NamespaceId, queue: &QueueInfo, actions: &mut Vec<AppAction>) {
    let path = EntityPath::Queue(queue.properties.name.clone());
    let label = entity_label(
        ui,
        &icon(Icon::Inbox),
        &queue.properties.name,
        Some(&queue.runtime.count_details),
    );
    let response = ui.selectable_label(false, label);
    if response.clicked() {
        actions.push(AppAction::OpenEntity(ScopedEntity::new(ns, path.clone())));
    }
    response.context_menu(|ui| {
        entity_context_menu(ui, ns, &path, AppAction::LoadQueues(ns), actions);
    });
}

fn topic_node(
    ui: &mut egui::Ui,
    ns: NamespaceId,
    topic: &TopicInfo,
    tree: &EntityTree,
    filter: &TreeFilter,
    actions: &mut Vec<AppAction>,
) {
    let name = &topic.properties.name;
    let path = EntityPath::Topic(name.clone());
    let subs = tree.subscriptions.get(name);
    let title = format!(
        "{} {name}{}",
        icon(Icon::Radio),
        subs.map_or_else(|| format!(" ({})", topic.subscription_count), list_suffix)
    );

    // With an active filter, expand so matching subscriptions are visible; a
    // topic that matches by name shows all its subscriptions.
    let topic_matches = filter.matches(name);
    let mut header = egui::CollapsingHeader::new(title).id_salt(("topic", ns, name));
    if filter.is_active() {
        header = header.open(Some(true));
    }
    let header = header.show(ui, |ui| match subs {
        None | Some(Loadable::NotLoaded) => {
            actions.push(load_subs(ns, name));
        }
        Some(Loadable::Loading) => {
            ui.spinner();
        }
        Some(Loadable::Failed(e)) => {
            failed_row(ui, e, load_subs(ns, name), actions);
        }
        Some(Loadable::Loaded(subscriptions)) => {
            let visible: Vec<&SubscriptionInfo> = subscriptions
                .iter()
                .filter(|s| topic_matches || filter.matches(&s.properties.name))
                .collect();
            if visible.is_empty() {
                ui.label(egui::RichText::new("no subscriptions").weak());
            }
            for subscription in visible {
                subscription_node(ui, ns, subscription, tree, actions);
            }
        }
    });
    header.header_response.context_menu(|ui| {
        if ui.button("Open").clicked() {
            actions.push(AppAction::OpenEntity(ScopedEntity::new(ns, path.clone())));
            ui.close();
        }
        if ui.button("Create subscription…").clicked() {
            actions.push(AppAction::OpenCreateDialog {
                ns,
                kind: CreateKind::Subscription {
                    topic: name.clone(),
                },
            });
            ui.close();
        }
        if ui.button("Refresh").clicked() {
            actions.push(load_subs(ns, name));
            ui.close();
        }
        ui.separator();
        if ui.button("Delete…").clicked() {
            actions.push(AppAction::RequestDelete(ScopedEntity::new(
                ns,
                path.clone(),
            )));
            ui.close();
        }
    });
}

fn subscription_node(
    ui: &mut egui::Ui,
    ns: NamespaceId,
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
        &icon(Icon::Mail),
        &name,
        Some(&subscription.runtime.count_details),
    );

    let load_rules = || AppAction::LoadRules {
        ns,
        topic: topic.clone(),
        subscription: name.clone(),
    };

    // Custom header so the disclosure triangle expands the rules while a
    // click on the name opens the subscription (matches queue behavior).
    let id = ui.make_persistent_id(("subscription", ns, &topic, &name));
    let state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let header = state.show_header(ui, |ui| {
        let response = ui
            .selectable_label(false, label)
            .on_hover_text("Click to open");
        if response.clicked() {
            actions.push(AppAction::OpenEntity(ScopedEntity::new(ns, path.clone())));
        }
        response.context_menu(|ui| {
            if ui.button("Open").clicked() {
                actions.push(AppAction::OpenEntity(ScopedEntity::new(ns, path.clone())));
                ui.close();
            }
            if ui.button("Add rule…").clicked() {
                actions.push(AppAction::OpenCreateDialog {
                    ns,
                    kind: CreateKind::Rule {
                        topic: topic.clone(),
                        subscription: name.clone(),
                    },
                });
                ui.close();
            }
            if ui.button("Refresh rules").clicked() {
                actions.push(load_rules());
                ui.close();
            }
            ui.separator();
            if ui.button("Delete…").clicked() {
                actions.push(AppAction::RequestDelete(ScopedEntity::new(
                    ns,
                    path.clone(),
                )));
                ui.close();
            }
        });
    });
    header.body(|ui| match rules {
        None | Some(Loadable::NotLoaded) => {
            actions.push(load_rules());
        }
        Some(Loadable::Loading) => {
            ui.spinner();
        }
        Some(Loadable::Failed(e)) => {
            failed_row(ui, e, load_rules(), actions);
        }
        Some(Loadable::Loaded(rules)) => {
            for rule in rules {
                rule_row(ui, ns, rule, actions);
            }
        }
    });
}

fn rule_row(ui: &mut egui::Ui, ns: NamespaceId, rule: &RuleInfo, actions: &mut Vec<AppAction>) {
    let path = EntityPath::Rule {
        topic: rule.properties.topic.clone(),
        subscription: rule.properties.subscription.clone(),
        name: rule.properties.name.clone(),
    };
    let label = format!("{} {}", icon(Icon::Filter), rule.properties.name);
    let response = ui
        .selectable_label(false, label)
        .on_hover_text(rule.properties.filter.summary());
    if response.clicked() {
        actions.push(AppAction::OpenEntity(ScopedEntity::new(ns, path.clone())));
    }
    response.context_menu(|ui| {
        if ui.button("Open").clicked() {
            actions.push(AppAction::OpenEntity(ScopedEntity::new(ns, path.clone())));
            ui.close();
        }
        ui.separator();
        if ui.button("Delete…").clicked() {
            actions.push(AppAction::RequestDelete(ScopedEntity::new(
                ns,
                path.clone(),
            )));
            ui.close();
        }
    });
}

// ---- shared helpers -------------------------------------------------------------

fn load_subs(ns: NamespaceId, topic: &str) -> AppAction {
    AppAction::LoadSubscriptions {
        ns,
        topic: topic.to_owned(),
    }
}

/// `name (active, dead-letter, scheduled)`, tinted when the DLQ is non-empty.
fn entity_label(
    ui: &egui::Ui,
    glyph: &str,
    name: &str,
    counts: Option<&MessageCountDetails>,
) -> egui::RichText {
    match counts {
        Some(c) => {
            let text = format!(
                "{glyph} {name} ({}, {}, {})",
                c.active, c.dead_letter, c.scheduled
            );
            if c.dead_letter > 0 {
                egui::RichText::new(text).color(ui.visuals().warn_fg_color)
            } else {
                egui::RichText::new(text)
            }
        }
        None => egui::RichText::new(format!("{glyph} {name}")),
    }
}

fn entity_context_menu(
    ui: &mut egui::Ui,
    ns: NamespaceId,
    path: &EntityPath,
    refresh: AppAction,
    actions: &mut Vec<AppAction>,
) {
    if ui.button("Open").clicked() {
        actions.push(AppAction::OpenEntity(ScopedEntity::new(ns, path.clone())));
        ui.close();
    }
    if ui.button("Refresh").clicked() {
        actions.push(refresh);
        ui.close();
    }
    ui.separator();
    if ui.button("Delete…").clicked() {
        actions.push(AppAction::RequestDelete(ScopedEntity::new(
            ns,
            path.clone(),
        )));
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
fn topic_visible(topic: &TopicInfo, tree: &EntityTree, filter: &TreeFilter) -> bool {
    if !filter.is_active() || filter.matches(&topic.properties.name) {
        return true;
    }
    match tree.subscriptions.get(&topic.properties.name) {
        Some(Loadable::Loaded(subs)) => subs.iter().any(|s| filter.matches(&s.properties.name)),
        _ => true,
    }
}
