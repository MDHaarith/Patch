//! Tailscale address helpers and the peer allow-list.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::process::Command;

/// Tailscale assigns IPv4 addresses from the CGNAT range 100.64.0.0/10.
pub fn is_tailscale_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..128).contains(&o[1])
}

/// Tailscale IPv6 addresses live in fd7a:115c:a1e0::/48.
pub fn is_tailscale(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_tailscale_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_tailscale_v4(v4);
            }
            let s = v6.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
    }
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4.is_private() || v4.is_link_local();
            }
            let first = v6.segments()[0];
            (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
    }
}

/// Whether the hub should talk to `ip` at all.
pub fn peer_allowed(ip: IpAddr, allow_lan: bool) -> bool {
    is_loopback(ip) || is_tailscale(ip) || (allow_lan && is_private(ip))
}

/// This machine's Tailscale IPv4, found by asking the kernel which source
/// address it would use to reach Tailscale's MagicDNS resolver. No packet
/// is sent.
pub fn tailscale_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("100.100.100.100:53").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if is_tailscale_v4(v4) => Some(v4),
        _ => None,
    }
}

/// MagicDNS name of this machine (e.g. `laptop.tail1234.ts.net`) when the
/// `tailscale` CLI is available.
pub fn tailscale_dns_name() -> Option<String> {
    let out = Command::new("tailscale").args(["status", "--json"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let name = v.get("Self")?.get("DNSName")?.as_str()?.trim_end_matches('.');
    (!name.is_empty()).then(|| name.to_string())
}

/// Addresses other tailnet devices can use to reach a hub on `port`.
pub fn hub_urls(port: u16) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(name) = tailscale_dns_name() {
        if let Some(short) = name.split('.').next() {
            urls.push(format!("http://{short}:{port}"));
        }
        urls.push(format!("http://{name}:{port}"));
    }
    if let Some(ip) = tailscale_ipv4() {
        urls.push(format!("http://{ip}:{port}"));
    }
    urls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_list() {
        assert!(peer_allowed("127.0.0.1".parse().unwrap(), false));
        assert!(peer_allowed("::1".parse().unwrap(), false));
        assert!(peer_allowed("100.101.102.103".parse().unwrap(), false));
        assert!(peer_allowed("::ffff:100.64.0.1".parse().unwrap(), false));
        assert!(peer_allowed("fd7a:115c:a1e0::1".parse().unwrap(), false));
        assert!(!peer_allowed("100.128.0.1".parse().unwrap(), false));
        assert!(!peer_allowed("192.168.1.20".parse().unwrap(), false));
        assert!(peer_allowed("192.168.1.20".parse().unwrap(), true));
        assert!(!peer_allowed("8.8.8.8".parse().unwrap(), true));
    }
}
