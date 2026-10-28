//! The sync hub: an HTTP server holding the merged state of every device.

use crate::http::{read_head, DeadlineStream, MAX_BODY};
use crate::net;
use crate::proto::{HubInfo, SyncRequest, SyncResponse, WaitResponse};
use patch_core::doc::{preserve_corrupt, write_atomic};
use patch_core::hlc::{now_ms, MAX_FUTURE_SKEW_MS};
use patch_core::id::{pairing_token, random_string};
use patch_core::Record;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, BufReader, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tiny_http::{Header, HTTPVersion, Response};

const MAX_REQUESTS: usize = 64;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_WAIT: Duration = Duration::from_secs(25);

#[derive(Clone, Debug)]
pub struct HubConfig {
    /// Directory holding `hub.json` and `token`.
    pub data_dir: PathBuf,
    /// Listen address, e.g. `0.0.0.0:4747`.
    pub bind: String,
    /// Pairing token; loaded from / saved to `data_dir/token` when `None`.
    pub token: Option<String>,
    /// Also accept peers on private LAN ranges (not just the tailnet).
    pub allow_lan: bool,
    pub name: String,
}

impl HubConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> HubConfig {
        HubConfig {
            data_dir: data_dir.into(),
            bind: format!("0.0.0.0:{}", crate::DEFAULT_PORT),
            token: None,
            allow_lan: false,
            name: "Patch hub".into(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    seq: u64,
    rec: Record,
}

#[derive(Clone, Serialize, Deserialize)]
struct State {
    hub: String,
    seq: u64,
    records: BTreeMap<String, Stored>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    file: PathBuf,
    registry: Mutex<crate::devices::Registry>,
    allow_lan: bool,
    name: String,
    port: u16,
    stopping: AtomicBool,
    active_requests: AtomicUsize,
    revocations: AtomicUsize,
}

pub struct Hub {
    shared: Arc<Shared>,
    server: Arc<TcpListener>,
    thread: Option<JoinHandle<()>>,
}

fn load_token(cfg: &HubConfig) -> io::Result<String> {
    if let Some(t) = &cfg.token {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
    }
    let path = cfg.data_dir.join("token");
    match std::fs::read_to_string(&path) {
        Ok(t) if !t.trim().is_empty() => return Ok(t.trim().to_string()),
        Ok(_) => {},
        Err(e) if e.kind() == io::ErrorKind::NotFound => {},
        Err(e) => return Err(e),
    }
    let token = pairing_token();
    write_atomic(&path, token.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

fn save_state(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn json_response(status: u16, body: String) -> Response<io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(status)
        .with_chunked_threshold(MAX_BODY + 1)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("static header"))
        .with_header(Header::from_bytes(&b"Connection"[..], &b"close"[..]).expect("static header"))
}

fn error(status: u16, msg: &str) -> Response<io::Cursor<Vec<u8>>> {
    json_response(status, serde_json::json!({ "error": msg }).to_string())
}

fn query_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == key).then_some(v)
    })
}

impl Hub {
    pub fn start(cfg: HubConfig) -> io::Result<Hub> {
        std::fs::create_dir_all(&cfg.data_dir)?;
        let token = load_token(&cfg)?;
        let file = cfg.data_dir.join("hub.json");
        let state = match std::fs::read(&file) {
            Ok(bytes) => match serde_json::from_slice::<State>(&bytes) {
                Ok(state) => Some(state),
                Err(_) => {
                    preserve_corrupt(&file)?;
                    None
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        }
        .unwrap_or_else(|| State { hub: format!("hub-{}", random_string(10)), seq: 0, records: BTreeMap::new() });
        // tiny_http pre-reads small bodies and exposes no socket deadlines or connection limit.
        let server = TcpListener::bind(&cfg.bind)?;
        let port = server.local_addr()?.port();
        let server = Arc::new(server);
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            changed: Condvar::new(),
            file,
            registry: Mutex::new(crate::devices::Registry::load(&cfg.data_dir, token)?),
            allow_lan: cfg.allow_lan,
            name: cfg.name,
            port,
            stopping: AtomicBool::new(false),
            active_requests: AtomicUsize::new(0),
            revocations: AtomicUsize::new(0),
        });
        {
            // Persist a freshly generated hub id right away so it is stable.
            let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if !shared.file.exists() {
                save_state(&shared.file, &serde_json::to_vec(&*st)?)?;
            }
        }
        let thread = {
            let server = server.clone();
            let shared = shared.clone();
            std::thread::Builder::new().name("patch-hub".into()).spawn(move || {
                for stream in server.incoming() {
                    if shared.stopping.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = stream else { continue };
                    if shared.active_requests.load(Ordering::SeqCst) >= MAX_REQUESTS {
                        let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                        let _ = error(503, "Hub is busy").raw_print(&mut stream, HTTPVersion(1, 1), &[], false, None);
                        continue;
                    }
                    shared.active_requests.fetch_add(1, Ordering::SeqCst);
                    let active = ActiveRequest(shared.clone());
                    let _ = std::thread::Builder::new()
                        .name("patch-hub-req".into())
                        .spawn(move || handle(&active.0, stream));
                }
            })?
        };
        Ok(Hub { shared, server, thread: Some(thread) })
    }

