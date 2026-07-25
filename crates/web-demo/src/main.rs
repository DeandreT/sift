//! Browser entry point for the sift interactive demo.

mod app;

use app::DemoApp;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("sift interactive demo")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "sift interactive demo",
        options,
        Box::new(|cc| Ok(Box::new(DemoApp::new(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        let Some(element) = document.get_element_by_id("sift_canvas") else {
            return;
        };
        let Ok(canvas) = element.dyn_into::<web_sys::HtmlCanvasElement>() else {
            return;
        };

        let started = eframe::WebRunner::new()
            .start(
                canvas.clone(),
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(DemoApp::new(cc)))),
            )
            .await;

        if started.is_ok() {
            let _ = canvas.set_attribute("data-sift-ready", "true");
        } else if let Some(status) = document.get_element_by_id("loader_status") {
            status.set_text_content(Some("The sift demo could not start."));
        }
    });
}
