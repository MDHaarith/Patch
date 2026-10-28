//! Background sync client used by the apps.

use crate::DeviceIdentity;
use crate::http::{request, Base, HttpError};
use crate::proto::{SyncRequest, SyncResponse, WaitResponse};
use patch_core::Doc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncConfig {
    pub base: Base,
    pub token: String,
    /// Explicit user pairing; persisted credentials always set this to false.
    pub pairing: bool,
}

impl SyncConfig {
    pub fn new(url: &str, token: &str) -> Result<SyncConfig, String> {
        let token = token.trim();
        if token.is_empty() {
            return Err("Enter the pairing token shown on the hub".into());
        }
        if !token.is_ascii() || token.bytes().any(|b| b.is_ascii_control()) {
            return Err("Bad pairing token".into());
        }
        Ok(SyncConfig { base: Base::parse(url)?, token: token.to_string(), pairing: false })
    }

    /// What a hub's pairing QR code encodes.
    pub fn pair_link(url: &str, token: &str) -> String {
        format!("patch://pair?hub={}&token={}", encode_pair_value(url), encode_pair_value(token))
    }

    /// Reads a scanned [`SyncConfig::pair_link`].
    pub fn from_pair_link(link: &str) -> Result<SyncConfig, String> {
        let query = link.trim().strip_prefix("patch://pair?").ok_or("That QR code isn't a Patch pairing code")?;
        let (mut hub, mut token) = (String::new(), String::new());
        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("hub", v)) => hub = decode_pair_value(v)?,
                Some(("token", v)) => token = decode_pair_value(v)?,
                _ => {}
            }
        }
        SyncConfig::new(&hub, &token)
    }
}

