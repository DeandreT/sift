//! Bottom log panel fed by the tracing ring buffer.

use crate::logging::LogBuffer;

pub fn show(ui: &mut egui::Ui, log: &LogBuffer) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Log").strong());
        ui.label(egui::RichText::new(format!("({} entries)", log.len())).weak());
        if ui.small_button("Clear").clicked() {
            log.clear();
        }
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            log.for_each(|entry| {
                let color = level_color(ui, entry.level);
                let time = format!(
                    "{:02}:{:02}:{:02}",
                    entry.time.hour(),
                    entry.time.minute(),
                    entry.time.second()
                );
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.monospace(egui::RichText::new(time).weak());
                    ui.monospace(egui::RichText::new(format!("{:5}", entry.level)).color(color));
                    ui.monospace(&entry.message).on_hover_text(&entry.target);
                });
            });
        });
}

fn level_color(ui: &egui::Ui, level: tracing::Level) -> egui::Color32 {
    let visuals = ui.visuals();
    match level {
        tracing::Level::ERROR => visuals.error_fg_color,
        tracing::Level::WARN => visuals.warn_fg_color,
        tracing::Level::INFO => visuals.text_color(),
        _ => visuals.weak_text_color(),
    }
}
