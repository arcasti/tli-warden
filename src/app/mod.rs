//! The app window beside the game: a hotkey captures the game, the reader
//! reads the board and the solver ranks the moves.

pub mod runlog;
pub mod session;
pub mod settings;
#[cfg(windows)]
pub mod share;
#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod hotkey;
#[cfg(windows)]
pub mod http;
#[cfg(windows)]
pub mod install;
#[cfg(windows)]
pub mod ui;
#[cfg(windows)]
pub mod update;

/// Opens the window, with `preload` read first when given.
#[cfg(windows)]
pub fn run(preload: Option<&std::path::Path>) -> Result<(), String> {
    use eframe::egui;
    update::wait_for_previous();
    let current = std::env::current_exe().ok();
    let mut installed = false;
    if let Some(current) = &current {
        update::clean_old(current);
        installed = install::installed_here(current);
        if installed {
            let _ = install::register(current);
        }
    }
    let settings = settings::Settings::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Warden")
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon.png")).expect("assets/icon.png is a PNG"))
            .with_inner_size([296.0, 210.0])
            .with_always_on_top()
            .with_resizable(true),
        ..Default::default()
    };
    eframe::run_native("Warden", options, Box::new(move |cc| Ok(Box::new(ui::WardenApp::new(cc, settings, preload, current, installed)))))
        .map_err(|e| e.to_string())
}
