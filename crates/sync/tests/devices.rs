use patch_core::{
    doc::{COL_TODO, DEFAULT_LIST},
    Doc, Task,
};
use patch_sync::{
    http::{request, Base, HttpError},
    proto::SyncResponse,
    Hub, HubConfig, SyncClient, SyncConfig, SyncStatus,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn config() -> HubConfig {
    let mut cfg = HubConfig::new(std::env::temp_dir().join(format!(
        "patch-devices-{}",
        patch_core::id::random_string(10)
    )));
    cfg.bind = "127.0.0.1:0".into();
    cfg
}
fn base(hub: &Hub) -> Base {
    Base::parse(&format!("127.0.0.1:{}", hub.port())).unwrap()
}
fn sync(hub: &Hub, token: &str, body: serde_json::Value) -> Result<SyncResponse, HttpError> {
    let response = request(
        &base(hub),
        "POST",
        "/api/sync",
        token,
        Some(&serde_json::to_vec(&body).unwrap()),
        Duration::from_secs(2),
    )?;
    Ok(serde_json::from_slice(&response.body).unwrap())
}
fn body(node: &str) -> serde_json::Value {
    serde_json::json!({"since":0,"changes":[],"node":node,"device":{"name":"Pixel 9","platform":"android"}})
}

#[test]
fn registration_issues_distinct_credentials_and_records_identity() {
    let cfg = config();
    let hub = Hub::start(cfg.clone()).unwrap();
    let a = sync(&hub, &hub.token(), body("phone"))
        .unwrap()
        .credential
        .unwrap();
    let b = sync(&hub, &hub.token(), body("tablet"))
        .unwrap()
        .credential
        .unwrap();
    assert_ne!(a, b);
    assert_ne!(a, hub.token());
    let devices = hub.devices();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].name, "Pixel 9");
    assert_eq!(devices[0].platform, "android");
    assert_eq!(devices[0].address, "127.0.0.1");
    assert!(devices[0].last_seen >= devices[0].paired_at);
    sync(&hub, &a, body("phone")).unwrap();
    assert_eq!(hub.devices().len(), 2);
    let info = request(
        &base(&hub),
        "GET",
        "/api/info",
        &a,
        None,
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(
        !String::from_utf8(info.body).unwrap().contains(&a),
        "public info must not expose credentials"
    );
    drop(hub);
    std::fs::remove_dir_all(cfg.data_dir).unwrap();
}

#[test]
fn revocation_rejects_sync_info_and_inflight_poll_and_survives_restart() {
    let cfg = config();
    let hub = Hub::start(cfg.clone()).unwrap();
    let credential = sync(&hub, &hub.token(), body("phone"))
        .unwrap()
        .credential
        .unwrap();
    let poll_base = base(&hub);
    let poll_token = credential.clone();
    let poll = std::thread::spawn(move || {
        request(
            &poll_base,
            "GET",
            "/api/wait?since=0",
            &poll_token,
            None,
            Duration::from_secs(3),
        )
    });
    std::thread::sleep(Duration::from_millis(80));
    hub.revoke_device("phone").unwrap();
    assert!(matches!(
        poll.join().unwrap(),
        Err(HttpError::Status(401, _))
    ));
    assert!(matches!(
        sync(&hub, &credential, body("phone")),
        Err(HttpError::Status(401, _))
    ));
    drop(hub);
    let hub = Hub::start(cfg.clone()).unwrap();
    assert!(hub.devices()[0].revoked);
    for path in ["/api/info", "/api/wait?since=0"] {
        assert!(matches!(
            request(
                &base(&hub),
                "GET",
                path,
                &credential,
                None,
                Duration::from_secs(2)
            ),
            Err(HttpError::Status(401, _))
        ));
    }
    drop(hub);
    std::fs::remove_dir_all(cfg.data_dir).unwrap();
}

