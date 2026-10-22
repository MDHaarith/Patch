//! The hub as the desktop app hosts it (`hosting::start`), exercised with the
//! same sync client the phone and other desktops use.

use patch_core::doc::{COL_TODO, DEFAULT_LIST};
use patch_core::{Doc, Task};
use patch_linux::hosting;
use patch_sync::{SyncClient, SyncConfig, SyncStatus};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("patch-linux-test-{name}-{}", patch_core::id::random_string(6)));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn free_port() -> u16 {
    TcpListener::bind("0.0.0.0:0").unwrap().local_addr().unwrap().port()
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

fn task(title: &str) -> Task {
    let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
    t.title = title.into();
    t
}

#[test]
fn busy_port_falls_back_and_sticks() {
    let dir = temp_dir("fallback");
    let port = free_port();
    let squatter = TcpListener::bind(("0.0.0.0", port)).unwrap();
    let hub = hosting::start(&dir, port).unwrap();
    assert_ne!(hub.port(), port);
    assert_eq!(hosting::remembered_port(&dir), hub.port(), "the fallback port is reused next time");
    let token = hub.token().to_string();
    hub.stop();
    drop(squatter);

    // Restarting keeps the token, so paired devices keep working.
    let again = hosting::start(&dir, hosting::remembered_port(&dir)).unwrap();
    assert_eq!(again.token(), token);
}

#[test]
fn devices_sync_through_the_hosted_hub() {
    let hub = hosting::start(&temp_dir("e2e"), free_port()).unwrap();
    // Exactly what the shell hands the App as `HostedHub::local_url`.
    let local_url = format!("http://127.0.0.1:{}", hub.port());
    assert!(hosting::patch_hub_running(hub.port()));

    let desktop = Arc::new(Mutex::new(Doc::with_node("desktop")));
    let phone = Arc::new(Mutex::new(Doc::with_node("phone")));
    let a = SyncClient::start(desktop.clone(), Some(SyncConfig::new(&local_url, hub.token()).unwrap()), |_| {});
    let b = SyncClient::start(phone.clone(), Some(SyncConfig::new(&local_url, hub.token()).unwrap()), |_| {});

    let id = desktop.lock().unwrap().add_task(task("Written on the desktop"));
    a.notify_local_change();
    wait_until("phone to receive the task", || phone.lock().unwrap().task(&id).is_some());

    let id2 = phone.lock().unwrap().add_task(task("Added on the phone"));
    b.notify_local_change();
    wait_until("desktop to receive the phone's task", || desktop.lock().unwrap().task(&id2).is_some());
    wait_until("both synced", || {
        matches!(a.status(), SyncStatus::Synced(_)) && matches!(b.status(), SyncStatus::Synced(_))
    });

    let wrong = Arc::new(Mutex::new(Doc::with_node("stranger")));
    let c = SyncClient::start(wrong, Some(SyncConfig::new(&local_url, "NOT-THE-TOKEN").unwrap()), |_| {});
    wait_until("stranger rejected", || c.status() == SyncStatus::Unauthorized);
}

/// Pushes a task into a running Patch the way a paired phone would:
///
///     PATCH_HUB_URL=http://127.0.0.1:4747 PATCH_HUB_TOKEN=… \
///         cargo test -p patch-linux --test hub -- --ignored live
#[test]
#[ignore = "needs a running Patch that hosts the hub"]
fn live_phone_pushes_to_running_app() {
    let url = std::env::var("PATCH_HUB_URL").expect("PATCH_HUB_URL");
    let token = std::env::var("PATCH_HUB_TOKEN").expect("PATCH_HUB_TOKEN");
    let phone = Arc::new(Mutex::new(Doc::with_node("phone")));
    let client = SyncClient::start(phone.clone(), Some(SyncConfig::new(&url, &token).unwrap()), |_| {});
    let now = patch_core::time::now_local();
    let mut t = task("Sent from the phone");
    t.start = Some(now + chrono::Duration::hours(1));
    t.end = Some(now + chrono::Duration::hours(2));
    phone.lock().unwrap().add_task(t);
    client.notify_local_change();
    wait_until("pushed", || matches!(client.status(), SyncStatus::Synced(_)) && !phone.lock().unwrap().has_pending());
}
