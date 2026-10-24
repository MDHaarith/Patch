//! Patch's user interface, shared by the native Linux desktop app and the
//! native Android app.
//!
//! The platform shells only provide windows and a [`Host`] implementation;
//! everything else (state, view models, sync) lives in [`App`].
//!
//! ```ignore
//! let app = App::new(data_dir, Rc::new(MyHost));
//! let main = MainWindow::new()?;
//! app.attach_main(&main);
//! ```

mod app;
mod models;
pub mod prefs;
mod themes;
pub mod pet;

slint::include_modules!();

pub use app::App;

/// Platform services the shared UI needs from the shell it runs in.
/// Every method has a sensible no-op default.
pub trait Host {
    /// Phone layout (Android) instead of the desktop layout.
    fn is_mobile(&self) -> bool {
        false
    }

    /// Whether this device can run the sync hub itself (the Linux app can).
    fn can_host_hub(&self) -> bool {
        false
    }

    /// Start (`true`) or stop the embedded sync hub. On success when starting,
    /// returns how other devices can reach it.
    fn set_hosting(&self, _on: bool) -> Result<Option<HostedHub>, String> {
        Err("This device can't host the hub".into())
    }

    /// Launch Patch at login (desktop).
    fn set_autostart(&self, _on: bool) -> Result<(), String> {
        Ok(())
    }

    /// Show or hide the always-on-top widget window (desktop).
    fn set_widget(&self, _on: bool) {}

    /// Show or hide the floating "display over other apps" panel (Android).
    /// Returns false when the permission is missing; the host should then
    /// send the user to the permission screen.
    fn set_overlay(&self, _on: bool) -> bool {
        false
    }

    /// Whether the overlay permission is currently granted (Android).
    fn overlay_allowed(&self) -> bool {
        true
    }

    /// Restore the overlay after reboot (Android).
    fn set_overlay_boot(&self, _on: bool) {}

    /// The overlay switched between bubble (`false`) and panel (`true`);
    /// resize the native overlay/widget window accordingly.
    fn overlay_expand(&self, _expanded: bool) {}

    /// Bring the main window/activity to front, optionally opening a task.
    fn open_app(&self, _task_id: Option<&str>) {}

    /// Show a system notification (reminders).
    fn notify(&self, _title: &str, _body: &str) {}

    fn notify_reminder(&self, _id: &str, title: &str, body: &str) {
        self.notify(title, body);
    }

    /// Hand the upcoming reminders to the OS so they fire even when the app
    /// isn't running. Each call replaces the previous set. Return true when
    /// the host delivers them itself, so the app's own timer stays quiet.
    fn schedule_reminders(&self, _reminders: &[Reminder]) -> bool {
        false
    }

    fn copy_text(&self, _text: &str) {}

    /// A text field gained or lost focus (Android raises the keyboard).
    fn text_focus(&self, _focused: bool) {}

    /// Whether the shell draws no system title bar, so the UI shows its own
    /// window controls and rounded frame (Linux desktop).
    fn custom_chrome(&self) -> bool {
        false
    }

    /// A press in the custom title bar or frame (move, resize, buttons).
    fn window_action(&self, _action: WindowAction) {}

    /// Keep the main window above every other app.
    fn set_pinned(&self, _on: bool) {}

    /// The widget floats above other apps (`true`) or sits on the desktop
    /// layer like a home-screen widget (`false`).
    fn set_widget_float(&self, _float: bool) {}

    /// Latest state for a native home-screen widget (Android). JSON:
    /// `{"date":"Thursday, 22 October","open":3,"mood":"busy",
    ///   "today":[{"id","title","when","color":"#RRGGBB","done","overdue"}],
    ///   "upcoming":[...same...]}`. Only called when it changed.
    fn publish_snapshot(&self, _json: &str) {}

    /// Whether [`Host::scan_qr`] works here (Android camera scanner).
    fn can_scan_qr(&self) -> bool {
        false
    }

    /// Scan a hub's pairing QR code; the host hands the text to
    /// [`App::pair_from_link`] when it gets one.
    fn scan_qr(&self) {}

    /// Whether the host can download and install app updates.
    fn can_update(&self) -> bool {
        false
    }

    fn check_update(&self) {}

    /// Open Patch's system app details, where Android exposes battery usage.
    fn open_battery_settings(&self) {}

    /// `Some(true)` when the OS exempts Patch from battery optimization,
    /// `None` where that doesn't apply (desktop).
    fn battery_unrestricted(&self) -> Option<bool> {
        None
    }

    /// Ask the OS to stop restricting Patch in the background (Android).
    fn request_battery_unrestricted(&self) {}
}

/// Window-management requests from the custom chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    /// Start moving the window with the pointer (press in the title bar).
    Drag,
    /// Start resizing from an edge: 0 N, 1 NE, 2 E, 3 SE, 4 S, 5 SW, 6 W, 7 NW.
    Resize(u8),
    Close,
    Minimize,
    ToggleMaximize,
}

/// A reminder due at `at_ms` (Unix epoch milliseconds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reminder {
    /// Stable per task occurrence, e.g. `t-abc@2026-10-22T10:00`.
    pub key: String,
    pub at_ms: i64,
    pub title: String,
    pub body: String,
}

/// How other tailnet devices reach a hub hosted on this machine.
#[derive(Clone, Debug, Default)]
pub struct HostedHub {
    /// Loopback URL this device itself uses, e.g. `http://127.0.0.1:4747`.
    pub local_url: String,
    /// Tailnet URLs for other devices.
    pub urls: Vec<String>,
    pub token: String,
}
