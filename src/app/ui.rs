//! The window: reads on the read hotkey, the Read button, a saved screenshot
//! or by itself when the game's screen changes (auto read, turned on and off
//! by the Auto button or the auto read hotkey). It has four tabs. Board shows
//! the screen's ring of cells (click a cell to correct it) with the pick, the
//! move after it and the other good picks marked on their cells, and the pick
//! written in the courtyard inside the ring. Moves lists every move the
//! playouts rated. Run holds the held instruments and the run theme, and
//! Settings the keys and options. The window moves itself to the empty area
//! under the game's instrument panel once, when the game is first found.

use super::capture::{Event, Watcher, capture, game_rect};
use super::hotkey::{Hotkey, parse};
use super::session::{CellEdit, Session};
use crate::solver::advise::also_good;
use crate::solver::solve::FollowUp;
use super::runlog::RunLog;
use super::settings::Settings;
use crate::board::{CellModel, EventMark, Model};
use crate::engine::data::{Beast, RAINBOW, Tool, data};
use crate::engine::text::{cell_label, tier_name};
use crate::engine::{Action, ChoiceId};
use crate::reader::load_png;
use crate::reader::model::WarningKind;
use crate::reader::ocr;
use crate::reader::panels::Screen;
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Vec2};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Space between two board tiles.
const GAP: f32 = 4.0;
const WARDEN: Color32 = Color32::from_rgb(240, 200, 60);
const MARKER: Color32 = Color32::from_rgb(255, 140, 40);
const LEADER: Color32 = Color32::from_rgb(110, 200, 120);
const DARK_GREEN: Color32 = Color32::from_rgb(44, 74, 51);
const PALE_GREEN: Color32 = Color32::from_rgb(191, 232, 197);
/// The top left of the empty area under the game's Objective and instrument
/// panels, in a 1920x1080 client area.
const PLACE: (f32, f32) = (15.0, 470.0);

/// The window's tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Board,
    Moves,
    Run,
    Settings,
}

/// Which global hotkey: read now, or turn auto read on or off.
#[derive(Clone, Copy)]
enum Which {
    Read,
    Auto,
}

impl Which {
    fn other(self) -> Which {
        match self {
            Which::Read => Which::Auto,
            Which::Auto => Which::Read,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Which::Read => "read",
            Which::Auto => "auto read",
        }
    }

    /// The saved key.
    fn setting(self, settings: &mut Settings) -> &mut String {
        match self {
            Which::Read => &mut settings.read_hotkey,
            Which::Auto => &mut settings.auto_hotkey,
        }
    }
}

/// A registered hotkey, its field as typed (applied when it loses focus) and
/// the flag its presses set.
#[derive(Default)]
struct Key {
    hotkey: Option<Hotkey>,
    text: String,
    pressed: Arc<AtomicBool>,
}

pub struct WardenApp {
    session: Session,
    settings: Settings,
    ctx: egui::Context,
    /// The read and auto read hotkeys, indexed by [`Which`].
    keys: [Key; 2],
    /// The last capture, read, hotkey or settings error.
    error: Option<String>,
    /// The screenshot on show, when one was opened.
    path: String,
    /// The screen cell whose correction window is open.
    editing: Option<u8>,
    /// Auto read's capture thread, while auto read is on.
    watcher: Option<Watcher>,
    /// The window title the watcher looks for.
    watched: String,
    /// Why auto read can't see the game, shown quietly.
    auto_status: Option<String>,
    /// The window auto read's capture session is on, as the watcher describes it.
    watching: Option<String>,
    /// Why Windows OCR can't read the game (no English OCR language).
    ocr_problem: Option<String>,
    /// The window was moved to the game once, or placement is off; it is
    /// never moved again, so a window the player drags stays put.
    placed: bool,
    /// The tab on show.
    tab: Tab,
    /// The cell warnings of the last reading, so the Board tab opens when
    /// a reading brings the first.
    warned: usize,
}

impl WardenApp {
    pub fn new(cc: &eframe::CreationContext<'_>, settings: Settings, preload: Option<&Path>) -> WardenApp {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        let mut session = Session::new(settings.rollouts, settings.held_tools(), settings.theme, settings.tools_days);
        session.log = settings.run_log.then(RunLog::in_appdata).flatten();
        let ctx = cc.egui_ctx.clone();
        session.on_progress(move || ctx.request_repaint());
        let mut app = WardenApp {
            session,
            keys: [
                Key { text: settings.read_hotkey.clone(), ..Key::default() },
                Key { text: settings.auto_hotkey.clone(), ..Key::default() },
            ],
            settings,
            ctx: cc.egui_ctx.clone(),
            error: None,
            path: String::new(),
            editing: None,
            watcher: None,
            watched: String::new(),
            auto_status: None,
            watching: None,
            ocr_problem: ocr::problem(),
            placed: false,
            tab: Tab::Board,
            warned: 0,
        };
        app.watch();
        for which in [Which::Read, Which::Auto] {
            let text = which.setting(&mut app.settings).clone();
            match app.register(which, &text) {
                Ok(h) => app.keys[which as usize].hotkey = Some(h),
                Err(e) => app.error = Some(e),
            }
        }
        if let Some(p) = preload {
            app.read_png(p);
        }
        app
    }

