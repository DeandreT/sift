//! The dashboard: a sortable overview of every entity's message counts, with
//! auto-refresh and click-to-open. Data comes from the already-loaded tree
//! model; a refresh fans out list requests.

use egui_extras::{Column, TableBuilder};
use sift_backend::{EntityPath, NamespaceId};
use sift_mgmt::MessageCountDetails;

use crate::icons::{Icon, icon};
use crate::state::{AppAction, AutoRefresh, Connection, DashboardState, Loadable, ScopedEntity};

struct Row {
    ns: NamespaceId,
    kind: &'static str,
    path: EntityPath,
    display: String,
    counts: MessageCountDetails,
}

#[allow(clippy::too_many_lines)] // toolbar + table + totals row read best together
pub fn show(
    ui: &mut egui::Ui,
    connections: &[Connection],
    state: &mut DashboardState,
    actions: &mut Vec<AppAction>,
) {
    ui.horizontal(|ui| {
        if ui
            .button(format!("{} Refresh", icon(Icon::RefreshCw)))
            .clicked()
        {
            actions.push(AppAction::RefreshDashboard);
        }
        ui.separator();
        ui.label("Auto-refresh:");
        let mut selected = state.auto_refresh;
        egui::ComboBox::from_id_salt("dashboard-auto-refresh")
            .selected_text(selected.label())
            .show_ui(ui, |ui| {
                for option in AutoRefresh::ALL {
                    ui.selectable_value(&mut selected, option, option.label());
                }
            });
        if selected != state.auto_refresh {
            actions.push(AppAction::SetDashboardAutoRefresh(selected));
        }
        if state.auto_refresh != AutoRefresh::Off {
            ui.label(
                egui::RichText::new("auto-refresh adds load to the namespace")
                    .weak()
                    .small(),
            );
        }
    });
    ui.separator();

    let rows = collect_rows(connections);
    if rows.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "No counts yet. Refresh, or expand topics in the tree to load subscriptions.",
            )
            .weak(),
        );
        return;
    }

    let totals = rows
        .iter()
        .fold(MessageCountDetails::default(), |mut acc, r| {
            acc.active += r.counts.active;
            acc.dead_letter += r.counts.dead_letter;
            acc.scheduled += r.counts.scheduled;
            acc
        });

    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .column(Column::auto().at_least(90.0)) // type
        .column(Column::remainder().at_least(180.0)) // name
        .column(Column::auto().at_least(70.0)) // active
        .column(Column::auto().at_least(80.0)) // dead-letter
        .column(Column::auto().at_least(80.0)) // scheduled
        .column(Column::auto().at_least(70.0)) // total
        .header(20.0, |mut header| {
            for title in [
                "Type",
                "Name",
                "Active",
                "Dead-letter",
                "Scheduled",
                "Total",
            ] {
                header.col(|ui| {
                    ui.label(egui::RichText::new(title).strong());
                });
            }
        })
        .body(|body| {
            body.rows(20.0, rows.len(), |mut row| {
                let r = &rows[row.index()];
                let warn = r.counts.dead_letter > 0;
                row.col(|ui| {
                    ui.label(egui::RichText::new(r.kind).weak());
                });
                row.col(|ui| {
                    ui.label(&r.display);
                });
                num(&mut row, r.counts.active, false);
                num(&mut row, r.counts.dead_letter, warn);
                num(&mut row, r.counts.scheduled, false);
                let total = r.counts.active + r.counts.dead_letter + r.counts.scheduled;
                num(&mut row, total, false);

                if row.response().double_clicked() {
                    actions.push(AppAction::OpenEntity(ScopedEntity::new(
                        r.ns,
                        r.path.clone(),
                    )));
                }
                row.response().on_hover_text("Double-click to open");
            });
        });

    ui.separator();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{} entities", rows.len())).weak());
        ui.separator();
        ui.label(
            egui::RichText::new(format!(
                "totals — active {}, dead-letter {}, scheduled {}",
                totals.active, totals.dead_letter, totals.scheduled
            ))
            .strong(),
        );
    });
}

fn collect_rows(connections: &[Connection]) -> Vec<Row> {
    let mut rows = Vec::new();
    // Prefix names with the connection when more than one is open, so
    // same-named entities across namespaces stay distinguishable.
    let multi = connections.len() > 1;
    for conn in connections {
        let prefix = |name: &str| {
            if multi {
                format!("{} · {name}", conn.name)
            } else {
                name.to_owned()
            }
        };
        let tree = &conn.tree;
        if let Loadable::Loaded(queues) = &tree.queues {
            for q in queues {
                rows.push(Row {
                    ns: conn.profile_id,
                    kind: "queue",
                    path: EntityPath::Queue(q.properties.name.clone()),
                    display: prefix(&q.properties.name),
                    counts: q.runtime.count_details,
                });
            }
        }
        for subs in tree.subscriptions.values() {
            if let Loadable::Loaded(subs) = subs {
                for s in subs {
                    rows.push(Row {
                        ns: conn.profile_id,
                        kind: "subscription",
                        path: EntityPath::Subscription {
                            topic: s.properties.topic.clone(),
                            name: s.properties.name.clone(),
                        },
                        display: prefix(&format!("{}/{}", s.properties.topic, s.properties.name)),
                        counts: s.runtime.count_details,
                    });
                }
            }
        }
    }
    // Sort by type then name — stable, predictable ordering.
    rows.sort_by(|a, b| a.kind.cmp(b.kind).then_with(|| a.display.cmp(&b.display)));
    rows
}

fn num(row: &mut egui_extras::TableRow<'_, '_>, value: i64, warn: bool) {
    row.col(|ui| {
        let mut text = egui::RichText::new(value.to_string()).monospace();
        if warn {
            text = text.color(ui.visuals().warn_fg_color);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(text);
        });
    });
}
