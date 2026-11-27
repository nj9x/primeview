// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alerts;
mod app;
mod layout;
mod market;
mod scripts;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("PrimeView")
            .with_inner_size([1440.0, 880.0])
            .with_min_inner_size([820.0, 520.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!(
                    "../../packaging/icons/icon-256.png"
                ))
                .unwrap_or_default(),
            ),
        ..Default::default()
    };
    eframe::run_native(
        "PrimeView",
        options,
        Box::new(|cc| Ok(Box::new(app::PrimeViewApp::new(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    eframe::WebLogger::init(log::LevelFilter::Info).ok();

    wasm_bindgen_futures::spawn_local(async {
        let document = eframe::web_sys::window()
            .expect("no window")
            .document()
            .expect("no document");
        let canvas = document
            .get_element_by_id("primeview_canvas")
            .expect("missing #primeview_canvas")
            .dyn_into::<eframe::web_sys::HtmlCanvasElement>()
            .expect("#primeview_canvas is not a canvas");

        let result = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(app::PrimeViewApp::new(cc)))),
            )
            .await;

        if let Some(loading) = document.get_element_by_id("loading") {
            match result {
                Ok(()) => loading.remove(),
                Err(e) => {
                    loading.set_inner_html(
                        "<p>PrimeView failed to start. See the developer console for details.</p>",
                    );
                    log::error!("failed to start: {e:?}");
                }
            }
        }
    });
}