    /// Registers `text` as the `which` hotkey, unless it is the other one's.
    fn register(&mut self, which: Which, text: &str) -> Result<Hotkey, String> {
        let other = which.other();
        if parse(text).is_ok() && parse(text) == parse(other.setting(&mut self.settings)) {
            return Err(format!("{text} is already the {} key.", other.name()));
        }
        let (flag, ctx) = (self.keys[which as usize].pressed.clone(), self.ctx.clone());
        Hotkey::register(text, move || {
            flag.store(true, Ordering::SeqCst);
            ctx.request_repaint();
        })
    }

    /// Reads the watcher's newest frame, else a fresh capture.
    fn read_now(&mut self) {
        match self.watcher.as_ref().and_then(Watcher::read_latest).map_or_else(|| capture(&self.settings.window_title), Ok) {
            Ok(frame) => {
                self.session.read_in_background(frame);
                self.path.clear();
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn read_png(&mut self, path: &Path) {
        self.path = path.display().to_string();
        match load_png(path) {
            Ok(frame) => {
                self.session.read_in_background(frame);
                if let Some(w) = &self.watcher {
                    w.adopt_next();
                }
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn set_auto_read(&mut self, on: bool) {
        self.settings.auto_read = on;
        self.watch();
        self.save();
    }

    /// Starts auto read's watcher when auto read is on and none watches the
    /// set window title; stops it when auto read is off.
    fn watch(&mut self) {
        if !self.settings.auto_read || self.watched != self.settings.window_title {
            // The old watcher stops before a new one opens a session.
            (self.watcher, self.auto_status, self.watching) = (None, None, None);
        }
        if self.settings.auto_read && self.watcher.is_none() {
            self.watched = self.settings.window_title.clone();
            let ctx = self.ctx.clone();
            self.watcher = Some(Watcher::start(&self.watched, move || ctx.request_repaint()));
        }
    }

    fn open(&mut self) {
        if let Some(path) = rfd::FileDialog::new().add_filter("Screenshot", &["png"]).pick_file() {
            self.read_png(&path);
        }
    }

    fn save(&mut self) {
        if let Err(e) = self.settings.save() {
            self.error = Some(format!("Settings not saved: {e}"));
        }
    }

    /// Registers the typed `which` hotkey in place of the current one; on
    /// failure the previous key stays.
    fn apply_hotkey(&mut self, which: Which) {
        let i = which as usize;
        let text = self.keys[i].text.trim().to_string();
        let saved = which.setting(&mut self.settings).clone();
        if text == saved && self.keys[i].hotkey.is_some() {
            return;
        }
        self.keys[i].hotkey = None;
        match self.register(which, &text) {
            Ok(h) => {
                self.keys[i].hotkey = Some(h);
                *which.setting(&mut self.settings) = text;
                self.error = None;
                self.save();
            }
            Err(e) => {
                self.error = Some(e);
                self.keys[i].hotkey = self.register(which, &saved).ok();
                self.keys[i].text = saved;
            }
        }
    }

    /// Moves the window to the top left of the empty area under the game's
    /// instrument panel the first time the game window is found.
    fn place(&mut self, ctx: &egui::Context) {
        if self.placed || !self.settings.auto_place {
            return;
        }
        let Ok([left, top, width, height]) = game_rect(&self.settings.window_title) else {
            ctx.request_repaint_after(Duration::from_secs(1));
            return;
        };
        let (x, y) = PLACE;
        let at = Pos2::new(left as f32 + x * width as f32 / 1920.0, top as f32 + y * height as f32 / 1080.0);
        let ppp = ctx.input(|i| i.viewport().native_pixels_per_point.unwrap_or(i.pixels_per_point));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(at / ppp));
        self.placed = true;
    }

    fn set_tools(&mut self, tools: Vec<Tool>) {
        self.settings.tools = tools.iter().map(|t| t.key().to_string()).collect();
        self.settings.tools_days = self.session.days_left();
        self.session.set_tools(tools);
        self.save();
    }

    fn set_theme(&mut self, theme: Option<u32>) {
        self.settings.theme = theme;
        self.settings.tools_days = self.session.days_left();
        self.session.set_theme(theme);
        self.save();
    }
}

impl eframe::App for WardenApp {
    /// The hotkeys, auto read and the worker results. eframe skips `ui` while
    /// the game covers the window but still calls this on a repaint request.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.place(ctx);
        if self.keys[Which::Read as usize].pressed.swap(false, Ordering::SeqCst) {
            self.read_now();
        }
        if self.keys[Which::Auto as usize].pressed.swap(false, Ordering::SeqCst) {
            self.set_auto_read(!self.settings.auto_read);
        }
        while let Some(event) = self.watcher.as_ref().and_then(Watcher::event) {
            match event {
                Event::Frame(frame) => {
                    self.session.read_in_background(frame);
                    self.path.clear();
                }
                Event::Status(status) => {
                    if status.is_some() {
                        self.watching = None;
                    }
                    self.auto_status = status;
                }
                Event::Window(window) => {
                    self.session.log_window(&window);
                    self.watching = Some(window);
                }
            }
        }
        self.session.poll();
        let warned = self.session.reading.as_ref().map_or(0, |r| r.warnings.iter().filter(|w| w.cell.is_some()).count() + r.kept_lines().len());
        if self.warned == 0 && warned > 0 {
            self.tab = Tab::Board;
        }
        self.warned = warned;
        // The count goes with the held instruments and the theme, so a
        // restart in a new run (a higher count) clears them.
        let held = !self.session.tools.is_empty() || self.session.theme.is_some();
        let recount = held && self.settings.tools_days != self.session.days_left();
        if self.session.take_tools_changed() || recount {
            self.settings.tools = self.session.tools.iter().map(|t| t.key().to_string()).collect();
            self.settings.theme = self.session.theme;
            self.settings.tools_days = self.session.days_left();
            self.save();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            self.tab_row(ui);
            if self.settings.auto_read
                && let Some(s) = &self.auto_status
            {
                ui.small(RichText::new(format!("Auto read waiting: {s}")).weak());
            }
            if let Some(e) = &self.error {
                ui.colored_label(Color32::LIGHT_RED, e);
            }
            egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                Tab::Board => self.board_tab(ui),
                Tab::Moves => self.moves_tab(ui),
                Tab::Run => self.instruments(ui),
                Tab::Settings => self.settings_section(ui),
            });
        });
        self.edit_window(&ui.ctx().clone());
    }
}

impl WardenApp {
    /// The tabs on the left; the Read and Auto buttons on the right, with a
    /// spinner while a read runs.
    fn tab_row(&mut self, ui: &mut egui::Ui) {
        let (mut read, mut auto) = (false, false);
        ui.horizontal(|ui| {
            if self.warned > 0 {
                let count = RichText::new(self.warned.to_string()).color(Color32::YELLOW);
                ui.selectable_value(&mut self.tab, Tab::Board, ("Board", count));
            } else {
                ui.selectable_value(&mut self.tab, Tab::Board, "Board");
            }
            ui.selectable_value(&mut self.tab, Tab::Moves, "Moves");
            ui.selectable_value(&mut self.tab, Tab::Run, "Run");
            if sliders_tab(ui, self.tab == Tab::Settings).on_hover_text("Settings").clicked() {
                self.tab = Tab::Settings;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                read = read_button(ui).on_hover_text(format!("Read now ({})", self.settings.read_hotkey)).clicked();
                let on = self.settings.auto_read;
                let hover = if on {
                    let mut hover = format!("Auto read is on: the game is read when its screen changes. Click or {} to turn it off", self.settings.auto_hotkey);
                    if let Some(w) = &self.watching {
                        hover.push_str(&format!("\nWatching {w}"));
                    }
                    hover
                } else {
                    format!("Auto read is off. Click or {} to turn it on", self.settings.auto_hotkey)
                };
                auto = auto_button(ui, on).on_hover_text(hover).clicked();
                if self.session.scanning() {
                    ui.spinner();
                }
            });
        });
        if read {
            self.read_now();
        }
        if auto {
            self.set_auto_read(!self.settings.auto_read);
        }
    }

