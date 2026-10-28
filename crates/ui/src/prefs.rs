//! Per-device preferences (not synced).

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Environment {
    pub url: String,
    pub credential: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Index into the theme list (0 Matte Black, 1 Light).
    pub look: i32,
    #[serde(default)]
    pub theme_version: u8,
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
    pub environments: Vec<Environment>,
    pub sync_enabled: bool,
    pub host_credential: String,
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
            theme_version: 2,
            pet_skin: 0,
            week_monday: true,
            time_24h: true,
            view: 2,
            sidebar: true,
            hub_url: String::new(),
            hub_token: String::new(),
            hosting: false,
            environments: vec![],
            sync_enabled: true,
            host_credential: String::new(),
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
        if prefs.theme_version < 2 {
            prefs.look = match prefs.look {
                2 => 1,
                3 if prefs.theme_version == 1 => 1,
                _ => 0,
            };
        } else if !(0..2).contains(&prefs.look) {
            prefs.look = 0;
        }
        if prefs.environments.is_empty() && !prefs.hub_url.is_empty() {
            prefs.environments.push(Environment { url: prefs.hub_url.clone(), credential: prefs.hub_token.clone(), name: prefs.hub_url.clone() });
        }
        prefs.theme_version = 2;
        prefs
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        patch_core::doc::write_atomic(
            path,
            serde_json::to_string_pretty(self)
                .unwrap_or_default()
                .as_bytes(),
        )?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_hub_becomes_a_saved_environment_and_credentials_stay_local() {
        let path = std::env::temp_dir().join(format!("patch-environment-prefs-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"hub_url":"http://laptop:4747","hub_token":"old-code"}"#).unwrap();
        let mut prefs = Prefs::load(&path);
        assert_eq!(prefs.environments.len(), 1);
        assert!(prefs.sync_enabled);
        prefs.environments[0].credential = "device-private".into();
        prefs.hub_token = "device-private".into();
        prefs.sync_enabled = false;
        prefs.save(&path).unwrap();
        let loaded = Prefs::load(&path);
        assert_eq!(loaded.environments[0].credential, "device-private");
        assert_eq!(loaded.hub_token, "device-private");
        assert!(!loaded.sync_enabled);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn retired_theme_migration() {
        let path =
            std::env::temp_dir().join(format!("patch-theme-prefs-{}.json", std::process::id()));
        // Version 1: Matte Black, Black Glass, Light, Light Glass.
        for (old, expected) in [(0, 0), (1, 0), (2, 1), (3, 1), (9, 0)] {
            std::fs::write(&path, format!(r#"{{"look":{old},"theme_version":1}}"#)).unwrap();
            let prefs = Prefs::load(&path);
            assert_eq!(prefs.look, expected);
            prefs.save(&path).unwrap();
            assert_eq!(Prefs::load(&path).look, expected);
        }
        std::fs::remove_file(path).unwrap();
    }
}
