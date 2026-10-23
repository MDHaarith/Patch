//! Background sync client used by the apps.

use crate::http::{request, Base, HttpError};
use crate::proto::{SyncRequest, SyncResponse, WaitResponse};
use patch_core::Doc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncConfig {
    pub base: Base,
    pub token: String,
}

impl SyncConfig {
    pub fn new(url: &str, token: &str) -> Result<SyncConfig, String> {
        let token = token.trim();
        if token.is_empty() {
            return Err("Enter the pairing token shown on the hub".into());
        }
        Ok(SyncConfig { base: Base::parse(url)?, token: token.to_string() })
    }

    /// What a hub's pairing QR code encodes. Tailnet URLs and tokens need no escaping.
    pub fn pair_link(url: &str, token: &str) -> String {
        format!("patch://pair?hub={url}&token={token}")
    }

    /// Reads a scanned [`SyncConfig::pair_link`].
    pub fn from_pair_link(link: &str) -> Result<SyncConfig, String> {
        let query = link.trim().strip_prefix("patch://pair?").ok_or("That QR code isn't a Patch pairing code")?;
        let (mut hub, mut token) = ("", "");
        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("hub", v)) => hub = v,
                Some(("token", v)) => token = v,
                _ => {}
            }
        }
        SyncConfig::new(hub, token)
    }
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
}

impl SyncStatus {
    pub fn label(&self) -> String {
        match self {
            SyncStatus::Disabled => "Local only".into(),
            SyncStatus::Connecting => "Connecting…".into(),
            SyncStatus::Syncing => "Syncing…".into(),
            SyncStatus::Synced(_) => "Synced".into(),
            SyncStatus::Offline(e) => format!("Offline — {e}"),
            SyncStatus::Unauthorized => "Wrong pairing token".into(),
        }
    }
}

pub enum SyncEvent {
    Status(SyncStatus),
    /// Remote changes were merged into the doc.
    RemoteApplied,
}

enum Msg {
    Wake,
    Stop,
}

struct Shared {
    doc: Arc<Mutex<Doc>>,
    cfg: Mutex<Option<SyncConfig>>,
    generation: AtomicU64,
    status: Mutex<SyncStatus>,
    stop: AtomicBool,
    on_event: Box<dyn Fn(SyncEvent) + Send + Sync>,
}

impl Shared {
    fn set_status(&self, s: SyncStatus) {
        let mut cur = self.status.lock().unwrap();
        if *cur != s {
            *cur = s.clone();
            drop(cur);
            (self.on_event)(SyncEvent::Status(s));
        }
    }

    fn config(&self) -> (Option<SyncConfig>, u64) {
        (self.cfg.lock().unwrap().clone(), self.generation.load(Ordering::SeqCst))
    }
}

pub struct SyncClient {
    shared: Arc<Shared>,
    tx: Sender<Msg>,
}

enum Outcome {
    Done { applied: bool, more: bool },
    HubChanged,
}

fn sync_once(shared: &Shared, cfg: &SyncConfig) -> Result<Outcome, HttpError> {
    let (since, pending, node) = {
        let d = shared.doc.lock().unwrap();
        (d.last_seq, d.pending(), d.node().to_string())
    };
    let body = serde_json::to_vec(&SyncRequest { since, changes: pending.clone(), node: Some(node) })
        .map_err(|e| HttpError::Protocol(e.to_string()))?;
    let resp = request(&cfg.base, "POST", "/api/sync", &cfg.token, Some(&body), Duration::from_secs(30))?;
    let resp: SyncResponse = serde_json::from_slice(&resp.body).map_err(|e| HttpError::Protocol(e.to_string()))?;

    let mut d = shared.doc.lock().unwrap();
    let applied = d.apply_remote(&resp.changes);
    if d.adopt_hub(&resp.hub) {
        // First contact with this hub: push everything and pull from zero.
        return Ok(Outcome::HubChanged);
    }
    d.ack(&pending);
    d.last_seq = d.last_seq.max(resp.seq);
    Ok(Outcome::Done { applied, more: d.has_pending() })
}

impl SyncClient {
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
                        let (cfg, _) = shared.config();
                        let Some(cfg) = cfg else {
                            shared.set_status(SyncStatus::Disabled);
                            wait = Duration::from_secs(3600);
                            continue;
                        };
                        if !matches!(*shared.status.lock().unwrap(), SyncStatus::Synced(_)) {
                            shared.set_status(SyncStatus::Syncing);
                        }
                        let mut rounds = 0;
                        let result = loop {
                            rounds += 1;
                            match sync_once(&shared, &cfg) {
                                Ok(Outcome::HubChanged) if rounds < 4 => continue,
                                Ok(Outcome::HubChanged) => break Ok(true),
                                Ok(Outcome::Done { applied, more }) => {
                                    if applied {
                                        (shared.on_event)(SyncEvent::RemoteApplied);
                                    }
                                    if more && rounds < 4 {
                                        continue;
                                    }
                                    break Ok(applied);
                                }
                                Err(e) => break Err(e),
                            }
                        };
                        match result {
                            Ok(_) => {
                                if rounds > 1 {
                                    (shared.on_event)(SyncEvent::RemoteApplied);
                                }
                                shared.set_status(SyncStatus::Synced(patch_core::hlc::now_ms()));
                                backoff = Duration::from_secs(1);
                                wait = Duration::from_secs(60);
                            }
                            Err(HttpError::Status(401, _)) => {
                                shared.set_status(SyncStatus::Unauthorized);
                                wait = Duration::from_secs(30);
                            }
                            Err(e) => {
                                shared.set_status(SyncStatus::Offline(e.to_string()));
                                wait = backoff;
                                backoff = (backoff * 2).min(Duration::from_secs(30));
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
                    let mut backoff = Duration::from_secs(2);
                    while !shared.stop.load(Ordering::SeqCst) {
                        let (cfg, generation) = shared.config();
                        let Some(cfg) = cfg else {
                            thread::sleep(Duration::from_millis(500));
                            continue;
                        };
                        let (since, hub) = {
                            let d = shared.doc.lock().unwrap();
                            (d.last_seq, d.hub_id.clone())
                        };
                        if hub.is_none() {
                            // Wait for the worker's first sync.
                            thread::sleep(Duration::from_millis(500));
                            continue;
                        }
                        let path = format!("/api/wait?since={since}");
                        match request(&cfg.base, "GET", &path, &cfg.token, None, Duration::from_secs(40)) {
                            Ok(r) => {
                                backoff = Duration::from_secs(2);
                                if shared.generation.load(Ordering::SeqCst) != generation {
                                    continue;
                                }
                                if let Ok(w) = serde_json::from_slice::<WaitResponse>(&r.body) {
                                    if w.seq > since || Some(&w.hub) != hub.as_ref() {
                                        let _ = tx.send(Msg::Wake);
                                    }
                                }
                            }
                            Err(_) => {
                                let _ = tx.send(Msg::Wake);
                                thread::sleep(backoff);
                                backoff = (backoff * 2).min(Duration::from_secs(30));
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
        drop(cur);
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.shared.set_status(SyncStatus::Connecting);
        let _ = self.tx.send(Msg::Wake);
    }

    pub fn config(&self) -> Option<SyncConfig> {
        self.shared.cfg.lock().unwrap().clone()
    }

    /// Call after every local edit.
    pub fn notify_local_change(&self) {
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
        let _ = self.tx.send(Msg::Stop);
    }
}
