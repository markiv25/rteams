//! App shell: owns the data adapter, polls it, lays out header / board / detail panel.

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;

use crate::board;
use crate::data::Data;
use crate::detail::{self, DetailAction};

/// How often the log is checked for new events. Cheap: it is one `read` past the last offset.
const POLL: Duration = Duration::from_millis(500);

pub struct App {
    data: Result<Data, String>,
    /// Last refresh error, shown in the header; the previous board stays on screen.
    error: Option<String>,
    selected: Option<String>,
    /// Title last sent to the window, so it is only updated when the team name changes.
    window_title: String,
}

pub fn window_title(team: &str) -> String {
    format!("rteams \u{2014} {team}")
}

impl App {
    pub fn new(home: PathBuf, team: String) -> Self {
        App {
            data: Data::open(home, &team),
            error: None,
            selected: None,
            window_title: window_title(&team),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(POLL);

        let data = match &mut self.data {
            Ok(d) => d,
            Err(e) => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("cannot open team: {e}"));
                });
                return;
            }
        };
        self.error = data.refresh().err();

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.selected = None;
        }

        let board_result = data.board();
        let title = data
            .state()
            .ok()
            .and_then(|s| s.team.as_ref().map(|t| t.name.clone()));

        if let Some(name) = &title {
            let wanted = window_title(name);
            if wanted != self.window_title {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(wanted.clone()));
                self.window_title = wanted;
            }
        }

        egui::Panel::top("header").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(title.as_deref().unwrap_or("(no team yet)"));
                if let Some(e) = self.error.as_ref().or(board_result.as_ref().err()) {
                    ui.colored_label(egui::Color32::LIGHT_RED, e);
                }
            });
        });

        if let Some(id) = self.selected.clone() {
            egui::Panel::right("detail")
                .default_size(360.0)
                .show(ui, |ui| {
                    if let Ok(state) = data.state() {
                        let action = egui::ScrollArea::vertical()
                            .show(ui, |ui| detail::show(ui, state, &id))
                            .inner;
                        match action {
                            DetailAction::None => {}
                            DetailAction::Close => self.selected = None,
                            DetailAction::OpenTask(t) => self.selected = Some(t),
                        }
                    }
                });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            if let Ok(b) = &board_result {
                board::show(ui, b, &mut self.selected);
            }
        });
    }
}