    pub fn port(&self) -> u16 {
        self.shared.port
    }

    pub fn token(&self) -> String {
        self.shared.registry.lock().unwrap().code.clone()
    }

    pub fn devices(&self) -> Vec<crate::Device> {
        self.shared.registry.lock().unwrap().devices()
    }

    pub fn revoke_device(&self, id: &str) -> io::Result<()> {
        self.shared.registry.lock().unwrap().revoke(id)?;
        let _state = self.shared.state.lock().unwrap();
        self.shared.revocations.fetch_add(1, Ordering::SeqCst);
        self.shared.changed.notify_all();
        Ok(())
    }

    pub fn regenerate_code(&self) -> io::Result<String> {
        self.shared.registry.lock().unwrap().rotate()
    }

    pub fn hub_id(&self) -> String {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner()).hub.clone()
    }

    /// URLs other devices can use to pair with this hub.
    pub fn urls(&self) -> Vec<String> {
        net::hub_urls(self.port(), self.shared.allow_lan)
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        self.shared.changed.notify_all();
        if let Ok(mut addr) = self.server.local_addr() {
            if addr.ip().is_unspecified() {
                addr.set_ip(if addr.is_ipv4() { IpAddr::V4(Ipv4Addr::LOCALHOST) } else { IpAddr::V6(Ipv6Addr::LOCALHOST) });
            }
            let _ = TcpStream::connect_timeout(&addr, Duration::from_secs(1));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct ActiveRequest(Arc<Shared>);

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        self.0.active_requests.fetch_sub(1, Ordering::SeqCst);
    }
}

fn handle(shared: &Shared, stream: TcpStream) {
    let peer_address = stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    let peer_ok = stream.peer_addr().is_ok_and(|a| net::peer_allowed(a.ip(), shared.allow_lan));
    let mut stream = DeadlineStream { stream, deadline: Instant::now() + READ_TIMEOUT };
    let response = if peer_ok {
        handle_request_from(shared, &mut BufReader::new(&mut stream), &peer_address)
    } else {
        error(403, "Patch only syncs over Tailscale")
    };
    stream.deadline = Instant::now() + Duration::from_secs(15);
    let _ = response.raw_print(&mut stream, HTTPVersion(1, 1), &[], false, None);
}

#[cfg(test)]
fn handle_request(shared: &Shared, reader: &mut impl Read) -> Response<io::Cursor<Vec<u8>>> {
    handle_request_from(shared, reader, "127.0.0.1")
}