    /// The ring with the pick in its courtyard and marks on the cells of the
    /// pick, the move after it and the other good picks. Over a kept reading,
    /// the lines saying why the frames since were not read, and "last
    /// reading" in the courtyard.
    fn board_tab(&mut self, ui: &mut egui::Ui) {
        let s = &self.session;
        if let Some(text) = &s.run_over {
            ui.label(RichText::new(text).strong().size(18.0));
            return;
        }
        let Some(reading) = &s.reading else {
            if !s.scanning() {
                ui.label("No board read yet.");
                if let Some(problem) = &self.ocr_problem {
                    ui.label(RichText::new(problem).small().color(Color32::YELLOW));
                }
                if let Some(missed) = &s.missed {
                    ui.label(RichText::new(missed).small().color(Color32::YELLOW));
                }
                if let Some(w) = &self.watching {
                    ui.label(RichText::new(format!("Auto read watches {w}")).small().weak());
                }
            }
            return;
        };
        let advice = s.advice();
        let warden = reading.model.warden;
        let then = match s.follow_up() {
            Some(FollowUp::Move(next)) => Some(next),
            _ => None,
        };
        let pick = advice.as_ref().and_then(|a| action_cell(&a.action, warden));
        let marks = Marks {
            pick,
            then: then.and_then(|n| action_cell(&n.action, warden)),
            near: advice
                .as_ref()
                .map(|a| also_good(&s.ranking, a.option))
                .unwrap_or_default()
                .into_iter()
                .filter_map(|k| action_cell(&s.ranking[k].action, warden))
                .filter(|&c| Some(c) != pick)
                .collect(),
        };
        let kept = reading.kept_lines();
        for line in &kept {
            ui.label(RichText::new(*line).small().color(Color32::YELLOW));
        }
        let warned: Vec<(usize, &str)> = reading.warnings.iter().filter_map(|w| Some((w.cell?, w.text.as_str()))).collect();
        let (clicked, court) = board(ui, &reading.model, &warned, &marks);
        let mut day = riot(reading.model.day, reading.days_left);
        if !kept.is_empty() {
            day.push_str(" \u{b7} last reading");
        }
        if let Some(goal) = &reading.model.objective
            && let Some(o) = data().objective(goal.id)
        {
            let name = o.name.trim_start_matches("Objective: ");
            if goal.progress >= o.target {
                day.push_str(&format!("\n{name} \u{b7} done"));
            } else {
                day.push_str(&format!("\n{name} {}/{}", goal.progress, o.target));
            }
        }
        let pick = advice.as_ref().map(|a| move_name(&a.action, short(&a.label)));
        let next = then.map(|n| move_name(&n.action, short(&n.label)));
        courtyard(ui, court, &day, pick.as_deref(), next.as_deref(), s.solving);
        if let Some(a) = &advice {
            ui.interact(court, egui::Id::unique("warden-courtyard"), Sense::hover()).on_hover_text(format!("{}\n{}", a.label, a.why));
        }
        if clicked.is_some() {
            self.editing = clicked;
        }
    }

