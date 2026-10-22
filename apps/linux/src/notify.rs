//! Desktop notifications (org.freedesktop.Notifications over D-Bus).

use notify_rust::{Hint, Notification, Timeout};
use patch_linux::paths::APP_ID;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Notifications whose "Open" action is still being waited for. Each wait
/// parks a thread until the notification is clicked or dismissed, and GNOME
/// keeps unread notifications around indefinitely, so cap them.
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
        let mut n = Notification::new();
        n.appname("Patch")
            .summary(summary)
            .body(body)
            .icon(&self.icon)
            .hint(Hint::DesktopEntry(APP_ID.into()))
            .timeout(Timeout::Default);
        let wait = self.waiting.fetch_add(1, Ordering::SeqCst) < MAX_WAITING;
        if wait {
            n.action("default", "Open Patch");
        } else {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
        }
        let this = self.clone();
        let spawned = std::thread::Builder::new().name("patch-notify".into()).spawn(move || match n.show() {
            Ok(handle) if wait => {
                handle.wait_for_action(|action| {
                    if action == "default" {
                        (this.on_open)();
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
