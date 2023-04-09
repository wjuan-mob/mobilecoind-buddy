use clap::Parser;
use egui::Vec2;
use mobilecoind_buddy::{App, Config, Worker};

fn main() -> eframe::Result<()> {
    // Log to stdout (if you run with `RUST_LOG=debug`).
    tracing_subscriber::fmt::init();

    let config = Config::parse();

    let worker = Worker::new(config.clone()).expect("initialization failed");

    let native_options = eframe::NativeOptions {
        initial_window_size: Some(Vec2 { x: 600.0, y: 480.0 }),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "mobilecoind_buddy",
        native_options,
        Box::new(|cc| Box::new(App::new(cc, config, worker))),
    )
}
