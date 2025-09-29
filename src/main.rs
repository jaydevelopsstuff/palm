#![feature(mpmc_channel)]
#![feature(iter_array_chunks)]

use std::f32;
use std::sync::Arc;

use eframe::egui::{Align, Button, Color32, Context, Label, Layout, RichText, TextEdit};
use eframe::{egui, Frame};
use simplelog::*;
use tokio::runtime::Runtime;

use crate::backend::{LogData, Mode, NetState};
use crate::gui::Tab;
use crate::hexedit::HexEditor;
use crate::util::hex_encode_formatted;

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

    tabs: Vec<Tab>,
    focused_tab: u32,
    next_tab_id: u32,
}

impl Palm {
    pub fn new() -> Self {
        let rt = Arc::new(Runtime::new().expect("Failed to create tokio runtime"));

        Self {
            rt: rt.clone(),

            tabs: vec![Tab::new(0, rt)],
            focused_tab: 0,
            next_tab_id: 1,
        }
    }

    pub fn spawn_tab(&mut self) {
        self.tabs.push(Tab::new(self.next_tab_id, self.rt.clone()));
        self.next_tab_id += 1;
    }

    pub fn close_tab(&mut self, tab_id: u32) {
        if let Some(mut index) = self.tabs.iter().position(|t| t.id == tab_id) {
            self.tabs.remove(index);

            if index >= self.tabs.len() {
                index = self.tabs.len() - 1;
            }

            if self.focused_tab == tab_id {
                self.focus_tab(self.tabs[index].id);
            }
        }
    }

    pub fn focus_tab(&mut self, tab_id: u32) {
        self.focused_tab = tab_id;
    }

    pub fn focused_tab(&self) -> &Tab {
        self.tabs
            .iter()
            .find(|t| t.id == self.focused_tab)
            .expect("Could not find focused tab")
    }

    pub fn focused_tab_mut(&mut self) -> &mut Tab {
        self.tabs
            .iter_mut()
            .find(|t| t.id == self.focused_tab)
            .expect("Could not find focused tab")
    }
}