fn handle_request_from(shared: &Shared, reader: &mut impl Read, address: &str) -> Response<io::Cursor<Vec<u8>>> {
    let head = match read_head(reader) {
        Ok(head) => head,
        Err(e) => return error(400, &e.to_string()),
    };
    let mut lines = head.trim_end_matches("\r\n").split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default();
    let url = request_line.next().unwrap_or_default();
    if !url.starts_with('/') || !matches!(request_line.next(), Some("HTTP/1.1" | "HTTP/1.0")) || request_line.next().is_some() {
        return error(400, "Bad request line");
    }
    let mut headers = Vec::new();
    for line in lines {
        let Ok(header) = line.parse::<Header>() else { return error(400, "Bad header") };
        headers.push(header);
    }
    let path = url.split('?').next().unwrap_or("");

    if path == "/api/ping" {
        let hub = shared.state.lock().unwrap_or_else(|e| e.into_inner()).hub.clone();
        return json_response(200, serde_json::json!({ "ok": true, "hub": hub, "name": shared.name }).to_string());
    }

    let revocations = shared.revocations.load(Ordering::SeqCst);
    let token = headers.iter()
        .find(|h| h.field.equiv("Authorization"))
        .and_then(|h| h.value.as_str().trim().strip_prefix("Bearer "))
        .unwrap_or_default().trim();
    if !shared.registry.lock().unwrap().known_token(token) {
        return error(401, "Wrong pairing token");
    }

    if (method, path) != ("POST", "/api/sync") {
        if let Err((code, msg)) = shared.registry.lock().unwrap().authorize(token, None, None, address, false) {
            return error(code, &msg);
        }
    }
    match (method, path) {
        ("POST", "/api/sync") => {
            let mut length = None;
            for header in &headers {
                // shortcut: uploads require Content-Length; add chunked uploads if another client needs them.
                if header.field.equiv("Transfer-Encoding") {
                    return error(400, "Use Content-Length, not Transfer-Encoding");
                }
                if header.field.equiv("Content-Length") {
                    if length.is_some() {
                        return error(400, "Duplicate Content-Length");
                    }
                    let Ok(n) = header.value.as_str().parse::<usize>() else { return error(400, "Bad Content-Length") };
                    if n > MAX_BODY {
                        return error(413, "Sync request too large");
                    }
                    length = Some(n);
                }
            }
            let Some(length) = length else { return error(411, "Content-Length required") };
            let mut body = Vec::new();
            match reader.take(length as u64).read_to_end(&mut body) {
                Ok(_) if body.len() != length => error(400, "Truncated body"),
                Ok(_) => match serde_json::from_slice::<SyncRequest>(&body) {
                    Ok(sync) => {
                        let sync: SyncRequest = sync;
                        let auth = shared.registry.lock().unwrap().authorize(token, sync.node.as_deref(), sync.device.as_ref(), address, true);
                        let credential = match auth { Ok(c) => c, Err((code, msg)) => return error(code, &msg) };
                        match apply(shared, sync) {
                        Ok(mut response) => { response.credential = credential; json_response(200, serde_json::to_string(&response).unwrap_or_default()) },
                        Err((code, message)) => error(code, &message),
                    }},
                    Err(e) => error(400, &format!("bad sync request: {e}")),
                },
                Err(e) => error(400, &e.to_string()),
            }
        }
        ("GET", "/api/wait") => {
            let since = query_param(url, "since").and_then(|s| s.parse().ok()).unwrap_or(0);
            let response = wait_for(shared, since, revocations);
            if let Err((code, msg)) = shared.registry.lock().unwrap().authorize(token, None, None, address, false) { return error(code, &msg); }
            json_response(200, serde_json::to_string(&response).unwrap_or_default())
        }
        ("GET", "/api/info") => {
            let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            let info = HubInfo {
                hub: st.hub.clone(),
                name: shared.name.clone(),
                version: env!("CARGO_PKG_VERSION").into(),
                seq: st.seq,
                records: st.records.len(),
                urls: net::hub_urls(shared.port, shared.allow_lan),
            };
            json_response(200, serde_json::to_string(&info).unwrap_or_default())
        }
        _ => error(404, "not found"),
    }
}

