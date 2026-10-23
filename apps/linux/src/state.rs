//! Desktop-only state that isn't a user preference: where the pet and desk sit,
//! the main window's size, which one-time hints were shown.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetPlace {
    /// Top-left corner in physical screen pixels (X11 only: Wayland doesn't
    /// let clients position their windows).
    pub x: i32,
    pub y: i32,
    /// Legacy panel/bubble flag, kept for existing state files. Fixed windows
    /// save this as false.
    pub expanded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MainSize {
    /// Logical pixels.
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopState {
    pub widget: Option<WidgetPlace>,
    pub desk: Option<WidgetPlace>,
    pub main: Option<MainSize>,
    /// "Patch keeps running in the background" was shown.
    pub background_hint: bool,
    /// "Wayland can't keep the widget on top" was shown.
    pub wayland_widget_hint: bool,
}

impl DesktopState {
    pub fn load(path: &Path) -> DesktopState {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        let json = serde_json::to_string_pretty(self).unwrap_or_default();
        if let Err(e) = patch_core::doc::write_atomic(path, json.as_bytes()) {
            eprintln!("patch: can't save {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_partial_files_load() {
        assert_eq!(DesktopState::load(Path::new("/nonexistent/desktop.json")), DesktopState::default());
        let s: DesktopState = serde_json::from_str(r#"{"background_hint":true}"#).unwrap();
        assert!(s.background_hint && s.widget.is_none() && s.desk.is_none());
        let s: DesktopState = serde_json::from_str(r#"{"widget":{"x":10,"y":20,"expanded":true}}"#).unwrap();
        assert!(s.widget.unwrap().expanded && s.desk.is_none());
    }

    #[test]
    fn pet_and_desk_positions_round_trip() {
        let s = DesktopState {
            widget: Some(WidgetPlace { x: 1200, y: 56, expanded: false }),
            desk: Some(WidgetPlace { x: 908, y: 140, expanded: false }),
            ..DesktopState::default()
        };
        assert_eq!(serde_json::from_str::<DesktopState>(&serde_json::to_string(&s).unwrap()).unwrap(), s);
    }
}
