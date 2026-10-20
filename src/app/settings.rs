//! The app's settings, kept as JSON in %APPDATA%\warden\settings.json.

use crate::engine::data::Tool;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Reads the game now; such as "F9" or "Ctrl+Shift+R".
    pub read_hotkey: String,
    /// Turns auto read on or off.
    pub auto_hotkey: String,
    /// A part of the game window's title.
    pub window_title: String,
    pub rollouts: usize,
    /// Held instrument keys.
    pub tools: Vec<String>,
    /// Days until Riot when `tools` were saved: a higher count read twice
    /// at start is a new run and clears them.
    pub tools_days: Option<u32>,
    /// The run's theme, an id of the game data's themes; None for a run without one.
    pub theme: Option<u32>,
    /// Read the game by itself when its screen changes and settles.
    pub auto_read: bool,
    /// Move the window over the empty area under the game's instrument panel
    /// when the game is first found.
    pub auto_place: bool,
    /// Keep a log of each run's reads, advice and corrections, with the
    /// frames read, in %APPDATA%\warden\runs (the newest 10 runs), see
    /// [`super::runlog`].
    pub run_log: bool,
    /// Ask GitHub for a newer release at start.
    pub check_updates: bool,
    /// Offer to install when running from outside the install folder.
    pub offer_install: bool,
    /// Send each finished run's log to warden.arcasti.dev, see
    /// [`super::share`]; None until the player is asked.
    pub share_logs: Option<bool>,
    /// When the player last turned sharing on, as a run folder name (UTC):
    /// only runs whose folder name sorts at or after it are sent.
    pub shared_from: String,
    /// A random lowercase UUID sent with each run log; empty until the
    /// player first chooses to share.
    pub install_id: String,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            read_hotkey: "F9".to_string(),
            auto_hotkey: "F8".to_string(),
            window_title: "Torchlight".to_string(),
            rollouts: 400,
            tools: Vec::new(),
            tools_days: None,
            theme: None,
            auto_read: true,
            auto_place: true,
            run_log: true,
            check_updates: true,
            offer_install: true,
            share_logs: None,
            shared_from: String::new(),
            install_id: String::new(),
        }
    }
}

/// %APPDATA%\warden, where the app keeps its files; None when APPDATA is not set.
pub fn app_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("warden"))
}

impl Settings {
    fn path() -> Option<PathBuf> {
        app_dir().map(|d| d.join("settings.json"))
    }

    /// The saved settings, or the defaults when there are none or they don't parse.
    pub fn load() -> Settings {
        Settings::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Settings::path().ok_or("APPDATA is not set")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The held instruments; unknown keys are dropped.
    pub fn held_tools(&self) -> Vec<Tool> {
        self.tools.iter().filter_map(|k| Tool::from_key(k)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn old_hotkey_field() {
        let s: Settings = serde_json::from_str(r#"{"hotkey": "F7", "rollouts": 300}"#).unwrap();
        assert_eq!((s.read_hotkey.as_str(), s.auto_hotkey.as_str(), s.rollouts, s.auto_place, s.tools_days, s.theme, s.run_log), ("F9", "F8", 300, true, None, None, true));
        assert!(s.check_updates && s.offer_install);
        assert_eq!((s.share_logs, s.shared_from.as_str(), s.install_id.as_str()), (None, "", ""));
    }
}