fn apply(shared: &Shared, sync: SyncRequest) -> Result<SyncResponse, (u16, String)> {
    let mut committed = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    let reset = sync.since > committed.seq;
    let mut st = committed.clone();
    let mut changed = false;
    let max_wall = now_ms().saturating_add(MAX_FUTURE_SKEW_MS);
    for field in sync.changes.iter().flat_map(|rec| rec.f.values()) {
        let Some(wall) = crate::proto::stamp_wall(&field.t) else {
            return Err((400, "Invalid change stamp".into()));
        };
        if wall > max_wall {
            return Err((409, "This device's clock is ahead of the hub; fix the date and time to sync".into()));
        }
    }
    for rec in sync.changes {
        let next = st.seq + 1;
        match st.records.get_mut(&rec.id) {
            Some(stored) => {
                if stored.rec.merge(&rec) {
                    stored.seq = next;
                    st.seq = next;
                    changed = true;
                }
            }
            None => {
                st.records.insert(rec.id.clone(), Stored { seq: next, rec });
                st.seq = next;
                changed = true;
            }
        }
    }
    if changed {
        save_state(&shared.file, &serde_json::to_vec(&st).map_err(|e| (500, e.to_string()))?)
            .map_err(|e| (500, format!("Failed to save hub state: {e}")))?;
    }
    let since = if reset { 0 } else { sync.since };
    let response = SyncResponse {
        name: Some(shared.name.clone()),        credential: None,
        hub: st.hub.clone(),
        seq: st.seq,
        reset,
        changes: st.records.values().filter(|s| s.seq > since).map(|s| s.rec.clone()).collect(),
    };
    if changed {
        // Readers and long polls may only see changes after durable persistence.
        *committed = st;
        shared.changed.notify_all();
    }
    Ok(response)
}

/// Long poll: returns as soon as the hub has something newer than `since`.
#[cfg(test)]
fn wait(shared: &Shared, since: u64) -> WaitResponse {
    wait_for(shared, since, shared.revocations.load(Ordering::SeqCst))
}