    /// The problems and the warnings of no cell, then every rated move with
    /// its mean as a bar; the advised one with the move after it and why.
    fn moves_tab(&mut self, ui: &mut egui::Ui) {
        let s = &self.session;
        ui.horizontal(|ui| {
            if let Some(r) = &s.reading {
                ui.small(RichText::new(format!("{} \u{b7} {}", riot(r.model.day, r.days_left), screen_name(r.screen))).weak());
            }
            if s.solving {
                ui.spinner();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.small(RichText::new("Chromatics per run").weak());
            });
        });
        for p in &s.problems {
            ui.label(RichText::new(p).small().color(Color32::LIGHT_RED));
        }
        for w in s.reading.iter().flat_map(|r| &r.warnings).filter(|w| w.cell.is_none()) {
            let text = match w.kind {
                WarningKind::HeldToolsUnknown => {
                    format!("The {} panel shows icons the reader can't name: add them in the Run tab.", data().instrument_name)
                }
                WarningKind::Read => w.text.clone(),
            };
            ui.label(RichText::new(text).small().color(Color32::YELLOW));
        }
        if s.ranking.is_empty() {
            ui.label(if s.solving { "Solving..." } else { "No moves yet." });
            return;
        }
        let advice = s.advice();
        let advised = advice.as_ref().and_then(|a| a.option);
        let near = also_good(&s.ranking, advised);
        let (min, max) = s.ranking.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), o| (lo.min(o.mean), hi.max(o.mean)));
        let spacing = ui.spacing().item_spacing.x;
        for (k, o) in s.ranking.iter().enumerate() {
            let chosen = advised == Some(k);
            let (label, number) = (RichText::new(move_name(&o.action, &o.label)), RichText::new(format!("{:.2}", o.mean)));
            let (bar, label, number) = if chosen {
                (LEADER, label.strong().size(15.0).color(LEADER), number.color(LEADER))
            } else {
                let bar = if near.contains(&k) { Color32::from_rgb(95, 158, 105) } else { Color32::from_rgb(59, 90, 65) };
                (bar, label.color(Color32::from_gray(207)), number.weak())
            };
            ui.horizontal(|ui| {
                let h = ui.spacing().interact_size.y;
                ui.add_sized([14.0, h], egui::Label::new(RichText::new((k + 1).to_string()).weak()));
                let width = (ui.available_width() - 64.0 - 30.0 - 2.0 * spacing).max(20.0);
                ui.allocate_ui_with_layout(Vec2::new(width, h), Layout::left_to_right(Align::Center), |ui| {
                    ui.set_min_width(width);
                    ui.add(egui::Label::new(label).truncate());
                });
                let share = if max > min { ((o.mean - min) / (max - min)) as f32 } else { 1.0 };
                let (rect, _) = ui.allocate_exact_size(Vec2::new(64.0, 6.0), Sense::hover());
                let fill = Rect::from_min_size(rect.min, Vec2::new((64.0 * share).max(4.0), 6.0));
                ui.painter().rect_filled(fill, 3.0, bar);
                ui.allocate_ui_with_layout(Vec2::new(30.0, h), Layout::right_to_left(Align::Center), |ui| {
                    ui.label(number);
                });
            });
            if chosen && let Some(a) = &advice {
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.vertical(|ui| {
                        if let Some(FollowUp::Move(next)) = s.follow_up() {
                            ui.label(RichText::new(format!("then {}", move_name(&next.action, &next.label))).small().color(Color32::from_gray(221)));
                        }
                        ui.label(RichText::new(&a.why).small().weak());
                    });
                });
            }
        }
    }

    /// Held instruments, a list to add one, and the run theme.
    fn instruments(&mut self, ui: &mut egui::Ui) {
        let d = data();
        let held = self.session.tools.clone();
        let (mut remove, mut add) = (None, None);
        ui.small(RichText::new(format!("{}s", d.instrument_name)).weak());
        for (k, t) in held.iter().enumerate() {
            if ui.button(format!("{} \u{d7}", d.tool_names[*t as usize])).on_hover_text("Remove").clicked() {
                remove = Some(k);
            }
        }
        egui::ComboBox::from_id_salt("add-instrument").selected_text("Add...").show_ui(ui, |ui| {
            let mut offer: Vec<Tool> = Tool::ALL.iter().copied().filter(|t| !held.contains(t)).collect();
            offer.sort_by_key(|t| &d.tool_names[*t as usize]);
            for t in offer {
                if ui.selectable_label(false, &d.tool_names[t as usize]).clicked() {
                    add = Some(t);
                }
            }
        });
        if let Some(k) = remove {
            let mut tools = held;
            tools.remove(k);
            self.set_tools(tools);
        } else if let Some(t) = add {
            let mut tools = held;
            tools.push(t);
            self.set_tools(tools);
        }
        ui.separator();
        let theme = self.session.theme;
        let name = |id: Option<u32>| d.theme_name(id).unwrap_or("None");
        let mut picked = theme;
        ui.horizontal(|ui| {
            ui.small(RichText::new("Run theme").weak());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                egui::ComboBox::from_id_salt("run-theme").selected_text(name(theme)).show_ui(ui, |ui| {
                    for id in std::iter::once(None).chain(d.themes.iter().map(|t| Some(t.id))) {
                        ui.selectable_value(&mut picked, id, name(id));
                    }
                });
            });
        });
        ui.small(RichText::new("Both are read from the game; set them here if a read missed them.").weak());
        if picked != theme {
            self.set_theme(picked);
        }
    }

    fn settings_section(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("settings").num_columns(2).show(ui, |ui| {
            for (which, label) in [(Which::Read, "Read key"), (Which::Auto, "Auto read key")] {
                ui.label(label);
                let text = &mut self.keys[which as usize].text;
                let r = ui.add(egui::TextEdit::singleline(text).desired_width(120.0)).on_hover_text("Such as F8, Ctrl+Shift+R or Alt+5");
                if r.lost_focus() {
                    self.apply_hotkey(which);
                }
                ui.end_row();
            }
            ui.label("Game window");
            let r = ui.add(egui::TextEdit::singleline(&mut self.settings.window_title).desired_width(120.0)).on_hover_text("Used when no torchlight_infinite.exe window is found: a part of the game window's title");
            if r.lost_focus() {
                self.watch();
                self.save();
            }
            ui.end_row();
            ui.label("Playouts");
            let r = ui.add(egui::DragValue::new(&mut self.settings.rollouts).range(50..=5000).speed(10)).on_hover_text("Per option, for each solve");
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                self.session.rollouts = self.settings.rollouts;
                self.session.solve();
                self.save();
            }
            ui.end_row();
        });
        let mut place = self.settings.auto_place;
        let hover = "Move this window under the game's instrument panel when the game is first found";
        if ui.checkbox(&mut place, "Place by the game panels").on_hover_text(hover).changed() {
            self.settings.auto_place = place;
            self.save();
        }
        let mut open = false;
        ui.horizontal(|ui| {
            let mut log = self.settings.run_log;
            let hover = "Keep each run's reads, advice and corrections, with the frames read, in %APPDATA%\\warden\\runs (the newest 10 runs)";
            if ui.checkbox(&mut log, "Keep a run log").on_hover_text(hover).changed() {
                self.settings.run_log = log;
                self.session.log = log.then(RunLog::in_appdata).flatten();
                self.save();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                open = ui.button("Open screenshot...").clicked();
            });
        });
        if open {
            self.open();
        }
        if !self.path.is_empty() {
            ui.small(&self.path);
        }
    }

    /// The correction window of the cell clicked on the board.
    fn edit_window(&mut self, ctx: &egui::Context) {
        let Some(cell) = self.editing else { return };
        let d = data();
        let (mut open, mut edit, mut marker) = (true, None, None);
        let marked = self.session.reading.as_ref().and_then(|r| r.model.cells.get(usize::from(cell) - 1)).is_some_and(|c| c.event().is_some());
        let warnings: Vec<String> = self.session.reading.iter().flat_map(|r| &r.warnings).filter(|w| w.cell == Some(usize::from(cell) - 1)).map(|w| w.text.clone()).collect();
        egui::Window::new(format!("Cell {cell:02}"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, [0.0, 26.0])
            .show(ctx, |ui| {
                for text in &warnings {
                    ui.label(RichText::new(text).small().color(Color32::YELLOW));
                }
                ui.horizontal(|ui| {
                    if ui.small_button("Locked").clicked() {
                        edit = Some(CellEdit::Locked);
                    }
                    if ui.small_button("Empty").clicked() {
                        edit = Some(CellEdit::Empty);
                    }
                    let mut on = marked;
                    if ui.checkbox(&mut on, format!("{} marker", d.incident_term)).changed() {
                        marker = Some(on);
                    }
                });
                egui::Grid::new("edit-cell").spacing([2.0, 2.0]).show(ui, |ui| {
                    ui.label("");
                    for tier in 0..RAINBOW {
                        ui.label(RichText::new(&tier_name(tier)[..3]).size(10.0).color(tier_colour(tier)));
                    }
                    ui.end_row();
                    for beast in Beast::ALL {
                        ui.label(RichText::new(&d.beast_names[beast.index()]).small());
                        for tier in 0..RAINBOW {
                            let text = RichText::new(initial(beast)).strong().size(11.0).color(Color32::WHITE);
                            let button = egui::Button::new(text).fill(tier_colour(tier).gamma_multiply(0.45)).min_size(Vec2::new(24.0, 18.0));
                            let name = format!("{} {}", tier_name(tier), d.beast_names[beast.index()]);
                            if ui.add(button).on_hover_text(name).clicked() {
                                edit = Some(CellEdit::Inmate { beast, tier: i64::from(tier) });
                            }
                        }
                        ui.end_row();
                    }
                });
            });
        let done = match (edit, marker) {
            (Some(edit), _) => Some(self.session.edit_cell(cell, edit)),
            (None, Some(on)) => Some(self.session.set_marker(cell, on)),
            (None, None) => None,
        };
        if let Some(result) = done {
            if let Err(e) = result {
                self.error = Some(e);
            }
            self.editing = None;
        } else if !open {
            self.editing = None;
        }
    }
}

