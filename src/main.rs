use eframe::egui;
use koiflow::app::KoiFlowApp;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("KoiFlow")
            .with_inner_size([1240.0, 820.0])
            .with_min_inner_size([720.0, 520.0]),
        ..Default::default()
    };

    eframe::run_native(
        "KoiFlow",
        options,
        Box::new(|cc| Ok(Box::new(KoiFlowApp::new(cc)))),
    )
}

