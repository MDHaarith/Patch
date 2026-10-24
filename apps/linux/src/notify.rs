//! Desktop notifications (org.freedesktop.Notifications over D-Bus).

use notify_rust::{Hint, Notification, Timeout};
use patch_linux::paths::APP_ID;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Generic notifications whose "Open" action is still being waited for. Each wait
/// parks a thread until the notification is clicked or dismissed, and GNOME
/// keeps unread notifications around indefinitely, so cap them. Reminders always
/// retain their task actions.
const MAX_WAITING: usize = 4;

#[derive(Clone)]
pub struct Notifier {
    icon: String,
    waiting: Arc<AtomicUsize>,
    on_open: Arc<dyn Fn() + Send + Sync>,
}

impl Notifier {
    /// `on_open` runs on a background thread when the user clicks a
    /// notification.
    pub fn new(icon: String, on_open: impl Fn() + Send + Sync + 'static) -> Notifier {
        Notifier { icon, waiting: Arc::default(), on_open: Arc::new(on_open) }
    }

    /// Sends without blocking the UI: D-Bus round trips happen on a thread.
    pub fn send(&self, summary: &str, body: &str) {
        self.send_with_task(None, summary, body);
    }

    pub fn send_reminder(&self, id: &str, summary: &str, body: &str) {
        self.send_with_task(Some(id.to_owned()), summary, body);
    }

    fn send_with_task(&self, task: Option<String>, summary: &str, body: &str) {
        let summary = summary.to_owned();
        let body = body.to_owned();
        let mut n = Notification::new();
        n.appname("Patch")
            .summary(&summary)
            .body(&body)
            .icon(&self.icon)
            .hint(Hint::DesktopEntry(APP_ID.into()))
            .timeout(Timeout::Default);
        let wait = self.waiting.fetch_add(1, Ordering::SeqCst) < MAX_WAITING || task.is_some();
        if wait {
            n.action("default", "Open Patch");
            if task.is_some() {
                n.action("done", "Done").action("snooze", "Snooze 10 min").hint(Hint::Resident(false));
            }
        } else {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
        }
        let this = self.clone();
        let spawned = std::thread::Builder::new().name("patch-notify".into()).spawn(move || match n.show() {
            Ok(handle) if wait => {
                handle.wait_for_action(|action| {
                    match action {
                        "default" => (this.on_open)(),
                        "done" => {
                            if let Some(id) = task {
                                let _ = slint::invoke_from_event_loop(move || patch_ui::App::complete_current_task(&id));
                            }
                        }
                        "snooze" => {
                            // shortcut: snoozes last only while Patch runs; persist alarms to survive quitting.
                            std::thread::sleep(std::time::Duration::from_secs(600));
                            this.send_with_task(task, &summary, &body);
                        }
                        _ => {}
                    }
                });
                this.waiting.fetch_sub(1, Ordering::SeqCst);
            }
            Ok(_) => {}
            Err(e) => {
                if wait {
                    this.waiting.fetch_sub(1, Ordering::SeqCst);
                }
                eprintln!("patch: can't show notification: {e}");
            }
        });
        if spawned.is_err() && wait {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