/// "Day 7 · 13 to Riot".
fn riot(day: i64, days_left: Option<u32>) -> String {
    match days_left {
        Some(n) => format!("Day {day} \u{b7} {n} to Riot"),
        None => format!("Day {day} \u{b7} Riot not read"),
    }
}

/// The screen the reading shows, as the Moves tab names it.
fn screen_name(screen: Screen) -> &'static str {
    match screen {
        Screen::Patrol => "patrol",
        Screen::Manage => "manage",
        Screen::ChainTarget => "Consume target pick",
        Screen::InstrumentPick => "instrument pick",
        Screen::TreasurePick => "treasure pick",
        Screen::PrisonStatus => "prison status",
        Screen::Interrogation => "Interrogation Room",
        Screen::CellPick => "Sewer Ratway cell pick",
        Screen::DayBanner => "day banner",
        Screen::Theme => "run theme",
        Screen::Popup => "popup",
        Screen::RunEnd => "run over",
        Screen::Unknown => "no decision on screen",
    }
}

/// A move's label without its bracketed details, which the board marks show.
fn short(label: &str) -> &str {
    label.split_once(" (").map_or(label, |(name, _)| name)
}

/// A move's label as the window shows it: a Consume names only its cell and
/// a swap is just "Replace".
fn move_name(action: &Action, label: &str) -> String {
    match action {
        Action::Devour { to } => format!("Consume {}", cell_label(*to)),
        Action::Swap => "Replace".to_string(),
        _ => label.to_string(),
    }
}