fn encode_pair_value(value: &str) -> String {
    value.bytes().map(|b| match b {
        b'!'..=b'~' if !matches!(b, b'&' | b'%' | b'#') => char::from(b).to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

fn decode_pair_value(value: &str) -> Result<String, String> {
    let mut bytes = value.bytes();
    let mut decoded = Vec::new();
    while let Some(b) = bytes.next() {
        decoded.push(if b == b'%' {
            let high = bytes.next().and_then(|b| char::from(b).to_digit(16));
            let low = bytes.next().and_then(|b| char::from(b).to_digit(16));
            match (high, low) {
                (Some(high), Some(low)) => (high * 16 + low) as u8,
                _ => return Err("Bad pairing code escape".into()),
            }
        } else { b });
    }
    String::from_utf8(decoded).map_err(|_| "Bad pairing code text".into())
}

#[cfg(test)]
mod pair_tests {
    use super::SyncConfig;

    #[test]
    fn pair_link_round_trips() {
        let link = SyncConfig::pair_link("http://100.101.102.103:4747", "K7QF-2MZD-9XPA-T4RH");
        let cfg = SyncConfig::from_pair_link(&link).unwrap();
        assert_eq!(cfg, SyncConfig::new("http://100.101.102.103:4747", "K7QF-2MZD-9XPA-T4RH").unwrap());
        assert!(SyncConfig::from_pair_link("https://example.com").is_err());
        assert!(SyncConfig::from_pair_link("patch://pair?hub=http://100.1.2.3:4747").is_err(), "token required");
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncStatus {
    /// No hub configured.
    Disabled,
    Connecting,
    Syncing,
    /// Up to date; millisecond timestamp of the last successful sync.
    Synced(u64),
    Offline(String),
    Unauthorized,
    ClockAhead,
}

impl SyncStatus {
    pub fn label(&self) -> String {
        match self {
            SyncStatus::Disabled => "Local only".into(),
            SyncStatus::Connecting => "Connecting…".into(),
            SyncStatus::Syncing => "Syncing…".into(),
            SyncStatus::Synced(_) => "Synced".into(),
            SyncStatus::Offline(e) => format!("Offline — {e}"),
            SyncStatus::Unauthorized => "Access revoked, pair again".into(),
            SyncStatus::ClockAhead => "This device's clock is ahead of the hub; fix the date and time to sync".into(),
        }
    }
}

pub enum SyncEvent {
    Status(SyncStatus),
    /// Records or sync bookkeeping changed; refresh and persist the doc.
    RemoteApplied,
    /// Save this credential in device preferences, never in the synced document.
    Registered(SyncConfig, String),
}

enum Msg {
    Wake,
    Stop,
}

struct Shared {
    doc: Arc<Mutex<Doc>>,
    cfg: Mutex<Option<SyncConfig>>,
    identity: Mutex<DeviceIdentity>,
    changed: Condvar,
    generation: AtomicU64,
    status: Mutex<SyncStatus>,
    stop: AtomicBool,
    on_event: Box<dyn Fn(SyncEvent) + Send + Sync>,
}

impl Shared {
    fn set_status(&self, generation: u64, s: SyncStatus) {
        let cfg = self.cfg.lock().unwrap();
        if self.stop.load(Ordering::SeqCst) || self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let mut cur = self.status.lock().unwrap();
        if *cur != s {
            *cur = s.clone();
            drop(cur);
            drop(cfg);
            (self.on_event)(SyncEvent::Status(s));
        }
    }

    fn pause(&self, duration: Duration, generation: u64) {
        let cfg = self.cfg.lock().unwrap();
        let _ = self.changed.wait_timeout_while(cfg, duration, |_| {
            !self.stop.load(Ordering::SeqCst) && self.generation.load(Ordering::SeqCst) == generation
        }).unwrap();
    }

    fn config(&self) -> (Option<SyncConfig>, u64) {
        let cfg = self.cfg.lock().unwrap();
        (cfg.clone(), self.generation.load(Ordering::SeqCst))
    }
}

fn retry_delay(backoff: &mut Duration) -> Duration {
    let ceiling = *backoff;
    *backoff = (*backoff * 2).min(Duration::from_secs(30));
    let random = patch_core::id::random_string(8).bytes().fold(0u64, |n, b| n * 33 + u64::from(b));
    Duration::from_millis(ceiling.as_millis() as u64 / 2 + random % (ceiling.as_millis() as u64 / 2 + 1))
}

pub struct SyncClient {
    shared: Arc<Shared>,
    tx: Sender<Msg>,
}

enum Outcome {
    Done { applied: bool, more: bool },
    Cancelled,
}

fn sync_once(shared: &Shared, cfg: &SyncConfig, generation: u64) -> Result<Outcome, HttpError> {
    if shared.stop.load(Ordering::SeqCst) || shared.generation.load(Ordering::SeqCst) != generation {
        return Ok(Outcome::Cancelled);
    }
    let (since, pending, node) = {
        let d = shared.doc.lock().unwrap();
        (d.last_seq, d.pending(), d.node().to_string())
    };
    let mut identity = shared.identity.lock().unwrap().clone();
    identity.pairing = cfg.pairing;
    let body = serde_json::to_vec(&SyncRequest { device: Some(identity), since, changes: pending.clone(), node: Some(node) })
        .map_err(|e| HttpError::Protocol(e.to_string()))?;
    if shared.stop.load(Ordering::SeqCst) || shared.generation.load(Ordering::SeqCst) != generation {
        return Ok(Outcome::Cancelled);
    }
    let resp = request(&cfg.base, "POST", "/api/sync", &cfg.token, Some(&body), Duration::from_secs(30))?;
    let resp: SyncResponse = serde_json::from_slice(&resp.body).map_err(|e| HttpError::Protocol(e.to_string()))?;
    if resp.hub.trim().is_empty() {
        return Err(HttpError::Protocol("missing hub identity".into()));
    }
    let max_wall = patch_core::hlc::now_ms().saturating_add(patch_core::hlc::MAX_FUTURE_SKEW_MS);
    for field in resp.changes.iter().flat_map(|r| r.f.values()) {
        let wall = crate::proto::stamp_wall(&field.t)
            .ok_or_else(|| HttpError::Protocol("invalid change stamp".into()))?;
        // The local clock refuses to observe these; merging them would block later local edits.
        if wall > max_wall {
            return Err(HttpError::Protocol("hub changes are ahead of this device; check the date and time".into()));
        }
    }

    let mut current_cfg = shared.cfg.lock().unwrap();
    let mut d = shared.doc.lock().unwrap();
    if shared.stop.load(Ordering::SeqCst) || shared.generation.load(Ordering::SeqCst) != generation {
        return Ok(Outcome::Cancelled);
    }
    let rev = d.rev();
    // A restored backup can retain the same hub id with an older sequence.
    if d.hub_id.as_deref() == Some(&resp.hub) && (resp.reset || resp.seq < since) {
        d.hub_id = None;
    }
    let adopted = d.adopt_hub(&resp.hub);
    d.apply_remote(&resp.changes);
    if !adopted {
        d.ack(&pending);
        d.advance_seq(resp.seq);
    }
    let outcome = Outcome::Done { applied: d.rev() != rev, more: adopted || d.has_pending() };
    let registered = resp.credential.filter(|token| token.starts_with("device-") && token.len() <= 256 && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .map(|token| { let mut cfg = cfg.clone(); cfg.token = token; cfg.pairing = false; cfg });
    if let Some(cfg) = &registered { *current_cfg = Some(cfg.clone()); }
    drop(d);
    drop(current_cfg);
    if let Some(cfg) = registered {
        let name = resp.name.filter(|name| !name.trim().is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)).unwrap_or_else(|| cfg.base.host.clone());
        (shared.on_event)(SyncEvent::Registered(cfg, name));
    }
    Ok(outcome)
}

impl SyncClient {
    pub fn set_identity(&self, name: String, platform: String) {
        *self.shared.identity.lock().unwrap() = DeviceIdentity { name, platform, pairing: false };
    }

    pub fn start(
        doc: Arc<Mutex<Doc>>,
        cfg: Option<SyncConfig>,
        on_event: impl Fn(SyncEvent) + Send + Sync + 'static,
    ) -> SyncClient {
        let (tx, rx) = channel::<Msg>();
        let shared = Arc::new(Shared {
            doc,
            status: Mutex::new(if cfg.is_some() { SyncStatus::Connecting } else { SyncStatus::Disabled }),
            cfg: Mutex::new(cfg),
            identity: Mutex::new(DeviceIdentity { pairing: false, name: std::env::var("HOSTNAME").unwrap_or_else(|_| "Unknown device".into()), platform: std::env::consts::OS.into() }),
            changed: Condvar::new(),
            generation: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            on_event: Box::new(on_event),
        });

        // Worker: pushes and pulls.
        {
            let shared = shared.clone();
            thread::Builder::new()
                .name("patch-sync".into())
                .spawn(move || {
                    let mut wait = Duration::ZERO;
                    let mut backoff = Duration::from_secs(1);
                    let mut last_generation = 0;
                    loop {
                        match rx.recv_timeout(wait) {
                            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                            Ok(Msg::Wake) | Err(RecvTimeoutError::Timeout) => {}
                        }
                        // Coalesce bursts of edits into one request.
                        thread::sleep(Duration::from_millis(120));
                        while let Ok(m) = rx.try_recv() {
                            if matches!(m, Msg::Stop) {
                                return;
                            }
                        }
                        let (cfg, generation) = shared.config();
                        if generation != last_generation {
                            backoff = Duration::from_secs(1);
                            last_generation = generation;
                        }
                        let Some(cfg) = cfg else {
                            shared.set_status(generation, SyncStatus::Disabled);
                            wait = Duration::from_secs(3600);
                            continue;
                        };
                        shared.set_status(generation, SyncStatus::Syncing);
                        let mut rounds = 0;
                        let result = loop {
                            rounds += 1;
                            let cfg = shared.config().0.unwrap_or_else(|| cfg.clone());
                            match sync_once(&shared, &cfg, generation) {
                                Ok(Outcome::Cancelled) => break Ok(false),
                                Ok(Outcome::Done { applied, more }) => {
                                    if applied {
                                        (shared.on_event)(SyncEvent::RemoteApplied);
                                    }
                                    let more = more || shared.doc.lock().unwrap().has_pending();
                                    if more && rounds < 4 {
                                        continue;
                                    }
                                    break Ok(more);
                                }
                                Err(e) => break Err(e),
                            }
                        };
                        if shared.stop.load(Ordering::SeqCst) {
                            break;
                        }
                        if shared.generation.load(Ordering::SeqCst) != generation {
                            wait = Duration::ZERO;
                            continue;
                        }
                        match result {
                            Ok(more) => {
                                let more = more || shared.doc.lock().unwrap().has_pending();
                                backoff = Duration::from_secs(1);
                                if more {
                                    wait = Duration::ZERO;
                                } else {
                                    shared.set_status(generation, SyncStatus::Synced(patch_core::hlc::now_ms()));
                                    wait = Duration::from_secs(30);
                                }
                            }
                            Err(HttpError::Status(401, _)) => {
                                shared.set_status(generation, SyncStatus::Unauthorized);
                                wait = Duration::from_secs(30);
                            }
                            Err(HttpError::Status(409, _)) => {
                                shared.set_status(generation, SyncStatus::ClockAhead);
                                wait = Duration::from_secs(60);
                            }
                            Err(e) => {
                                shared.set_status(generation, SyncStatus::Offline(e.to_string()));
                                wait = retry_delay(&mut backoff);
                            }
                        }
                    }
                })
                .expect("spawn sync worker");
        }

        // Watcher: long-polls the hub and wakes the worker on remote changes.
        {
            let shared = shared.clone();
            let tx = tx.clone();
            thread::Builder::new()
                .name("patch-sync-watch".into())
                .spawn(move || {
                    let mut backoff = Duration::from_secs(1);
                    let mut last_generation = 0;
                    while !shared.stop.load(Ordering::SeqCst) {
                        let (cfg, generation) = shared.config();
                        if generation != last_generation {
                            backoff = Duration::from_secs(1);
                            last_generation = generation;
                        }
                        let Some(cfg) = cfg else {
                            shared.pause(Duration::from_secs(30), generation);
                            continue;
                        };
                        let (since, hub) = {
                            let d = shared.doc.lock().unwrap();
                            (d.last_seq, d.hub_id.clone())
                        };
                        if hub.is_none() || !matches!(*shared.status.lock().unwrap(), SyncStatus::Synced(_)) {
                            // Only the worker retries failures, so polling cannot defeat its backoff.
                            shared.pause(Duration::from_millis(250), generation);
                            continue;
                        }
                        let path = format!("/api/wait?since={since}");
                        if shared.stop.load(Ordering::SeqCst) || shared.generation.load(Ordering::SeqCst) != generation {
                            continue;
                        }
                        let result = request(&cfg.base, "GET", &path, &cfg.token, None, Duration::from_secs(35))
                            .and_then(|r| serde_json::from_slice::<WaitResponse>(&r.body)
                                .map_err(|e| HttpError::Protocol(e.to_string())));
                        if shared.stop.load(Ordering::SeqCst) || shared.generation.load(Ordering::SeqCst) != generation {
                            continue;
                        }
                        match result {
                            Ok(w) => {
                                backoff = Duration::from_secs(1);
                                let d = shared.doc.lock().unwrap();
                                if w.seq != d.last_seq || Some(&w.hub) != d.hub_id.as_ref() {
                                    let _ = tx.send(Msg::Wake);
                                    drop(d);
                                    // A changed hub returns immediately until the worker has pulled it.
                                    shared.pause(Duration::from_millis(250), generation);
                                }
                            }
                            Err(e) => {
                                // Verify through /sync; a broken watcher must not leave "Synced" forever.
                                shared.set_status(generation, match e {
                                    HttpError::Status(401, _) => SyncStatus::Unauthorized,
                                    _ => SyncStatus::Offline(e.to_string()),
                                });
                                let _ = tx.send(Msg::Wake);
                                shared.pause(retry_delay(&mut backoff), generation);
                            }
                        }
                    }
                })
                .expect("spawn sync watcher");
        }

        SyncClient { shared, tx }
    }

    pub fn set_config(&self, cfg: Option<SyncConfig>) {
        let mut cur = self.shared.cfg.lock().unwrap();
        if *cur == cfg {
            drop(cur);
            self.sync_now();
            return;
        }
        *cur = cfg;
        let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let enabled = cur.is_some();
        drop(cur);
        self.shared.changed.notify_all();
        self.shared.set_status(generation, if enabled { SyncStatus::Connecting } else { SyncStatus::Disabled });
        let _ = self.tx.send(Msg::Wake);
    }

    pub fn config(&self) -> Option<SyncConfig> {
        self.shared.cfg.lock().unwrap().clone()
    }

    /// Call after every local edit.
    pub fn notify_local_change(&self) {
        let (cfg, generation) = self.shared.config();
        if cfg.is_some() && matches!(self.status(), SyncStatus::Synced(_)) {
            self.shared.set_status(generation, SyncStatus::Syncing);
        }
        let _ = self.tx.send(Msg::Wake);
    }

    pub fn sync_now(&self) {
        let _ = self.tx.send(Msg::Wake);
    }

    pub fn status(&self) -> SyncStatus {
        self.shared.status.lock().unwrap().clone()
    }
}

impl Drop for SyncClient {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.changed.notify_all();
        let _ = self.tx.send(Msg::Stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patch_core::doc::{COL_TODO, DEFAULT_LIST};
    use patch_core::Task;
    use tiny_http::{Response, Server};

    fn server() -> (Server, SyncConfig) {
        let server = Server::http("127.0.0.1:0").unwrap();
        let cfg = SyncConfig::new(&server.server_addr().to_string(), "TOKEN").unwrap();
        (server, cfg)
    }

    fn doc() -> (Arc<Mutex<Doc>>, String) {
        let mut d = Doc::with_node("local");
        d.adopt_hub("hub");
        let mut task = Task::blank("", DEFAULT_LIST, COL_TODO);
        task.title = "Local".into();
        let id = d.add_task(task);
        (Arc::new(Mutex::new(d)), id)
    }

    // Next request to /api/*. Other local processes (dev tools) probe ephemeral loopback ports, so strays get a 404.
    fn recv_api(server: &Server) -> tiny_http::Request {
        loop {
            let request = server.recv_timeout(Duration::from_secs(4)).unwrap().expect("sync request");
            if request.url().starts_with("/api/") { return request; }
            request.respond(Response::from_string("").with_status_code(404)).unwrap();
        }
    }

    fn receive(server: &Server) -> (tiny_http::Request, SyncRequest) {
        let mut request = recv_api(server);
        assert_eq!(request.url(), "/api/sync");
        let body = serde_json::from_reader(request.as_reader()).unwrap();
        (request, body)
    }

    fn wait_status(events: &std::sync::mpsc::Receiver<SyncEvent>, predicate: impl Fn(&SyncStatus) -> bool) {
        loop {
            if let SyncEvent::Status(status) = events.recv_timeout(Duration::from_secs(4)).expect("status event") {
                if predicate(&status) { return; }
            }
        }
    }

    #[test]
    fn retry_delay_has_exponential_ceiling_jitter_and_cap() {
        let mut backoff = Duration::from_secs(1);
        for expected in [1, 2, 4, 8, 16, 30, 30] {
            assert_eq!(backoff, Duration::from_secs(expected));
            let delay = retry_delay(&mut backoff);
            assert!(delay >= Duration::from_millis(expected * 500));
            assert!(delay <= Duration::from_secs(expected));
        }
        let samples: std::collections::HashSet<_> = (0..16).map(|_| retry_delay(&mut backoff)).collect();
        assert!(samples.len() > 1);
    }

    #[test]
    fn pairing_escapes_values_and_accepts_legacy_links() {
        let token = "A&B%20# C";
        let url = "http://[fd7a:115c:a1e0::1]:4747";
        let link = SyncConfig::pair_link(url, token);
        assert_eq!(SyncConfig::from_pair_link(&link).unwrap(), SyncConfig::new(url, token).unwrap());
        assert!(SyncConfig::from_pair_link("patch://pair?hub=laptop&token=%GG").is_err());
        assert!(SyncConfig::from_pair_link("patch://pair?hub=laptop&token=%FF").is_err());
        assert!(SyncConfig::new("laptop", "a\r\nX: injected").is_err());
        assert_eq!(SyncConfig::from_pair_link("patch://pair?hub=laptop&token=OLD-TOKEN").unwrap().token, "OLD-TOKEN");
    }

    #[test]
    fn continuous_inflight_edits_are_drained_without_false_synced_status() {
        let (server, cfg) = server();
        let (doc, id) = doc();
        let (tx, rx) = channel();
        let client = SyncClient::start(doc.clone(), Some(cfg), move |e| { let _ = tx.send(e); });
        for seq in 1..=5 {
            let (request, sent) = receive(&server);
            if seq < 5 {
                let mut d = doc.lock().unwrap();
                let mut task = d.task(&id).unwrap();
                task.title = format!("Edit {seq}");
                d.save_task(&task);
            }
            let response = SyncResponse { name: None, credential: None, reset: false, hub: "hub".into(), seq, changes: sent.changes };
            request.respond(Response::from_string(serde_json::to_string(&response).unwrap())).unwrap();
            if seq < 5 {
                assert!(!matches!(client.status(), SyncStatus::Synced(_)));
            }
        }
        wait_status(&rx, |s| matches!(s, SyncStatus::Synced(_)));
        assert!(!doc.lock().unwrap().has_pending());
        assert_eq!(doc.lock().unwrap().task(&id).unwrap().title, "Edit 4");
    }

    #[test]
    fn edits_during_apply_events_are_not_reported_synced_or_left_pending() {
        let (server, cfg) = server();
        let (doc, id) = doc();
        let callback_doc = doc.clone();
        let callback_id = id.clone();
        let edited = AtomicBool::new(false);
        let (tx, rx) = channel();
        let client = SyncClient::start(doc.clone(), Some(cfg), move |event| {
            if matches!(&event, SyncEvent::RemoteApplied) && !edited.swap(true, Ordering::SeqCst) {
                let mut d = callback_doc.lock().unwrap();
                let mut task = d.task(&callback_id).unwrap();
                task.notes = "Edit after the response was applied".into();
                d.save_task(&task);
            }
            let _ = tx.send(event);
        });
        let (request, _) = receive(&server);
        request.respond(Response::from_string(r#"{"hub":"hub","seq":1,"changes":[]}"#)).unwrap();
        let (request, sent) = receive(&server);
        assert!(sent.changes.iter().any(|r| r.id == id && r.str("notes") == Some("Edit after the response was applied")));
        assert!(!matches!(client.status(), SyncStatus::Synced(_)));
        request.respond(Response::from_string(r#"{"hub":"hub","seq":2,"changes":[]}"#)).unwrap();
        wait_status(&rx, |s| matches!(s, SyncStatus::Synced(_)));
        assert!(!doc.lock().unwrap().has_pending());
    }

    #[test]
    fn notifying_a_local_edit_immediately_clears_synced_status() {
        let (server, cfg) = server();
        let (doc, _) = doc();
        let shared = Arc::new(shared(doc));
        *shared.cfg.lock().unwrap() = Some(cfg);
        *shared.status.lock().unwrap() = SyncStatus::Synced(1);
        let (tx, _rx) = channel();
        let client = SyncClient { shared, tx };
        client.notify_local_change();
        assert_eq!(client.status(), SyncStatus::Syncing);
        drop(server);
    }

    #[test]
    fn first_contact_notifies_before_a_failed_followup() {
        let (server, cfg) = server();
        let (source, id) = doc();
        let doc = Arc::new(Mutex::new(Doc::with_node("receiver")));
        let (tx, rx) = channel();
        let client = SyncClient::start(doc.clone(), Some(cfg), move |e| { let _ = tx.send(e); });
        let (request, _) = receive(&server);
        let response = SyncResponse { name: None, credential: None, reset: false, hub: "hub".into(), seq: 1, changes: source.lock().unwrap().pending() };
        request.respond(Response::from_string(serde_json::to_string(&response).unwrap())).unwrap();
        let (request, _) = receive(&server);
        request.respond(Response::from_string("disk failed").with_status_code(500)).unwrap();
        let mut applied = false;
        loop {
            match rx.recv_timeout(Duration::from_secs(4)).unwrap() {
                SyncEvent::RemoteApplied => applied = true,
                SyncEvent::Status(SyncStatus::Offline(_)) => break,
                _ => {},
            }
        }
        assert!(applied, "first merge must be persisted even when the next request fails");
        assert!(doc.lock().unwrap().task(&id).is_some());
        drop(client);
    }

    #[test]
    fn retries_connection_loss_without_another_local_edit() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = SyncConfig::new(&listener.local_addr().unwrap().to_string(), "TOKEN").unwrap();
        let (doc, id) = doc();
        let (tx, rx) = channel();
        let (ready_tx, ready_rx) = channel();
        let ready_rx = Mutex::new(ready_rx);
        let client = SyncClient::start(doc.clone(), Some(cfg), move |e| {
            let offline = matches!(e, SyncEvent::Status(SyncStatus::Offline(_)));
            let _ = tx.send(e);
            if offline { let _ = ready_rx.lock().unwrap().recv(); }
        });
        let (stream, _) = listener.accept().unwrap();
        drop(stream);
        wait_status(&rx, |s| matches!(s, SyncStatus::Offline(_)));
        // Release the worker only after the replacement server is ready.
        let server = Server::from_listener(listener, None).unwrap();
        ready_tx.send(()).unwrap();
        wait_status(&rx, |s| matches!(s, SyncStatus::Syncing));
        let (request, sent) = receive(&server);
        assert!(sent.changes.iter().any(|r| r.id == id));
        request.respond(Response::from_string(r#"{"hub":"hub","seq":1,"changes":[]}"#)).unwrap();
        wait_status(&rx, |s| matches!(s, SyncStatus::Synced(_)));
        assert!(!doc.lock().unwrap().has_pending());
        drop(client);
    }

    #[test]
    fn malformed_long_poll_is_surfaced_and_rechecked() {
        let (server, cfg) = server();
        let (doc, _) = doc();
        let (tx, rx) = channel();
        let client = SyncClient::start(doc, Some(cfg), move |e| { let _ = tx.send(e); });
        let (request, _) = receive(&server);
        request.respond(Response::from_string(r#"{"hub":"hub","seq":1,"changes":[]}"#)).unwrap();
        wait_status(&rx, |s| matches!(s, SyncStatus::Synced(_)));
        let request = recv_api(&server);
        assert!(request.url().starts_with("/api/wait?"));
        request.respond(Response::from_string("broken JSON")).unwrap();
        wait_status(&rx, |s| matches!(s, SyncStatus::Offline(e) if e.contains("bad response")));
        let (request, _) = receive(&server);
        request.respond(Response::from_string(r#"{"hub":"hub","seq":1,"changes":[]}"#)).unwrap();
        drop(client);
    }

    fn shared(doc: Arc<Mutex<Doc>>) -> Shared {
        Shared {
            identity: Mutex::new(DeviceIdentity::default()),
            doc, cfg: Mutex::new(None), changed: Condvar::new(), generation: AtomicU64::new(0),
            status: Mutex::new(SyncStatus::Connecting), stop: AtomicBool::new(false), on_event: Box::new(|_| {}),
        }
    }

    fn round(shared: &Shared, response: SyncResponse) -> (Result<Outcome, HttpError>, SyncRequest) {
        let (server, cfg) = server();
        let server = thread::spawn(move || {
            let (request, sent) = receive(&server);
            request.respond(Response::from_string(serde_json::to_string(&response).unwrap())).unwrap();
            sent
        });
        let result = sync_once(shared, &cfg, 0);
        (result, server.join().unwrap())
    }

    #[test]
    fn restored_hub_republishes_acknowledged_records() {
        let (doc, id) = doc();
        {
            let mut d = doc.lock().unwrap();
            let pending = d.pending();
            d.ack(&pending);
            d.advance_seq(99);
        }
        let shared = shared(doc.clone());
        for (seq, reset) in [(0, false), (100, true)] {
            doc.lock().unwrap().advance_seq(99);
            let (result, sent) = round(&shared, SyncResponse { name: None, credential: None, reset, hub: "hub".into(), seq, changes: vec![] });
            assert_eq!(sent.since, 99);
            assert!(matches!(result, Ok(Outcome::Done { applied: true, more: true })));
            assert_eq!(doc.lock().unwrap().last_seq, 0);
        }
        let (result, sent) = round(&shared, SyncResponse { name: None, credential: None, reset: false, hub: "hub".into(), seq: 1, changes: vec![] });
        assert_eq!(sent.since, 0);
        assert!(sent.changes.iter().any(|r| r.id == id));
        assert!(matches!(result, Ok(Outcome::Done { more: false, .. })));
    }

    #[test]
    fn invalid_response_cannot_acknowledge_or_modify_data() {
        let (doc, id) = doc();
        let before = doc.lock().unwrap().to_json();
        let shared = shared(doc.clone());
        let mut bad = patch_core::Record::new(&id, "task");
        bad.put("title", serde_json::json!("poison"), &patch_core::Stamp("z".into()));
        let mut future = patch_core::Record::new(&id, "task");
        future.put("title", serde_json::json!("future"), &patch_core::Stamp::new(
            patch_core::hlc::now_ms() + patch_core::hlc::MAX_FUTURE_SKEW_MS + 60_000, 0, "remote"));
        for (hub, changes) in [(String::new(), vec![]), ("hub".into(), vec![bad]), ("hub".into(), vec![future])] {
            let (result, _) = round(&shared, SyncResponse { name: None, credential: None, reset: false, hub, seq: 99, changes });
            assert!(matches!(result, Err(HttpError::Protocol(_))));
            assert_eq!(doc.lock().unwrap().to_json(), before);
        }
    }

    #[test]
    fn stale_config_cannot_overwrite_status() {
        let (doc, _) = doc();
        let shared = shared(doc);
        shared.generation.store(1, Ordering::SeqCst);
        shared.set_status(0, SyncStatus::Unauthorized);
        assert_eq!(*shared.status.lock().unwrap(), SyncStatus::Connecting);
    }
}
