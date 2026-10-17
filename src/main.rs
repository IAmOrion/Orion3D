#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod doc;
mod file;
mod geom;
mod icons;
mod inspector;
mod library;
mod measure;
mod meshio;
mod ops;
mod prefs;
mod render;
mod units;
mod viewport;

#[cfg(test)]
mod tests;

fn main() -> eframe::Result {
    env_logger::init();

    // Allow "Open with" / double-click: first argument is a design file.
    let open = std::env::args_os().nth(1).map(std::path::PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Orion3D")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([900.0, 560.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Orion3D",
        options,
        Box::new(move |cc| Ok(Box::new(app::OrionApp::new(cc, open)))),
    )
}