/// The model cell a move acts on: the cell patrolled, devoured or picked,
/// or the Warden's own for a swap.
fn action_cell(action: &Action, warden: i64) -> Option<usize> {
    match action {
        Action::Patrol { cell: n } | Action::Devour { to: n } | Action::Chain { to: n } | Action::Choose { id: ChoiceId::Cell(n) } => Some(*n),
        Action::Swap => usize::try_from(warden).ok(),
        _ => None,
    }
}

/// The Settings tab: a selectable of three slider lines, knobs at different
/// places.
fn sliders_tab(ui: &mut egui::Ui, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(24.0, 20.0), Sense::click());
    let visuals = ui.style().interact_selectable(&response, selected);
    let p = ui.painter();
    if selected || response.hovered() {
        p.rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
    }
    let colour = visuals.fg_stroke.color;
    let (left, right) = (rect.left() + 6.0, rect.right() - 6.0);
    for (dy, knob) in [(-5.0, 0.3), (0.0, 0.7), (5.0, 0.45)] {
        let y = rect.center().y + dy;
        p.line_segment([Pos2::new(left, y), Pos2::new(right, y)], Stroke::new(1.2, colour));
        p.circle_filled(Pos2::new(left + (right - left) * knob, y), 2.2, colour);
    }
    response
}

/// The Read button: a circular arrow.
fn read_button(ui: &mut egui::Ui) -> egui::Response {
    let response = ui.add_sized([22.0, 20.0], egui::Button::new(""));
    let colour = ui.style().interact(&response).fg_stroke.color;
    let (c, r) = (response.rect.center(), 5.5);
    let at = |a: f32| c + r * Vec2::angled(a);
    let (from, to) = (30f32.to_radians(), 330f32.to_radians());
    let arc: Vec<Pos2> = (0..=20).map(|k| at(from + (to - from) * k as f32 / 20.0)).collect();
    let p = ui.painter();
    p.add(Shape::line(arc, Stroke::new(1.5, colour)));
    let (end, along, out) = (at(to), Vec2::angled(to + std::f32::consts::FRAC_PI_2), Vec2::angled(to));
    p.add(Shape::convex_polygon(vec![end + along * 3.0, end + out * 3.0, end - out * 3.0], colour, Stroke::NONE));
    response
}

/// The Auto button: a dot and "Auto", green while auto read is on.
fn auto_button(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let dot = egui::IdSalt::new("auto-dot");
    let text = if on { RichText::new("Auto").color(PALE_GREEN) } else { RichText::new("Auto") };
    let mut button = egui::Button::new((egui::Atom::custom(dot, Vec2::splat(8.0)), text));
    if on {
        button = button.fill(DARK_GREEN);
    }
    let r = button.atom_ui(ui);
    if let Some(rect) = r.rect(dot) {
        ui.painter().circle_filled(rect.center(), 3.5, if on { LEADER } else { Color32::from_gray(85) });
    }
    r.response
}

/// Rarity colours as the cells glow: white, blue, purple, amber, red; Chromatic pink.
fn tier_colour(tier: i32) -> Color32 {
    match tier {
        0 => Color32::from_rgb(200, 200, 200),
        1 => Color32::from_rgb(80, 140, 255),
        2 => Color32::from_rgb(180, 100, 255),
        3 => Color32::from_rgb(255, 170, 40),
        4 => Color32::from_rgb(240, 70, 60),
        _ => Color32::from_rgb(255, 120, 220),
    }
}

