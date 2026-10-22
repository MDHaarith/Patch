//! Per-device preferences (not synced).

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Index into the theme list (0 Matte Black, 1 Liquid Glass, 2 Light, ...).
    pub look: i32,
    pub week_monday: bool,
    pub time_24h: bool,
    /// 0 day, 1 week, 2 month, 3 board, 4 list
    pub view: i32,
    pub sidebar: bool,
    pub hub_url: String,
    pub hub_token: String,
    pub hosting: bool,
    pub autostart: bool,
    pub widget: bool,
    /// The widget floats above other apps instead of sitting on the desktop.
    pub widget_float: bool,
    /// The main window stays above other apps.
    pub pinned: bool,
    /// User-defined repeats, e.g. "every:2:week".
    pub custom_repeats: Vec<String>,
    /// User-defined alert offsets in minutes before the start.
    pub custom_alerts: Vec<i64>,
    pub overlay: bool,
    pub overlay_boot: bool,
    pub overlay_expanded: bool,
    pub hidden_lists: Vec<String>,
    /// Reminders already shown, as "task-id@occurrence" keys.
    pub notified: Vec<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            look: 0,
            week_monday: true,
            time_24h: true,
            view: 2,
            sidebar: true,
            hub_url: String::new(),
            hub_token: String::new(),
            hosting: false,
            autostart: false,
            widget: false,
            widget_float: false,
            pinned: false,
            custom_repeats: Vec::new(),
            custom_alerts: Vec::new(),
            overlay: false,
            overlay_boot: false,
            overlay_expanded: true,
            hidden_lists: Vec::new(),
            notified: Vec::new(),
        }
    }
}

impl Prefs {
    pub fn load(path: &Path) -> Prefs {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        patch_core::doc::write_atomic(path, serde_json::to_string_pretty(self).unwrap_or_default().as_bytes())
    }
}
