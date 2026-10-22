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

    /// Hand the upcoming reminders to the OS so they fire even when the app
    /// isn't running. Each call replaces the previous set. Return true when
    /// the host delivers them itself, so the app's own timer stays quiet.
    fn schedule_reminders(&self, _reminders: &[Reminder]) -> bool {
        false
    }

    fn copy_text(&self, _text: &str) {}

    /// A text field gained or lost focus (Android raises the keyboard).
    fn text_focus(&self, _focused: bool) {}
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