fn initial(beast: Beast) -> &'static str {
    match beast {
        Beast::Mammoth => "M",
        Beast::Lion => "L",
        Beast::Wolf => "W",
        Beast::Rat => "R",
    }
}

/// Column and row of model cell `n` on the screen's 6x4 ring, clockwise from
/// the Interrogation Room at the top left.
fn ring(n: usize) -> (usize, usize) {
    match n {
        0..=5 => (n, 0),
        6 | 7 => (5, n - 5),
        8..=13 => (13 - n, 3),
        _ => (0, 16 - n),
    }
}

/// What a cell holds, for its tooltip.
fn describe(cell: &CellModel) -> String {
    let d = data();
    let beast = |b: &Beast| d.beast_names[b.index()].as_str();
    let tier = |t: &i64| tier_name(*t as i32);
    let marker = |e: &Option<EventMark>| if e.is_some() { format!(", {} marker", d.incident_term) } else { String::new() };
    match cell {
        CellModel::Office => "Interrogation Room".to_string(),
        CellModel::Sealed { event } => format!("Locked{}", marker(event)),
        CellModel::Empty { event } => format!("Empty{}", marker(event)),
        CellModel::Inmate { beast: b, tier: t, event, .. } => format!("{} {}{}", tier(t), beast(b), marker(event)),
        CellModel::Nest { due: Some(n) } => format!("Nest, due in {n} days"),
        CellModel::Nest { due: None } => "Nest, due tomorrow".to_string(),
        CellModel::Scouted { beast: b, tier: t, event } => format!("Scouted: {} {}{}", tier(t), beast(b), marker(event)),
        CellModel::Scratch { tier: t, event } => format!("Locked, {} behind the door{}", tier(t), marker(event)),
    }
}

/// The model cells the board marks: the pick's, the next move's and those of
/// the other good picks.
struct Marks {
    pick: Option<usize>,
    then: Option<usize>,
    near: Vec<usize>,
}

/// The board as the screen's ring of tiles, with the reading warnings of a
/// cell marked on it and in its tooltip and the cells of `marks` marked;
/// returns the screen cell clicked and the courtyard inside the ring.
fn board(ui: &mut egui::Ui, m: &Model, warned: &[(usize, &str)], marks: &Marks) -> (Option<u8>, Rect) {
    let w = ((ui.available_width() - 5.0 * GAP) / 6.0).clamp(36.0, 72.0);
    let tile = Vec2::new(w, w * 0.8);
    let (area, _) = ui.allocate_exact_size(Vec2::new(6.0 * w + 5.0 * GAP, 4.0 * tile.y + 3.0 * GAP), Sense::hover());
    let at = |col: usize, row: usize| Rect::from_min_size(area.min + Vec2::new(col as f32 * (w + GAP), row as f32 * (tile.y + GAP)), tile);
    let court = at(1, 1).union(at(4, 2));
    let mut clicked = None;
    let mut badges = Vec::new();
    for (n, cell) in m.cells.iter().enumerate().take(16) {
        let (col, row) = ring(n);
        let rect = at(col, row);
        let sense = if n == 0 { Sense::hover() } else { Sense::click() };
        let response = ui.interact(rect, egui::Id::unique(("warden-cell", n)), sense);
        let mut tip = format!("{} {}", cell_label(n), describe(cell));
        for (_, text) in warned.iter().filter(|(c, _)| *c == n) {
            tip = format!("{tip}\n{text}");
        }
        let response = response.on_hover_text(if n == 0 { tip } else { format!("{tip}\nClick to correct") });
        if response.clicked() {
            clicked = Some(n as u8 + 1);
        }
        let p = ui.painter();
        tile_shape(p, rect, cell, n, response.hovered());
        if marks.pick == Some(n) {
            p.rect_stroke(rect, 4.0, Stroke::new(2.0, LEADER), StrokeKind::Inside);
            badges.push((rect, "1", LEADER, Color32::from_rgb(13, 36, 16)));
        } else if marks.then == Some(n) {
            dashed_outline(p, rect.shrink(0.75), Stroke::new(1.5, LEADER));
            badges.push((rect, "2", DARK_GREEN, PALE_GREEN));
        }
        if marks.near.contains(&n) {
            p.circle_filled(rect.left_top() + Vec2::new(6.0, 6.0), 3.0, LEADER.gamma_multiply(0.8));
        }
        if warned.iter().any(|(c, _)| *c == n) {
            let c = rect.left_bottom() + Vec2::new(6.0, -6.0);
            p.circle_filled(c, 5.5, Color32::YELLOW);
            p.text(c, Align2::CENTER_CENTER, "?", FontId::proportional(9.0), Color32::BLACK);
        }
        if m.warden == n as i64 {
            p.rect_stroke(rect, 4.0, Stroke::new(2.5, WARDEN), StrokeKind::Outside);
        }
    }
    let p = ui.painter();
    for (rect, text, fill, colour) in badges {
        let c = rect.left_top() + Vec2::splat(7.0);
        p.circle_filled(c, 6.5, fill);
        p.text(c, Align2::CENTER_CENTER, text, FontId::proportional(10.0), colour);
    }
    (clicked, court)
}