#[test]
fn legacy_shared_token_auto_registers_and_keeps_working_after_rotation() {
    let cfg = config();
    let hub = Hub::start(cfg.clone()).unwrap();
    let code = hub.token();
    let legacy = serde_json::json!({"since":0,"changes":[],"node":"old-phone"});
    let credential = sync(&hub, &code, legacy.clone())
        .unwrap()
        .credential
        .unwrap();
    assert_eq!(hub.devices()[0].name, "Unknown device");
    let rotated = hub.regenerate_code().unwrap();
    assert_ne!(rotated, code);
    sync(&hub, &code, legacy.clone()).unwrap();
    sync(&hub, &credential, legacy.clone()).unwrap();
    assert!(matches!(
        sync(&hub, &code, body("unregistered")),
        Err(HttpError::Status(401, _))
    ));
    drop(hub);
    let hub = Hub::start(cfg.clone()).unwrap();
    assert_eq!(hub.token(), rotated);
    assert_eq!(hub.devices().len(), 1);
    sync(&hub, &credential, legacy.clone()).unwrap();
    sync(&hub, &code, legacy.clone()).unwrap();
    hub.revoke_device("old-phone").unwrap();
    assert!(matches!(
        sync(&hub, &code, legacy),
        Err(HttpError::Status(401, _))
    ));
    drop(hub);
    std::fs::remove_dir_all(cfg.data_dir).unwrap();
}

#[test]
fn updated_client_uses_credential_and_keeps_local_data_after_revocation() {
    let cfg = config();
    let hub = Hub::start(cfg.clone()).unwrap();
    let mut doc = Doc::with_node("phone");
    let mut task = Task::blank("", DEFAULT_LIST, COL_TODO);
    task.title = "Keep me".into();
    let id = doc.add_task(task);
    let doc = Arc::new(Mutex::new(doc));
    let client = SyncClient::start(
        doc.clone(),
        Some(SyncConfig::new(&base(&hub).url(), &hub.token()).unwrap()),
        |_| {},
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !matches!(client.status(), SyncStatus::Synced(_)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(matches!(client.status(), SyncStatus::Synced(_)));
    assert!(client.config().unwrap().token.starts_with("device-"));
    hub.regenerate_code().unwrap();
    client.sync_now();
    hub.revoke_device("phone").unwrap();
    client.sync_now();
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.status() != SyncStatus::Unauthorized && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(client.status().label(), "Access revoked, pair again");
    assert_eq!(doc.lock().unwrap().task(&id).unwrap().title, "Keep me");
    drop(client);
    drop(hub);
    std::fs::remove_dir_all(cfg.data_dir).unwrap();
}

#[test]
fn revoked_device_can_pair_again_without_restoring_its_old_credential() {
    let cfg = config();
    let hub = Hub::start(cfg.clone()).unwrap();
    let code = hub.token();
    let old = sync(&hub, &code, body("phone"))
        .unwrap()
        .credential
        .unwrap();
    hub.revoke_device("phone").unwrap();
    assert!(matches!(
        sync(&hub, &code, body("phone")),
        Err(HttpError::Status(401, _))
    ));
    let mut repair = body("phone");
    repair["device"]["pairing"] = true.into();
    let new = sync(&hub, &code, repair).unwrap().credential.unwrap();
    assert_ne!(old, new);
    assert_eq!(hub.devices().len(), 1);
    assert!(!hub.devices()[0].revoked);
    assert!(matches!(
        sync(&hub, &old, body("phone")),
        Err(HttpError::Status(401, _))
    ));
    sync(&hub, &new, body("phone")).unwrap();
    hub.revoke_device("phone").unwrap();
    assert!(matches!(
        sync(&hub, &new, body("phone")),
        Err(HttpError::Status(401, _))
    ));
    drop(hub);
    std::fs::remove_dir_all(cfg.data_dir).unwrap();
}

#[test]
fn malformed_identity_is_rejected_without_registering_a_device() {
    let cfg = config();
    let hub = Hub::start(cfg.clone()).unwrap();
    let mut bad = body("phone");
    bad["device"]["name"] = "x".repeat(257).into();
    assert!(matches!(
        sync(&hub, &hub.token(), bad),
        Err(HttpError::Status(400, _))
    ));
    assert!(hub.devices().is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(cfg.data_dir.join("devices.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    drop(hub);
    std::fs::remove_dir_all(cfg.data_dir).unwrap();
}
