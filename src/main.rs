use eframe::{egui, Frame};
use eframe::egui::Context;
use simplelog::*;

fn main() {
    TermLogger::init(
        LevelFilter::Info,
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Auto
    ).expect("Failed to initialize logger");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1080.0, 720.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Palm",
        options,
        Box::new(|cc| {
            Box::<PalmGui>::default()
        })
    ).unwrap()
}

#[derive(Default)]
struct PalmGui;

impl eframe::App for PalmGui {
    fn update(&mut self, ctx: &Context, frame: &mut Frame) {

    }
}
