//! A global hotkey: `RegisterHotKey` on a thread with its own message loop,
//! which calls back on every press until the `Hotkey` is dropped. With no
//! window the key belongs to that thread, so each `Hotkey` can use id 1.

use std::sync::mpsc::channel;
use std::thread::JoinHandle;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT};

/// Parses "F8", "Ctrl+Shift+R" or "Alt+5" into Win32 modifier bits and a
/// virtual key code: modifiers Ctrl, Alt, Shift, Win; keys F1-F24, A-Z, 0-9.
pub fn parse(text: &str) -> Result<(u32, u32), String> {
    let bad = || format!("unknown hotkey {text:?}: use a key such as F8, Ctrl+Shift+R or Alt+5");
    let parts: Vec<String> = text.split('+').map(|p| p.trim().to_uppercase()).collect();
    let (key, mods) = parts.split_last().ok_or_else(bad)?;
    let mut bits = 0;
    for m in mods {
        bits |= match m.as_str() {
            "CTRL" | "CONTROL" => MOD_CONTROL.0,
            "ALT" => MOD_ALT.0,
            "SHIFT" => MOD_SHIFT.0,
            "WIN" => MOD_WIN.0,
            _ => return Err(bad()),
        };
    }
    let vk = match key.as_bytes() {
        [c] if c.is_ascii_uppercase() || c.is_ascii_digit() => u32::from(*c),
        [b'F', rest @ ..] => match std::str::from_utf8(rest).ok().and_then(|n| n.parse::<u32>().ok()) {
            Some(n @ 1..=24) => 0x70 + n - 1,
            _ => return Err(bad()),
        },
        _ => return Err(bad()),
    };
    Ok((bits, vk))
}

pub struct Hotkey {
    thread: u32,
    join: Option<JoinHandle<()>>,
}

impl Hotkey {
    /// Registers `text` (see [`parse`]); `pressed` runs on the hotkey thread.
    pub fn register(text: &str, pressed: impl Fn() + Send + 'static) -> Result<Hotkey, String> {
        let (mods, vk) = parse(text)?;
        let (tx, rx) = channel();
        let name = text.to_string();
        let join = std::thread::spawn(move || {
            // SAFETY: Win32 calls on this thread's own message queue and hotkey id 1.
            unsafe {
                let mut msg = MSG::default();
                // Creates the thread's message queue before anyone posts to it.
                let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                if let Err(e) = RegisterHotKey(None, 1, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk) {
                    let _ = tx.send(Err(format!("Hotkey {name} is taken by another program ({e}).")));
                    return;
                }
                let _ = tx.send(Ok(GetCurrentThreadId()));
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    if msg.message == WM_HOTKEY {
                        pressed();
                    }
                }
                let _ = UnregisterHotKey(None, 1);
            }
        });
        let thread = rx.recv().map_err(|e| e.to_string())??;
        Ok(Hotkey { thread, join: Some(join) })
    }
}

impl Drop for Hotkey {
    fn drop(&mut self) {
        // SAFETY: posts WM_QUIT to the hotkey thread's queue, created before registration.
        if unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) }.is_ok()
            && let Some(join) = self.join.take()
        {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn hotkeys() {
        assert_eq!(parse("F8"), Ok((0, 0x77)));
        assert_eq!(parse("ctrl+shift+r"), Ok((2 | 4, u32::from(b'R'))));
        assert_eq!(parse("Alt + 5"), Ok((1, u32::from(b'5'))));
        assert!(parse("F25").is_err());
        assert!(parse("Hyper+F1").is_err());
        assert!(parse("").is_err());
    }

    /// Two keys held at once, as the app holds its read and auto read keys.
    #[cfg(windows)]
    #[test]
    fn two_at_once() {
        let a = super::Hotkey::register("Ctrl+Alt+Shift+F11", || {}).unwrap();
        let b = super::Hotkey::register("Ctrl+Alt+Shift+F12", || {}).unwrap();
        drop(a);
        drop(b);
    }
}
