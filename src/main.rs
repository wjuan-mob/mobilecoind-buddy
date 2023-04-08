use clap::Parser;
use mobilecoind_buddy::{App, Config, Worker};

fn main() -> eframe::Result<()> {
    // Log to stdout (if you run with `RUST_LOG=debug`).
    tracing_subscriber::fmt::init();
    
    let config = Config::parse();

    let worker = Worker::new(config.clone()).expect("initialization failed");

    let native_options = eframe::NativeOptions {
        
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "mobilecoind_buddy",
        native_options,
        Box::new(|cc| Box::new(App::new(cc, config, worker))),
    )
}
