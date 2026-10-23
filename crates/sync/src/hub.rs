//! The sync hub: an HTTP server holding the merged state of every device.

use crate::net;
use crate::proto::{HubInfo, SyncRequest, SyncResponse, WaitResponse};
use patch_core::doc::write_atomic;
use patch_core::id::{pairing_token, random_string};
use patch_core::Record;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server};

const MAX_BODY: u64 = 64 * 1024 * 1024;
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

#[derive(Serialize, Deserialize)]
struct Stored {
    seq: u64,
    rec: Record,
}

#[derive(Serialize, Deserialize)]
struct State {
    hub: String,
    seq: u64,
    records: BTreeMap<String, Stored>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    file: PathBuf,
    token: String,
    allow_lan: bool,
    name: String,
    port: u16,
    stopping: AtomicBool,
}

pub struct Hub {
    shared: Arc<Shared>,
    server: Arc<Server>,
    thread: Option<JoinHandle<()>>,
}

fn load_token(cfg: &HubConfig) -> io::Result<String> {
    if let Some(t) = &cfg.token {
        return Ok(t.trim().to_string());
    }
    let path = cfg.data_dir.join("token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
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

fn json_response(status: u16, body: String) -> Response<io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(status)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("static header"))
}

fn error(status: u16, msg: &str) -> Response<io::Cursor<Vec<u8>>> {
    json_response(status, serde_json::json!({ "error": msg }).to_string())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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
        let state = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str::<State>(&s).ok())
            .unwrap_or_else(|| State { hub: format!("hub-{}", random_string(10)), seq: 0, records: BTreeMap::new() });
        let server = Server::http(&cfg.bind).map_err(|e| io::Error::new(io::ErrorKind::AddrInUse, e.to_string()))?;
        let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(crate::DEFAULT_PORT);
        let server = Arc::new(server);
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            changed: Condvar::new(),
            file,
            token,
            allow_lan: cfg.allow_lan,
            name: cfg.name,
            port,
            stopping: AtomicBool::new(false),
        });
        {
            // Persist a freshly generated hub id right away so it is stable.
            let st = shared.state.lock().unwrap();
            if !shared.file.exists() {
                let _ = write_atomic(&shared.file, serde_json::to_string(&*st).unwrap_or_default().as_bytes());
            }
        }
        let thread = {
            let server = server.clone();
            let shared = shared.clone();
            std::thread::Builder::new().name("patch-hub".into()).spawn(move || {
                for req in server.incoming_requests() {
                    let shared = shared.clone();
                    let _ = std::thread::Builder::new()
                        .name("patch-hub-req".into())
                        .spawn(move || handle(&shared, req));
                }
            })?
        };
        Ok(Hub { shared, server, thread: Some(thread) })
    }

    pub fn port(&self) -> u16 {
        self.shared.port
    }

    pub fn token(&self) -> &str {
        &self.shared.token
    }

    pub fn hub_id(&self) -> String {
        self.shared.state.lock().map(|s| s.hub.clone()).unwrap_or_default()
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
        self.server.unblock();
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

fn handle(shared: &Shared, mut req: Request) {
    let peer_ok = req.remote_addr().is_some_and(|a| net::peer_allowed(a.ip(), shared.allow_lan));
    if !peer_ok {
        let _ = req.respond(error(403, "Patch only syncs over Tailscale"));
        return;
    }
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();

    if path == "/api/ping" {
        let hub = shared.state.lock().map(|s| s.hub.clone()).unwrap_or_default();
        let _ = req.respond(json_response(200, serde_json::json!({ "ok": true, "hub": hub, "name": shared.name }).to_string()));
        return;
    }

    let header_token = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str().trim().trim_start_matches("Bearer").trim().to_string());
    let token = header_token.or_else(|| query_param(&url, "token").map(str::to_string)).unwrap_or_default();
    if !constant_time_eq(token.as_bytes(), shared.token.as_bytes()) {
        let _ = req.respond(error(401, "Wrong pairing token"));
        return;
    }

    let response = match (req.method(), path.as_str()) {
        (Method::Post, "/api/sync") => {
            let mut body = Vec::new();
            match req.as_reader().take(MAX_BODY).read_to_end(&mut body) {
                Ok(_) => match serde_json::from_slice::<SyncRequest>(&body) {
                    Ok(sync) => json_response(200, serde_json::to_string(&apply(shared, sync)).unwrap_or_default()),
                    Err(e) => error(400, &format!("bad sync request: {e}")),
                },
                Err(e) => error(400, &e.to_string()),
            }
        }
        (Method::Get, "/api/wait") => {
            let since = query_param(&url, "since").and_then(|s| s.parse().ok()).unwrap_or(0);
            json_response(200, serde_json::to_string(&wait(shared, since)).unwrap_or_default())
        }
        (Method::Get, "/api/info") => {
            let st = shared.state.lock().unwrap();
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
    };
    let _ = req.respond(response);
}

fn apply(shared: &Shared, sync: SyncRequest) -> SyncResponse {
    let mut st = shared.state.lock().unwrap();
    let mut changed = false;
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
        if let Err(e) = write_atomic(&shared.file, serde_json::to_string(&*st).unwrap_or_default().as_bytes()) {
            eprintln!("patch-hub: failed to save {}: {e}", shared.file.display());
        }
        shared.changed.notify_all();
    }
    let since = if sync.since > st.seq { 0 } else { sync.since };
    SyncResponse {
        hub: st.hub.clone(),
        seq: st.seq,
        changes: st.records.values().filter(|s| s.seq > since).map(|s| s.rec.clone()).collect(),
    }
}

/// Long poll: returns as soon as the hub has something newer than `since`.
fn wait(shared: &Shared, since: u64) -> WaitResponse {
    let deadline = Instant::now() + MAX_WAIT;
    let mut st = shared.state.lock().unwrap();
    while st.seq <= since && !shared.stopping.load(Ordering::SeqCst) {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        st = shared.changed.wait_timeout(st, deadline - now).unwrap().0;
    }
    WaitResponse { hub: st.hub.clone(), seq: st.seq }
}