fn wait_for(shared: &Shared, since: u64, revocations: usize) -> WaitResponse {
    let deadline = Instant::now() + MAX_WAIT;
    let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    while st.seq == since && !shared.stopping.load(Ordering::SeqCst) {
        if shared.revocations.load(Ordering::SeqCst) != revocations { break; }
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        st = shared.changed.wait_timeout(st, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
    }
    WaitResponse { hub: st.hub.clone(), seq: st.seq }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patch_core::Stamp;
    use serde_json::json;

    #[test]
    fn invalid_stamps_are_rejected_before_insert_and_merge() {
        let dir = std::env::temp_dir().join(format!("patch-hub-stamps-{}", random_string(10)));
        let mut cfg = HubConfig::new(&dir);
        cfg.bind = "127.0.0.1:0".into();
        let hub = Hub::start(cfg).unwrap();
        let mut good = Record::new("task", "task");
        good.put("notes", json!("original"), &Stamp::seed());
        apply(&hub.shared, SyncRequest { device: None, since: 0, changes: vec![good.clone()], node: None }).unwrap();
        let seq = hub.shared.state.lock().unwrap().seq;
        for stamp in [
            "z".to_string(), "1-0000-node".into(), "000000000000001-0000".into(),
            "000000000000001-0000-".into(), "000000000000001-+001-node".into(),
            Stamp::new(now_ms(), 10_000, "node").0,
            Stamp::new(now_ms(), u32::MAX - 1, "node").0,
            Stamp::new(now_ms(), u32::MAX, "node").0,
        ] {
            for id in ["new", "task"] {
                let mut bad = Record::new(id, "task");
                bad.put("notes", json!("poison"), &Stamp(stamp.clone()));
                let result = apply(&hub.shared, SyncRequest { device: None, since: 0, changes: vec![bad], node: None });
                assert_eq!(result.unwrap_err().0, 400, "accepted {stamp}");
            }
        }
        let st = hub.shared.state.lock().unwrap();
        assert_eq!(st.seq, seq);
        assert_eq!(st.records.len(), 1);
        assert_eq!(st.records["task"].rec, good);
        drop(st);
        hub.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn blank_configured_tokens_use_the_stored_or_generated_token() {
        let dir = std::env::temp_dir().join(format!("patch-hub-token-{}", random_string(10)));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = HubConfig::new(&dir);
        cfg.token = Some(" \t\n".into());
        let token = load_token(&cfg).unwrap();
        assert!(!token.trim().is_empty());
        cfg.token = Some(String::new());
        assert_eq!(load_token(&cfg).unwrap(), token);
        std::fs::write(dir.join("token"), "   ").unwrap();
        assert!(!load_token(&cfg).unwrap().trim().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unauthorized_partial_bodies_are_rejected_before_reading() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("patch-hub-partial-{}", random_string(10)));
        let mut cfg = HubConfig::new(&dir);
        cfg.bind = "127.0.0.1:0".into();
        cfg.token = Some(String::new());
        let hub = Hub::start(cfg).unwrap();
        for auth in ["", "Authorization: Bearer \r\n", "Authorization: Bearer wrong\r\n"] {
            let mut stream = TcpStream::connect(("127.0.0.1", hub.port())).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
            write!(stream, "POST /api/sync HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Length: 4\r\n\r\n").unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(response.starts_with("HTTP/1.1 401"), "{response}");
        }
        hub.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn authenticated_bodies_have_a_deadline_and_size_limit() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("patch-hub-body-{}", random_string(10)));
        let mut cfg = HubConfig::new(&dir);
        cfg.bind = "127.0.0.1:0".into();
        let hub = Hub::start(cfg).unwrap();
        for (framing, status) in [
            ("Content-Length: 67108865\r\n", 413),
            ("Content-Length: bad\r\n", 400),
            ("Content-Length: 0\r\nContent-Length: 0\r\n", 400),
            ("Transfer-Encoding: chunked\r\n", 400),
            ("", 411),
        ] {
            let head = format!("POST /api/sync HTTP/1.1\r\nAuthorization: Bearer {}\r\n{framing}\r\n", &hub.token());
            assert_eq!(handle_request(&hub.shared, &mut head.as_bytes()).status_code().0, status);
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        write!(client, "POST /api/sync HTTP/1.1\r\nAuthorization: Bearer {}\r\nContent-Length: 20\r\n\r\n", &hub.token()).unwrap();
        let sender = std::thread::spawn(move || {
            for _ in 0..20 {
                if client.write_all(b"x").is_err() { break; }
                std::thread::sleep(Duration::from_millis(40));
            }
        });
        let (stream, _) = listener.accept().unwrap();
        let started = Instant::now();
        let stream = DeadlineStream { stream, deadline: started + Duration::from_millis(250) };
        let response = handle_request(&hub.shared, &mut BufReader::new(stream));
        assert_eq!(response.status_code().0, 400);
        assert!(started.elapsed() < Duration::from_millis(650));
        sender.join().unwrap();
        hub.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn partial_headers_cannot_create_unlimited_workers() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("patch-hub-limit-{}", random_string(10)));
        let mut cfg = HubConfig::new(&dir);
        cfg.bind = "127.0.0.1:0".into();
        let hub = Hub::start(cfg).unwrap();
        let mut sockets = Vec::new();
        for _ in 0..MAX_REQUESTS {
            let mut stream = TcpStream::connect(("127.0.0.1", hub.port())).unwrap();
            stream.write_all(b"POST /api/sync HTTP/1.1\r\n").unwrap();
            sockets.push(stream);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while hub.shared.active_requests.load(Ordering::SeqCst) < MAX_REQUESTS && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(hub.shared.active_requests.load(Ordering::SeqCst), MAX_REQUESTS);
        let mut extra = TcpStream::connect(("127.0.0.1", hub.port())).unwrap();
        extra.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut response = String::new();
        extra.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 503"), "{response}");
        assert_eq!(hub.shared.active_requests.load(Ordering::SeqCst), MAX_REQUESTS);
        drop(sockets);
        let deadline = Instant::now() + Duration::from_secs(3);
        while hub.shared.active_requests.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(hub.shared.active_requests.load(Ordering::SeqCst), 0);
        hub.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_save_is_not_acknowledged_even_on_retry() {
        let dir = std::env::temp_dir().join(format!("patch-hub-save-{}", random_string(10)));
        let mut cfg = HubConfig::new(&dir);
        cfg.bind = "127.0.0.1:0".into();
        let hub = Hub::start(cfg).unwrap();
        // A directory at the state file path deterministically prevents atomic replacement.
        std::fs::remove_file(&hub.shared.file).unwrap();
        std::fs::create_dir(&hub.shared.file).unwrap();
        let mut rec = Record::new("task", "task");
        rec.put("title", json!("saved later"), &Stamp::seed());
        for _ in 0..2 {
            let result = apply(&hub.shared, SyncRequest { device: None, since: 0, changes: vec![rec.clone()], node: None });
            assert_eq!(result.unwrap_err().0, 500);
            let state = hub.shared.state.lock().unwrap();
            assert_eq!(state.seq, 0, "failed save published a cursor");
            assert!(state.records.is_empty(), "failed save published records");
        }
        let pulled = apply(&hub.shared, SyncRequest { device: None, since: 0, changes: vec![], node: None }).unwrap();
        assert_eq!(pulled.seq, 0);
        assert!(pulled.changes.is_empty());
        let shared = hub.shared.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || { tx.send(wait(&shared, 0)).unwrap(); });
        std::fs::remove_dir(&hub.shared.file).unwrap();
        assert!(apply(&hub.shared, SyncRequest { device: None, since: 0, changes: vec![rec], node: None }).is_ok());
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap().seq, 1);
        waiter.join().unwrap();
        hub.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn startup_requires_persisting_a_stable_hub_identity() {
        let dir = std::env::temp_dir().join(format!("patch-hub-startup-{}", random_string(10)));
        std::fs::create_dir_all(dir.join("hub.tmp")).unwrap();
        let mut cfg = HubConfig::new(&dir);
        cfg.bind = "127.0.0.1:0".into();
        assert!(Hub::start(cfg).is_err());
        assert!(!dir.join("hub.json").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_token_is_never_replaced() {
        let dir = std::env::temp_dir().join(format!("patch-hub-unreadable-token-{}", random_string(10)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("token"), [0xff]).unwrap();
        assert!(load_token(&HubConfig::new(&dir)).is_err());
        assert_eq!(std::fs::read(dir.join("token")).unwrap(), [0xff]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rollback_is_detected_before_upload_advances_the_sequence() {
        let dir = std::env::temp_dir().join(format!("patch-hub-upload-rollback-{}", random_string(10)));
        let mut cfg = HubConfig::new(&dir);
        cfg.bind = "127.0.0.1:0".into();
        let hub = Hub::start(cfg).unwrap();
        let changes = ["a", "b"].into_iter().map(|id| {
            let mut rec = Record::new(id, "task");
            rec.put("title", json!(id), &Stamp::seed());
            rec
        }).collect();
        let response = apply(&hub.shared, SyncRequest { device: None, since: 1, changes, node: None }).unwrap();
        assert_eq!(response.seq, 2);
        assert!(response.reset);
        assert_eq!(response.changes.len(), 2, "the full state must be returned after rollback");
        hub.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn long_poll_reports_a_restored_cursor_immediately() {
        let dir = std::env::temp_dir().join(format!("patch-hub-rollback-{}", random_string(10)));
        let mut cfg = HubConfig::new(&dir);
        cfg.bind = "127.0.0.1:0".into();
        let hub = Hub::start(cfg).unwrap();
        let shared = hub.shared.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || { let _ = tx.send(wait(&shared, 99)); });
        let result = rx.recv_timeout(Duration::from_secs(1));
        hub.stop();
        waiter.join().unwrap();
        assert_eq!(result.unwrap().seq, 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn future_fields_are_rejected_before_insert_and_merge() {
        let dir = std::env::temp_dir().join(format!("patch-hub-skew-{}", random_string(10)));
        let mut cfg = HubConfig::new(&dir);
        cfg.bind = "127.0.0.1:0".into();
        let hub = Hub::start(cfg).unwrap();
        let mut rec = Record::new("task", "task");
        let future = Stamp::new(now_ms() + MAX_FUTURE_SKEW_MS + 60_000, 0, "remote");
        rec.put("title", json!("future"), &future);
        let send = |rec| apply(&hub.shared, SyncRequest { device: None, since: 0, changes: vec![rec], node: None });
        assert_eq!(send(rec.clone()).unwrap_err().0, 409);
        rec.put("notes", json!("seed"), &Stamp::seed());
        assert_eq!(send(rec.clone()).unwrap_err().0, 409);
        assert!(hub.shared.state.lock().unwrap().records.is_empty());
        rec.f.remove("title");
        assert_eq!(send(rec.clone()).unwrap().changes[0].str("notes"), Some("seed"));
        rec.put("notes", json!("poison"), &future);
        assert_eq!(send(rec).unwrap_err().0, 409);
        assert_eq!(hub.shared.state.lock().unwrap().records["task"].rec.str("notes"), Some("seed"));
        hub.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
