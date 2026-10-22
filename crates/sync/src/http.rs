//! A deliberately tiny HTTP/1.1 client over `std::net`.
//!
//! The hub is only reachable inside the tailnet, where WireGuard already
//! encrypts traffic, so plain HTTP is enough and keeps the Android build
//! free of TLS and async dependencies.

use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base {
    pub host: String,
    pub port: u16,
    pub prefix: String,
}

impl Base {
    /// Accepts `laptop`, `laptop:4747`, `http://100.64.1.2:4747/` ...
    pub fn parse(input: &str) -> Result<Base, String> {
        let s = input.trim();
        if s.is_empty() {
            return Err("Enter the hub address".into());
        }
        if s.starts_with("https://") {
            return Err("Use http:// — Tailscale already encrypts the connection".into());
        }
        let s = s.strip_prefix("http://").unwrap_or(s);
        let (hostport, prefix) = match s.find('/') {
            Some(i) => (&s[..i], s[i..].trim_end_matches('/').to_string()),
            None => (s, String::new()),
        };
        let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
            // [v6]:port
            let (h, after) = rest.split_once(']').ok_or("Bad IPv6 address")?;
            let port = after.strip_prefix(':').map(|p| p.parse::<u16>()).transpose().map_err(|_| "Bad port")?;
            (h.to_string(), port)
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) if !h.contains(':') => (h.to_string(), Some(p.parse::<u16>().map_err(|_| "Bad port")?)),
                _ => (hostport.to_string(), None),
            }
        };
        if host.is_empty() || host.contains(char::is_whitespace) {
            return Err("Bad host name".into());
        }
        Ok(Base { host, port: port.unwrap_or(crate::DEFAULT_PORT), prefix })
    }

    pub fn url(&self) -> String {
        if self.host.contains(':') {
            format!("http://[{}]:{}{}", self.host, self.port, self.prefix)
        } else {
            format!("http://{}:{}{}", self.host, self.port, self.prefix)
        }
    }
}

#[derive(Debug)]
pub enum HttpError {
    Connect(String),
    Io(String),
    Status(u16, String),
    Protocol(String),
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::Connect(e) => write!(f, "can't reach hub: {e}"),
            HttpError::Io(e) => write!(f, "connection error: {e}"),
            HttpError::Status(code, body) => write!(f, "hub answered {code}: {body}"),
            HttpError::Protocol(e) => write!(f, "bad response: {e}"),
        }
    }
}

impl std::error::Error for HttpError {}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

fn connect(base: &Base, timeout: Duration) -> Result<TcpStream, HttpError> {
    let addrs: Vec<_> = (base.host.as_str(), base.port)
        .to_socket_addrs()
        .map_err(|e| HttpError::Connect(format!("{}: {e}", base.host)))?
        .collect();
    let mut last = String::from("no addresses");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(5).min(timeout)) {
            Ok(s) => return Ok(s),
            Err(e) => last = e.to_string(),
        }
    }
    Err(HttpError::Connect(last))
}

pub fn request(
    base: &Base,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<Response, HttpError> {
    let mut stream = connect(base, timeout)?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(Duration::from_secs(15))).ok();
    let _ = stream.set_nodelay(true);

    let body = body.unwrap_or_default();
    let host = if base.host.contains(':') { format!("[{}]", base.host) } else { base.host.clone() };
    let head = format!(
        "{method} {}{path} HTTP/1.1\r\nHost: {host}:{}\r\nAuthorization: Bearer {token}\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nUser-Agent: patch/{}\r\n\r\n",
        base.prefix,
        base.port,
        body.len(),
        env!("CARGO_PKG_VERSION"),
    );
    stream.write_all(head.as_bytes()).map_err(|e| HttpError::Io(e.to_string()))?;
    stream.write_all(body).map_err(|e| HttpError::Io(e.to_string()))?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| HttpError::Io(e.to_string()))?;
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Result<Response, HttpError> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| HttpError::Protocol("no header terminator".into()))?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| HttpError::Protocol(format!("bad status line {status_line:?}")))?;
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "content-length" {
                length = v.parse::<usize>().ok();
            } else if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
    }
    let rest = &raw[split + 4..];
    let body = if chunked {
        dechunk(rest)?
    } else if let Some(n) = length {
        rest.get(..n).ok_or_else(|| HttpError::Protocol("truncated body".into()))?.to_vec()
    } else {
        rest.to_vec()
    };
    if !(200..300).contains(&status) {
        return Err(HttpError::Status(status, String::from_utf8_lossy(&body).chars().take(200).collect()));
    }
    Ok(Response { status, body })
}

fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, HttpError> {
    let mut out = Vec::new();
    loop {
        let line_end = data
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| HttpError::Protocol("bad chunk".into()))?;
        let size_str = String::from_utf8_lossy(&data[..line_end]);
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| HttpError::Protocol("bad chunk size".into()))?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        out.extend_from_slice(data.get(..size).ok_or_else(|| HttpError::Protocol("short chunk".into()))?);
        data = data.get(size + 2..).unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bases() {
        assert_eq!(Base::parse("laptop").unwrap().url(), "http://laptop:4747");
        assert_eq!(Base::parse("http://100.64.0.2:9000/").unwrap().url(), "http://100.64.0.2:9000");
        assert_eq!(Base::parse("laptop.tail1.ts.net:80/patch").unwrap().url(), "http://laptop.tail1.ts.net:80/patch");
        assert_eq!(Base::parse("[fd7a:115c:a1e0::1]:4747").unwrap().host, "fd7a:115c:a1e0::1");
        assert!(Base::parse("https://x").is_err());
        assert!(Base::parse("").is_err());
    }

    #[test]
    fn parses_chunked() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw).unwrap().body, b"Wikipedia");
        let raw = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 3\r\n\r\nbad";
        assert!(matches!(parse_response(raw), Err(HttpError::Status(401, _))));
    }
}
