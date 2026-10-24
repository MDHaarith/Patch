//! Per-device preferences (not synced).

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Index into the theme list (0 Matte Black, 1 Black Glass, 2 Light, 3 Light Glass).
    pub look: i32,
    #[serde(default)]
    pub theme_version: u8,
    pub glass_opacity: f32,
    /// Index into `pet::SKINS`.
    pub pet_skin: i32,
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
            theme_version: 1,
            glass_opacity: 0.85,
            pet_skin: 0,
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
        let mut prefs: Prefs = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if !(0..4).contains(&prefs.look) || (prefs.theme_version == 0 && prefs.look >= 3) {
            prefs.look = 0;
        }
        prefs.theme_version = 1;
        prefs.glass_opacity = glass_opacity(prefs.glass_opacity);
        prefs
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        patch_core::doc::write_atomic(path, serde_json::to_string_pretty(self).unwrap_or_default().as_bytes())
    }
}

/// Keep imported preferences and live slider values in the readable range.
pub fn glass_opacity(value: f32) -> f32 {
    if value.is_finite() { value.clamp(0.25, 0.95) } else { 0.85 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_migration_and_opacity() {
        let path = std::env::temp_dir().join(format!("patch-theme-prefs-{}.json", std::process::id()));
        for look in 0..=10 {
            std::fs::write(&path, format!(r#"{{"look":{look}}}"#)).unwrap();
            let prefs = Prefs::load(&path);
            assert_eq!(prefs.look, if look < 3 { look } else { 0 });
            assert_eq!(prefs.theme_version, 1);
            assert_eq!(prefs.glass_opacity, 0.85);
        }
        let prefs = Prefs { look: 3, glass_opacity: 0.4, ..Prefs::default() };
        prefs.save(&path).unwrap();
        assert_eq!(Prefs::load(&path).look, 3);
        assert_eq!(Prefs::load(&path).glass_opacity, 0.4);
        assert_eq!(glass_opacity(-1.0), 0.25);
        assert_eq!(glass_opacity(2.0), 0.95);
        assert_eq!(glass_opacity(f32::NAN), 0.85);
        std::fs::remove_file(path).unwrap();
    }
}
