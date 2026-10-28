//! Headless Patch sync hub for an always-on tailnet machine (home server,
//! Raspberry Pi, NAS...).
//!
//!     patch-hub [--port 4747] [--bind 0.0.0.0] [--data DIR] [--token TOKEN]
//!               [--name NAME] [--allow-lan]

use patch_sync::{Hub, HubConfig, DEFAULT_PORT};
use std::path::PathBuf;

fn default_data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("patch-hub");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/share/patch-hub")
}

fn usage() -> ! {
    eprintln!(
        "patch-hub — sync hub for Patch over Tailscale\n\n\
         Options:\n  \
         --port N       listen port (default {DEFAULT_PORT})\n  \
         --bind ADDR    listen address (default 0.0.0.0; use your 100.x.y.z to bind to Tailscale only)\n  \
         --data DIR     data directory (default ~/.local/share/patch-hub)\n  \
         --token T      pairing token (default: generated and stored in DIR/token)\n  \
         --name NAME    display name\n  \
         --allow-lan    also accept devices on the local network, not only the tailnet"
    );
    std::process::exit(2)
}

fn listen_address(bind: &str, port: u16) -> String {
    match bind.parse::<std::net::IpAddr>() {
        Ok(ip) => std::net::SocketAddr::new(ip, port).to_string(),
        Err(_) => format!("{bind}:{port}"),
    }
}

fn main() {
    let mut port = DEFAULT_PORT;
    let mut bind = "0.0.0.0".to_string();
    let mut cfg = HubConfig::new(default_data_dir());
    cfg.name = std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_string()).unwrap_or_else(|_| "Patch hub".into());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--port" => port = val().parse().unwrap_or_else(|_| usage()),
            "--bind" => bind = val(),
            "--data" => cfg.data_dir = PathBuf::from(val()),
            "--token" => cfg.token = Some(val()),
            "--name" => cfg.name = val(),
            "--allow-lan" => cfg.allow_lan = true,
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    cfg.bind = listen_address(&bind, port);
    let hub = match Hub::start(cfg.clone()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("patch-hub: can't listen on {}: {e}", cfg.bind);
            std::process::exit(1);
        }
    };
    println!("Patch hub \"{}\" listening on {}", cfg.name, cfg.bind);
    println!("Data:   {}", cfg.data_dir.display());
    println!("Token:  {}", hub.token());
    let urls = hub.urls();
    if urls.is_empty() {
        println!("Tailscale doesn't seem to be running here — start it so your devices can reach this hub.");
    }
    for u in urls {
        println!("Pair:   {u}");
    }
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ipv6_bind_is_bracketed() {
        assert_eq!(super::listen_address("fd7a:115c:a1e0::1", 4747), "[fd7a:115c:a1e0::1]:4747");
        assert_eq!(super::listen_address("0.0.0.0", 4747), "0.0.0.0:4747");
    }
}