impl eframe::App for Palm {
    fn update(&mut self, ctx: &Context, frame: &mut Frame) {
        let net_state = self.focused_tab().net_state();

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let mut tab_clicked: Option<u32> = None;
                let mut close_tab_clicked: Option<u32> = None;
                for tab in &self.tabs {
                    if ui
                        .add(
                            Button::new(format!("{} Tab {}", tab.mode(), tab.id))
                                .selected(tab.id == self.focused_tab),
                        )
                        .clicked()
                    {
                        tab_clicked = Some(tab.id);
                    }
                    ui.add_space(-7.);
                    if ui
                        .add_enabled(
                            self.tabs.len() > 1 && tab.net_state() == NetState::Inactive,
                            Button::new("X"),
                        )
                        .clicked()
                    {
                        close_tab_clicked = Some(tab.id);
                    }
                }
                if let Some(tab_id) = tab_clicked {
                    self.focus_tab(tab_id);
                }
                if let Some(tab_id) = close_tab_clicked {
                    self.close_tab(tab_id);
                }
                if ui.button("+").clicked() {
                    self.spawn_tab();
                }
            })
        });
        egui::TopBottomPanel::top("mode_selector").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        net_state == NetState::Inactive,
                        egui::Button::new("Client")
                            .selected(self.focused_tab().mode() == Mode::Client),
                    )
                    .clicked()
                {
                    self.focused_tab_mut().set_mode(Mode::Client);
                }
                if ui
                    .add_enabled(
                        net_state == NetState::Inactive,
                        egui::Button::new("Server")
                            .selected(self.focused_tab().mode() == Mode::Server),
                    )
                    .clicked()
                {
                    self.focused_tab_mut().set_mode(Mode::Server);
                }
                ui.separator();
                if self.focused_tab().mode() == Mode::Client {
                    ui.add(
                        TextEdit::singleline(&mut self.focused_tab_mut().client_mut().address)
                            .desired_width(172.0)
                            .hint_text("127.0.0.1:54321")
                            .interactive(net_state == NetState::Inactive),
                    );
                    match net_state {
                        NetState::Inactive => {
                            if ui.button("Connect").clicked() {
                                self.focused_tab_mut().start_client();
                            }
                        }
                        NetState::Active => {
                            if ui.button("Disconnect").clicked() {
                                self.focused_tab().client().backend().shutdown();
                            }
                        }
                        NetState::Establishing => {
                            ui.add_enabled(false, Button::new("Connecting"));
                        }
                    };
                } else if self.focused_tab().mode() == Mode::Server {
                    ui.add(
                        TextEdit::singleline(&mut self.focused_tab_mut().server_mut().port)
                            .desired_width(72.)
                            .hint_text("54321")
                            .interactive(net_state == NetState::Inactive),
                    );
                    match net_state {
                        NetState::Inactive => {
                            if ui.button("Start").clicked() {
                                self.focused_tab_mut().start_server();
                            }
                        }
                        NetState::Active => {
                            if ui.button("Stop").clicked() {
                                self.focused_tab().server().backend().shutdown();
                            }
                        }
                        NetState::Establishing => {
                            ui.add_enabled(false, Button::new("Starting"));
                        }
                    };
                    if !self.focused_tab().server().is_server_log_focused() {
                        if ui.button("End Focused Connection").clicked() {
                            self.focused_tab()
                                .server()
                                .focused_connection_ui()
                                .unwrap()
                                .with_backend(self.focused_tab().server(), |c| c.shutdown())
                        }
                    }
                }
            });
        });
        if self.focused_tab().mode() == Mode::Server {
            egui::TopBottomPanel::top("server_tabs").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            Button::new("Server Log")
                                .selected(self.focused_tab().server().is_server_log_focused()),
                        )
                        .clicked()
                    {
                        self.focused_tab_mut()
                            .server_mut()
                            .set_focused_connection(None);
                    }
                    let mut clicked_conn_addr = None;
                    let mut close_conn_tab_addr = None;
                    for conn in self.focused_tab().server().connection_uis() {
                        if ui
                            .add(Button::new(conn.address()).selected(
                                Some(conn.address())
                                    == self.focused_tab().server().focused_connection(),
                            ))
                            .clicked()
                        {
                            clicked_conn_addr = Some(conn.address().to_string());
                        }
                        ui.add_space(-7.);
                        if ui
                            .add_enabled(
                                conn.net_state(self.focused_tab().server()) == NetState::Inactive,
                                Button::new("X"),
                            )
                            .clicked()
                        {
                            close_conn_tab_addr = Some(conn.address().to_string());
                        }
                    }
                    if let Some(clicked_conn_addr) = clicked_conn_addr {
                        self.focused_tab_mut()
                            .server_mut()
                            .set_focused_connection(Some(clicked_conn_addr));
                    }
                    if let Some(addr) = close_conn_tab_addr {
                        self.focused_tab_mut()
                            .server_mut()
                            .close_connection_ui(&addr);
                    }
                });
            });
        }
        // egui::SidePanel::right("text_panel").show(ctx, |ui| {});
        egui::TopBottomPanel::bottom("input_panel").show(ctx, |ui| {
            ui.with_layout(Layout::left_to_right(Align::BOTTOM), |ui| {
                let mut empty_draft_data = Vec::new();
                let draft_data = self.focused_tab_mut().draft_data_mut();
                let draft_data_len = draft_data.as_ref().and_then(|d| Some(d.len()));

                ui.add_enabled(
                    draft_data.is_some(),
                    HexEditor::new(draft_data.unwrap_or(&mut empty_draft_data))
                        .desired_width(ui.available_width() - 64.),
                );
                if ui
                    .add_enabled(
                        net_state == NetState::Active
                            && draft_data_len != None
                            && draft_data_len != Some(0),
                        Button::new("Send"),
                    )
                    .clicked()
                {
                    self.focused_tab_mut().send_data().unwrap();
                }
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let server_log_focused = matches!(
                    self.focused_tab()
                        .server_safe()
                        .and_then(|s| Some(s.is_server_log_focused())),
                    Some(true)
                );

                for log in self.focused_tab_mut().update_and_read_logs() {
                    ui.horizontal(|ui| {
                        ui.monospace(log.timestamp.format("%H:%M:%S").to_string());
                        match &log.data {
                            LogData::ClientConnect(addr) => {
                                ui.monospace(if server_log_focused {
                                    format!("{} Connected", addr)
                                } else {
                                    "Connected".into()
                                });
                            }
                            LogData::ClientDisconnect(addr) => {
                                ui.monospace(if server_log_focused {
                                    format!("{} Disconnected", addr)
                                } else {
                                    "Disconnected".into()
                                });
                            }
                            LogData::SentPacket(packet) => {
                                ui.add_sized((108., 20.), Label::new("You"));
                                let mut hex_formatted = hex_encode_formatted(&packet.data);
                                ui.add(
                                    TextEdit::multiline(&mut hex_formatted)
                                        .code_editor()
                                        .desired_width(f32::INFINITY),
                                );
                            }
                            LogData::ServerStarted => {
                                ui.monospace("Server Started");
                            }
                            LogData::ServerStopped => {
                                ui.monospace("Server Stopped");
                            }
                            LogData::ReceivedPacket(packet) => {
                                ui.add_sized((108., 20.), Label::new(&packet.address));
                                let mut hex_formatted = hex_encode_formatted(&packet.data);
                                ui.add(
                                    TextEdit::multiline(&mut hex_formatted)
                                        .code_editor()
                                        .desired_width(f32::INFINITY),
                                );
                            }
                            LogData::ConnectTimedOut => {
                                ui.monospace("Failed to Connect: Timed Out");
                            }
                            LogData::ConnectError(error) => {
                                ui.monospace(format!("Failed to Connect: {}", error));
                            }
                            LogData::FatalReadError(error) => {
                                ui.monospace(format!("Fatal Read Error: {error}"));
                            }
                        };
                    });
                }
            })
        });
    }
}
