//! sift — a cross-platform Azure Service Bus explorer.

// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod icons;
mod logging;
mod state;
mod ui;

fn main() -> eframe::Result {
    let log = logging::init();

    // Headless import: `sift --import-legacy <path>` migrates namespace
    // profiles from a legacy XML config into the config + OS secret store.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(pos) = args.iter().position(|a| a == "--import-legacy") {
        import_legacy_and_exit(args.get(pos + 1).map(std::path::PathBuf::from));
        return Ok(());
    }

    tracing::info!("sift {} starting", env!("CARGO_PKG_VERSION"));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("sift")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "sift",
        options,
        Box::new(move |cc| Ok(Box::new(app::SiftApp::new(cc, log)))),
    )
}

fn import_legacy_and_exit(path: Option<std::path::PathBuf>) {
    let Some(path) = path else {
        tracing::error!("usage: sift --import-legacy <path-to-config>");
        std::process::exit(1);
    };

    let mut config = match sift_core::config::AppConfig::load() {
        Ok(config) => config,
        Err(e) => {
            tracing::error!("failed to load sift config: {e}");
            std::process::exit(1);
        }
    };
    let secrets = sift_core::secrets::open_default_store();

    match sift_core::legacy_import::import_from_file(&path, &mut config, secrets.as_ref()) {
        Ok(report) => {
            for warning in &report.warnings {
                tracing::warn!("{warning}");
            }
            for (name, reason) in &report.skipped {
                tracing::warn!("skipped '{name}': {reason}");
            }
            if let Err(e) = config.save() {
                tracing::error!("import succeeded but saving the config failed: {e}");
                std::process::exit(1);
            }
            tracing::info!(
                "imported from {}: {report} (secrets stored in {})",
                path.display(),
                secrets.backend_name()
            );
        }
        Err(e) => {
            tracing::error!("import failed: {e}");
            std::process::exit(1);
        }
    }
}
