//! Runs the sync hub inside the desktop app ("This computer is the hub").

use patch_sync::http::{request, Base};
use patch_sync::{Hub, HubConfig, DEFAULT_PORT};
use std::io;
use std::path::Path;
use std::time::Duration;

/// Ports tried after the default one, so a busy 4747 still gives a port that
/// stays the same across restarts (paired devices keep working). An
/// OS-assigned port is the last resort.
const FALLBACK_PORTS: std::ops::RangeInclusive<u16> = 4748..=4757;

pub fn hostname() -> String {
    ["/proc/sys/kernel/hostname", "/etc/hostname"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "Patch hub".into())
}

/// Whether a Patch hub (e.g. the headless `patch-hub` service) already
/// answers on `port` of this machine. `/api/ping` needs no token.
pub fn patch_hub_running(port: u16) -> bool {
    let base = Base { host: "127.0.0.1".into(), port, prefix: String::new() };
    request(&base, "GET", "/api/ping", "", None, Duration::from_millis(800))
        .ok()
        .filter(|r| r.status == 200)
        .and_then(|r| serde_json::from_slice::<serde_json::Value>(&r.body).ok())
        .is_some_and(|v| v.get("ok").and_then(|o| o.as_bool()) == Some(true) && v.get("hub").is_some())
}

/// The port the hub used last time, so devices paired with a fallback port
/// keep reaching it after a restart.
pub fn remembered_port(dir: &Path) -> u16 {
    std::fs::read_to_string(dir.join("port"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|p| *p != 0)
        .unwrap_or(DEFAULT_PORT)
}

/// Starts a hub storing its state in `dir`, listening on all interfaces
/// (the hub itself refuses peers outside the tailnet).
pub fn start(dir: &Path, port: u16) -> Result<Hub, String> {
    let mut cfg = HubConfig::new(dir);
    cfg.name = hostname();
    let candidates = std::iter::once(port).chain(FALLBACK_PORTS.filter(|p| *p != port)).chain(std::iter::once(0));
    let mut first_error = None;
    for candidate in candidates {
        cfg.bind = format!("0.0.0.0:{candidate}");
        match Hub::start(cfg.clone()) {
            Ok(hub) => {
                if candidate != port {
                    eprintln!("patch: port {port} is busy, the sync hub listens on {} instead", hub.port());
                }
                let _ = patch_core::doc::write_atomic(&dir.join("port"), hub.port().to_string().as_bytes());
                return Ok(hub);
            }
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                if candidate == DEFAULT_PORT && patch_hub_running(DEFAULT_PORT) {
                    return Err(format!(
                        "A Patch hub is already running on this computer (port {DEFAULT_PORT}). \
                         Connect to it instead: http://127.0.0.1:{DEFAULT_PORT} with the token it printed."
                    ));
                }
                first_error.get_or_insert(e);
            }
            Err(e) => return Err(format!("Can't start the hub: {e}")),
        }
    }
    Err(format!("Can't start the hub: {}", first_error.map(|e| e.to_string()).unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_is_never_empty() {
        assert!(!hostname().is_empty());
    }
}