/// The edge of `rect` in dashes.
fn dashed_outline(p: &egui::Painter, rect: Rect, stroke: Stroke) {
    let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
    p.extend(Shape::dashed_line(&corners, stroke, 4.0, 3.0));
}

/// The pick written in the ring's courtyard, centred: the day, the move with
/// a spinner while the playouts run, and the move after it.
fn courtyard(ui: &egui::Ui, court: Rect, day: &str, pick: Option<&str>, then: Option<&str>, solving: bool) {
    let p = ui.painter().with_clip_rect(court);
    let small = egui::TextStyle::Small.resolve(ui.style());
    let spin = 14.0;
    let wrap = court.width() - 8.0;
    let layout = |text: String, font: FontId, colour: Color32, width: f32| {
        let mut job = egui::text::LayoutJob::simple(text, font, colour, width);
        job.halign = Align::Center;
        p.layout_job(job)
    };
    let mut lines = vec![(layout(day.to_string(), small.clone(), ui.visuals().weak_text_color(), wrap), false)];
    match pick {
        Some(pick) => lines.push((layout(pick.to_string(), FontId::proportional(15.0), LEADER, wrap - spin), solving)),
        None if solving => lines.push((layout("Solving...".to_string(), small.clone(), ui.visuals().text_color(), wrap - spin), true)),
        None => {}
    }
    if let Some(then) = then {
        lines.push((layout(format!("then {then}"), small, Color32::from_gray(207), wrap), false));
    }
    let gap = 2.0;
    let height: f32 = lines.iter().map(|(g, _)| g.rect.height() + gap).sum::<f32>() - gap;
    let mut y = court.center().y - height / 2.0;
    for (galley, spinner) in lines {
        let (size, extra) = (galley.rect.size(), if spinner { spin + 4.0 } else { 0.0 });
        let left = court.center().x - (size.x + extra) / 2.0;
        let pos = Pos2::new(left, y) - galley.rect.min.to_vec2();
        if spinner {
            let r = Rect::from_min_size(Pos2::new(left + size.x + 4.0, y + (size.y - spin) / 2.0), Vec2::splat(spin));
            egui::Spinner::new().size(spin).paint_at(ui, r);
        }
        p.galley(pos, galley, Color32::WHITE);
        y += size.y + gap;
    }
}

fn tile_shape(p: &egui::Painter, rect: Rect, cell: &CellModel, n: usize, hovered: bool) {
    let big = FontId::proportional(rect.height() * 0.45);
    let (fill, edge) = match cell {
        CellModel::Inmate { tier, .. } | CellModel::Scouted { tier, .. } | CellModel::Scratch { tier, .. } => {
            let c = tier_colour(*tier as i32);
            (c.gamma_multiply(0.35), c)
        }
        CellModel::Office => (Color32::from_rgb(45, 50, 70), Color32::from_rgb(90, 100, 130)),
        CellModel::Empty { .. } => (Color32::from_gray(40), Color32::from_gray(70)),
        _ => (Color32::from_gray(26), Color32::from_gray(60)),
    };
    p.rect_filled(rect, 4.0, fill);
    let edge = if hovered { Stroke::new(2.0, Color32::WHITE) } else { Stroke::new(1.0, edge) };
    p.rect_stroke(rect, 4.0, edge, StrokeKind::Inside);
    let centre = rect.center();
    let text = |text: String, font: FontId, colour: Color32| {
        p.text(centre, Align2::CENTER_CENTER, text, font, colour);
    };
    match cell {
        CellModel::Inmate { beast, .. } => text(initial(*beast).to_string(), big, Color32::WHITE),
        CellModel::Scouted { beast, .. } => text(format!("{}?", initial(*beast)), big, Color32::from_gray(220)),
        CellModel::Scratch { .. } => text("?".to_string(), big, Color32::from_gray(220)),
        CellModel::Nest { due } => text(due.map_or("N".to_string(), |d| format!("N{d}")), big, Color32::from_gray(200)),
        CellModel::Office => text("IR".to_string(), FontId::proportional(rect.height() * 0.35), Color32::from_gray(200)),
        CellModel::Sealed { .. } => padlock(p, centre, rect.height() * 0.42),
        CellModel::Empty { .. } => {}
    }
    if cell.event().is_some() {
        p.text(rect.right_top() + Vec2::new(-5.0, 2.0), Align2::RIGHT_TOP, "!", FontId::proportional(rect.height() * 0.4), MARKER);
    }
    p.text(rect.right_bottom() - Vec2::new(3.0, 1.0), Align2::RIGHT_BOTTOM, cell_label(n), FontId::proportional(9.0), Color32::from_gray(150));
}

/// A padlock of height `h` centred on `c`.
fn padlock(p: &egui::Painter, c: Pos2, h: f32) {
    let colour = Color32::from_gray(150);
    let body = Rect::from_center_size(c + Vec2::new(0.0, h * 0.2), Vec2::new(h * 0.8, h * 0.55));
    let r = h * 0.25;
    let top = Pos2::new(c.x, body.top());
    let shackle: Vec<Pos2> = (0..=12)
        .map(|k| {
            let a = std::f32::consts::PI * k as f32 / 12.0;
            Pos2::new(top.x - r * a.cos(), top.y - r * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(shackle, Stroke::new(2.0, colour)));
    p.rect_filled(body, 2.0, colour);
}
