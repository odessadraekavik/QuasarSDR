#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1380.0, 880.0])
            .with_min_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "QuasarSDR",
        options,
        Box::new(|cc| Ok(Box::new(quasar_sdr::ui::QuasarApp::new(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    // Seul le noyau --lib --no-default-features est supporté sur WASM à ce stade.
}
