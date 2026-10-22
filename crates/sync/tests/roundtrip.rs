use patch_core::doc::{COL_TODO, DEFAULT_LIST};
use patch_core::{Doc, Task};
use patch_sync::{Hub, HubConfig, SyncClient, SyncConfig, SyncEvent, SyncStatus};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("patch-test-{name}-{}", patch_core::id::random_string(6)));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn two_devices_converge_through_hub() {
    let mut cfg = HubConfig::new(temp_dir("hub"));
    cfg.bind = "127.0.0.1:0".into();
    cfg.token = Some("TEST-TOKEN".into());
    let hub = Hub::start(cfg).unwrap();
    let url = format!("http://127.0.0.1:{}", hub.port());

    let laptop = Arc::new(Mutex::new(Doc::with_node("laptop")));
    let phone = Arc::new(Mutex::new(Doc::with_node("phone")));

    // The laptop has data before it ever syncs.
    let id = {
        let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
        t.title = "Written offline".into();
        laptop.lock().unwrap().add_task(t)
    };

    let (ptx, prx) = mpsc::channel();
    let a = SyncClient::start(laptop.clone(), Some(SyncConfig::new(&url, "TEST-TOKEN").unwrap()), |_| {});
    let b = SyncClient::start(phone.clone(), Some(SyncConfig::new(&url, "TEST-TOKEN").unwrap()), move |e| {
        if let SyncEvent::RemoteApplied = e {
            let _ = ptx.send(());
        }
    });

    wait_until("phone to receive offline task", || phone.lock().unwrap().task(&id).is_some());
    assert!(prx.recv_timeout(Duration::from_secs(5)).is_ok());

    // Live update: phone edits, laptop sees it via long-poll quickly.
    {
        let mut d = phone.lock().unwrap();
        let mut t = d.task(&id).unwrap();
        t.title = "Edited on phone".into();
        d.save_task(&t);
    }
    b.notify_local_change();
    let started = Instant::now();
    wait_until("laptop to see phone edit", || {
        laptop.lock().unwrap().task(&id).map(|t| t.title) == Some("Edited on phone".into())
    });
    assert!(started.elapsed() < Duration::from_secs(5));

    wait_until("both synced", || matches!(a.status(), SyncStatus::Synced(_)) && matches!(b.status(), SyncStatus::Synced(_)));
    wait_until("pending flushed", || !laptop.lock().unwrap().has_pending() && !phone.lock().unwrap().has_pending());
    assert_eq!(laptop.lock().unwrap().tasks(), phone.lock().unwrap().tasks());
}

#[test]
fn wrong_token_is_rejected() {
    let mut cfg = HubConfig::new(temp_dir("auth"));
    cfg.bind = "127.0.0.1:0".into();
    let hub = Hub::start(cfg).unwrap();
    assert_eq!(hub.token().len(), 19);
    let doc = Arc::new(Mutex::new(Doc::with_node("x")));
    let url = format!("http://127.0.0.1:{}", hub.port());
    let c = SyncClient::start(doc, Some(SyncConfig::new(&url, "NOPE").unwrap()), |_| {});
    wait_until("unauthorized", || c.status() == SyncStatus::Unauthorized);
}

#[test]
fn hub_persists_across_restarts() {
    let dir = temp_dir("persist");
    let mut cfg = HubConfig::new(&dir);
    cfg.bind = "127.0.0.1:0".into();
    let hub = Hub::start(cfg.clone()).unwrap();
    let (token, hub_id, port) = (hub.token().to_string(), hub.hub_id(), hub.port());
    let doc = Arc::new(Mutex::new(Doc::with_node("x")));
    let id = {
        let mut t = Task::blank("", DEFAULT_LIST, COL_TODO);
        t.title = "persist".into();
        doc.lock().unwrap().add_task(t)
    };
    let c = SyncClient::start(doc.clone(), Some(SyncConfig::new(&format!("http://127.0.0.1:{port}"), &token).unwrap()), |_| {});
    wait_until("flushed", || !doc.lock().unwrap().has_pending() && matches!(c.status(), SyncStatus::Synced(_)));
    drop(c);
    hub.stop();

    let hub = Hub::start(cfg).unwrap();
    assert_eq!(hub.token(), token);
    assert_eq!(hub.hub_id(), hub_id);
    let fresh = Arc::new(Mutex::new(Doc::with_node("y")));
    let url = format!("http://127.0.0.1:{}", hub.port());
    let _c = SyncClient::start(fresh.clone(), Some(SyncConfig::new(&url, &token).unwrap()), |_| {});
    wait_until("fresh device pulls", || fresh.lock().unwrap().task(&id).is_some());
}
