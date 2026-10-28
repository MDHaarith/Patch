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

#[test]
fn clock_rejection_keeps_pending_edits() {
    let dir = temp_dir("clock");
    let mut cfg = HubConfig::new(&dir);
    cfg.bind = "127.0.0.1:0".into();
    let hub = Hub::start(cfg).unwrap();
    let mut d = Doc::with_node("ahead");
    let mut rec = patch_core::Record::new("future", "task");
    let stamp = patch_core::Stamp::new(
        patch_core::hlc::now_ms() + patch_core::hlc::MAX_FUTURE_SKEW_MS + 60_000, 0, "ahead",
    );
    rec.put("notes", serde_json::json!("Keep this"), &stamp);
    d.apply_remote(&[rec]);
    let mut task = d.task("future").unwrap();
    task.title = "Pending".into();
    d.save_task(&task);
    let doc = Arc::new(Mutex::new(d));
    let url = format!("http://127.0.0.1:{}", hub.port());
    let client = SyncClient::start(doc.clone(), Some(SyncConfig::new(&url, &hub.token()).unwrap()), |_| {});
    wait_until("visible clock error", || client.status() == SyncStatus::ClockAhead);
    assert!(doc.lock().unwrap().has_pending());
    assert_eq!(doc.lock().unwrap().last_seq, 0);
    drop(client);
    hub.stop();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn config_changes_stop_in_flight_rounds_to_the_old_hub() {
    use tiny_http::{Response, Server};

    for disconnect in [true, false] {
        let old = Server::http("127.0.0.1:0").unwrap();
        let new = Server::http("127.0.0.1:0").unwrap();
        let doc = Arc::new(Mutex::new(Doc::with_node("changing-hubs")));
        let cfg = |server: &Server| SyncConfig::new(&server.server_addr().to_string(), "TOKEN").unwrap();
        let client = SyncClient::start(doc.clone(), Some(cfg(&old)), |_| {});
        let mut req = old.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        assert_eq!(req.url(), "/api/sync");
        req.as_reader().read_to_end(&mut Vec::new()).unwrap();
        client.set_config(if disconnect { None } else { Some(cfg(&new)) });
        req.respond(Response::from_string(r#"{"hub":"old","seq":99,"changes":[]}"#)).unwrap();
        if disconnect {
            assert_eq!(client.status(), SyncStatus::Disabled);
        } else {
            let mut req = new.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
            req.as_reader().read_to_end(&mut Vec::new()).unwrap();
            // Keep the new round in flight while checking the old hub.
            assert_eq!(doc.lock().unwrap().hub_id, None);
            client.set_config(None);
            req.respond(Response::from_string(r#"{"hub":"new","seq":0,"changes":[]}"#)).unwrap();
        }
        assert!(old.recv_timeout(Duration::from_millis(700)).unwrap().is_none(), "old hub received another request");
        assert_eq!(doc.lock().unwrap().hub_id, None, "stale response was applied");
        assert_eq!(doc.lock().unwrap().last_seq, 0);
    }
}

#[test]
fn concurrent_fields_and_tombstones_converge_without_overwriting_other_edits() {
    let dir = temp_dir("concurrent");
    let mut cfg = HubConfig::new(&dir);
    cfg.bind = "127.0.0.1:0".into();
    let hub = Hub::start(cfg).unwrap();
    let mut base = Doc::with_node("base");
    let mut task = Task::blank("", DEFAULT_LIST, COL_TODO);
    task.title = "Base".into();
    let id = base.add_task(task);
    let mut left = Doc::with_node("left");
    let mut right = Doc::with_node("right");
    left.apply_remote(&base.pending());
    right.apply_remote(&base.pending());
    let mut task = left.task(&id).unwrap();
    task.title = "Left title".into();
    left.save_task(&task);
    let mut task = right.task(&id).unwrap();
    task.notes = "Right notes".into();
    right.save_task(&task);
    let left = Arc::new(Mutex::new(left));
    let right = Arc::new(Mutex::new(right));
    let cfg = SyncConfig::new(&format!("http://127.0.0.1:{}", hub.port()), &hub.token()).unwrap();
    let a = SyncClient::start(left.clone(), Some(cfg.clone()), |_| {});
    let b = SyncClient::start(right.clone(), Some(cfg), |_| {});
    wait_until("both concurrent fields", || {
        [left.clone(), right.clone()].iter().all(|doc| {
            let d = doc.lock().unwrap();
            d.task(&id).is_some_and(|t| t.title == "Left title" && t.notes == "Right notes") && !d.has_pending()
        })
    });
    // A concurrent edit must not resurrect a deleted record.
    left.lock().unwrap().delete_task(&id);
    let mut task = right.lock().unwrap().task(&id).unwrap();
    task.notes = "Notes while deleted elsewhere".into();
    right.lock().unwrap().save_task(&task);
    a.notify_local_change();
    b.notify_local_change();
    wait_until("deletion and notes converge", || {
        [left.clone(), right.clone()].iter().all(|doc| {
            let d = doc.lock().unwrap();
            let record = d.record(&id).unwrap();
            record.is_deleted() && record.str("notes") == Some("Notes while deleted elsewhere") && !d.has_pending()
        })
    });
    drop(a);
    drop(b);
    hub.stop();
    std::fs::remove_dir_all(dir).unwrap();
}
