#![feature(mpmc_channel)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::f32;
use std::sync::Arc;

use eframe::egui::{
    Align, Button, CentralPanel, Color32, Context, Label, Layout, RichText, TextEdit,
};
use eframe::{egui, Frame};
use egui_tiles::{TileId, Tiles};
use simplelog::*;
use tokio::runtime::Runtime;

use crate::gui::{Pane, Tab, TreeBehavior};

pub mod backend;
pub mod gui;
pub mod hexedit;
pub mod util;

fn main() {
    TermLogger::init(
        LevelFilter::Info,
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    )
    .expect("Failed to initialize logger");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1080.0, 720.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Palm",
        options,
        Box::new(|cc| Ok(Box::<Palm>::new(Palm::new()))),
    )
    .unwrap();
}

struct Palm {
    rt: Arc<Runtime>,

    behavior: TreeBehavior,
    tree: egui_tiles::Tree<Pane>,
    next_tab_id: u32,
}

impl Palm {
    pub fn new() -> Self {
        let rt = Arc::new(Runtime::new().expect("Failed to create tokio runtime"));

        Self {
            rt: rt.clone(),

            behavior: TreeBehavior::default(),
            tree: egui_tiles::Tree::new_tabs("root", vec![Pane::Tab(Tab::new(1, rt.clone()))]),
            next_tab_id: 2,
        }
    }

    pub fn spawn_tab(&mut self, parent: TileId) {
        let tile_id = self
            .tree
            .tiles
            .insert_pane(Pane::Tab(Tab::new(self.next_tab_id, self.rt.clone())));

        if let Some(egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs))) =
            self.tree.tiles.get_mut(parent)
        {
            tabs.add_child(tile_id);
        }

        self.next_tab_id += 1;
    }
}

impl eframe::App for Palm {
    fn update(&mut self, ctx: &Context, frame: &mut Frame) {
        if let Some(tile_id) = self.behavior.spawn_tab_into.take() {
            self.spawn_tab(tile_id);
        }

        CentralPanel::default().show(ctx, |ui| {
            self.tree.ui(&mut self.behavior, ui);
        });
    }
}
