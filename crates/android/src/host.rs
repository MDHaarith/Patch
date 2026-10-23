//! [`patch_ui::Host`] for Android: everything is forwarded to Java.

use crate::runtime::Bridge;
use patch_ui::{Host, Reminder};
use std::cell::Cell;
use std::rc::Rc;

/// Requests the host makes of the runtime, picked up before the next frame.
#[derive(Default)]
pub struct HostRequests {
    /// The overlay switched between bubble and panel; its surface is about to
    /// be resized, so the runtime holds its frames until the new size lands.
    pub overlay_expand: Cell<Option<bool>>,
}

pub struct AndroidHost {
    bridge: Rc<dyn Bridge>,
    requests: Rc<HostRequests>,
}

impl AndroidHost {
    pub fn new(bridge: Rc<dyn Bridge>, requests: Rc<HostRequests>) -> AndroidHost {
        AndroidHost { bridge, requests }
    }
}

impl Host for AndroidHost {
    fn is_mobile(&self) -> bool {
        true
    }

    fn can_host_hub(&self) -> bool {
        false
    }

    fn set_overlay(&self, on: bool) -> bool {
        self.bridge.set_overlay(on)
    }

    fn overlay_allowed(&self) -> bool {
        self.bridge.overlay_allowed()
    }

    fn set_overlay_boot(&self, on: bool) {
        self.bridge.set_overlay_boot(on);
    }

    fn overlay_expand(&self, expanded: bool) {
        self.requests.overlay_expand.set(Some(expanded));
        self.bridge.overlay_expand(expanded);
    }

    fn open_app(&self, task_id: Option<&str>) {
        self.bridge.open_app(task_id.filter(|id| !id.is_empty()));
    }

    fn notify(&self, title: &str, body: &str) {
        self.bridge.notify(title, body);
    }

    fn schedule_reminders(&self, reminders: &[Reminder]) -> bool {
        let list: Vec<serde_json::Value> = reminders
            .iter()
            .map(|r| serde_json::json!({ "key": r.key, "at": r.at_ms, "title": r.title, "body": r.body }))
            .collect();
        self.bridge.schedule_reminders(&serde_json::Value::Array(list).to_string());
        true
    }

    fn copy_text(&self, text: &str) {
        self.bridge.set_clipboard(text);
    }

    fn publish_snapshot(&self, json: &str) {
        self.bridge.publish_snapshot(json);
    }

    fn can_scan_qr(&self) -> bool {
        true
    }

    fn scan_qr(&self) {
        self.bridge.scan_qr();
    }

    fn battery_unrestricted(&self) -> Option<bool> {
        Some(self.bridge.battery_unrestricted())
    }

    fn request_battery_unrestricted(&self) {
        self.bridge.request_battery_unrestricted();
    }

    fn text_focus(&self, _focused: bool) {
        // The keyboard follows the text inputs' own requests (see ime.rs),
        // which also cover re-tapping a field after the keyboard was
        // dismissed — something a focus change can't signal.
    }
}
