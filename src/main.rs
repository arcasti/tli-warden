//! The window beside the game, opened bare or on a saved screenshot. It is a
//! windows-subsystem program (no console window); the command line is the
//! console build, warden-cli.
//!
//!   warden [open <frame.png>]

#![windows_subsystem = "windows"]

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: warden [open <frame.png>]; the command line is warden-cli";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [] => gui(None),
        [cmd, frame] if cmd == "open" => gui(Some(Path::new(frame))),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            show_error(&message);
            ExitCode::from(1)
        }
    }
}

#[cfg(windows)]
fn gui(preload: Option<&Path>) -> Result<(), String> {
    warden::app::run(preload)
}

#[cfg(not(windows))]
fn gui(_: Option<&Path>) -> Result<(), String> {
    Err("the window runs on Windows only; use warden-cli".to_string())
}

/// There is no console to print to, so errors go to a message box.
#[cfg(windows)]
fn show_error(message: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    use windows::core::{HSTRING, w};
    // SAFETY: both strings outlive the call and no owner window is given.
    unsafe { MessageBoxW(None, &HSTRING::from(message), w!("Warden"), MB_OK | MB_ICONERROR) };
}

#[cfg(not(windows))]
fn show_error(message: &str) {
    eprintln!("{message}");
}
