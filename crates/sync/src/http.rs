//! A deliberately tiny HTTP/1.1 client over `std::net`.
//!
//! The hub is only reachable inside the tailnet, where WireGuard already
//! encrypts traffic, so plain HTTP is enough and keeps the Android build
//! free of TLS and async dependencies.

use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

pub(crate) const MAX_BODY: usize = 64 * 1024 * 1024;
const MAX_HEAD: usize = 16 * 1024;

pub(crate) struct DeadlineStream {
    pub stream: TcpStream,
    pub deadline: Instant,
}

impl DeadlineStream {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline.checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "HTTP deadline exceeded"))
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(buf)
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

pub(crate) fn read_head(reader: &mut impl Read) -> io::Result<String> {
    let mut head = Vec::new();
    while head.len() < MAX_HEAD {
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            return String::from_utf8(head)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 HTTP header"));
        }
    }
    Err(io::Error::new(io::ErrorKind::InvalidData, "HTTP header too large"))
}

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
            if h.parse::<std::net::Ipv6Addr>().is_err() || (!after.is_empty() && !after.starts_with(':')) {
                return Err("Bad IPv6 address".into());
            }
            let port = after.strip_prefix(':').map(|p| p.parse::<u16>()).transpose().map_err(|_| "Bad port")?;
            (h.to_string(), port)
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) if !h.contains(':') => (h.to_string(), Some(p.parse::<u16>().map_err(|_| "Bad port")?)),
                _ => (hostport.to_string(), None),
            }
        };
        if host.is_empty() || (!host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            && host.parse::<std::net::Ipv6Addr>().is_err()) {
            return Err("Bad host name".into());
        }
        if port == Some(0) || prefix.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '?' | '#' | '\\')) {
            return Err("Bad hub address".into());
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

fn connect(base: &Base, deadline: Instant) -> Result<TcpStream, HttpError> {
    let addrs = if let Ok(ip) = base.host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, base.port)]
    } else {
        // The system resolver has no timeout API; don't let it hold the request past its deadline.
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let address = (base.host.clone(), base.port);
        std::thread::Builder::new().name("patch-dns".into()).spawn(move || {
            let _ = tx.send(address.to_socket_addrs().map(|addrs| addrs.collect::<Vec<_>>()));
        }).map_err(|e| HttpError::Connect(e.to_string()))?;
        rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| HttpError::Connect(format!("DNS: {e}")))?
            .map_err(|e| HttpError::Connect(format!("{}: {e}", base.host)))?
    };
    let mut last = String::from("no addresses");
    for addr in addrs {
        let timeout = deadline.checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| HttpError::Connect("HTTP deadline exceeded".into()))?;
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
    if !token.is_ascii() || token.bytes().any(|b| b.is_ascii_control()) {
        return Err(HttpError::Protocol("bad pairing token".into()));
    }
    let deadline = Instant::now() + timeout;
    let mut stream = DeadlineStream { stream: connect(base, deadline)?, deadline };
    let _ = stream.stream.set_nodelay(true);

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

    read_response(&mut BufReader::new(stream))
}

fn read_response(reader: &mut impl Read) -> Result<Response, HttpError> {
    let head = read_head(reader).map_err(|e| HttpError::Io(e.to_string()))?;
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
                if length.is_some() {
                    return Err(HttpError::Protocol("duplicate Content-Length".into()));
                }
                let n = v.parse::<usize>().map_err(|_| HttpError::Protocol("bad Content-Length".into()))?;
                if n > MAX_BODY {
                    return Err(HttpError::Protocol("response too large".into()));
                }
                length = Some(n);
            } else if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
    }
    let mut body = Vec::new();
    let limit = if chunked { MAX_BODY + 1 } else { length.unwrap_or(MAX_BODY + 1) };
    reader.take(limit as u64).read_to_end(&mut body).map_err(|e| HttpError::Io(e.to_string()))?;
    if body.len() > MAX_BODY {
        return Err(HttpError::Protocol("response too large".into()));
    }
    if chunked {
        body = dechunk(&body)?;
    } else if length.is_some_and(|n| body.len() != n) {
        return Err(HttpError::Protocol("truncated body".into()));
    }
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
            if data == b"\r\n" || data.ends_with(b"\r\n\r\n") {
                return Ok(out);
            }
            return Err(HttpError::Protocol("truncated chunk terminator".into()));
        }
        if size > MAX_BODY - out.len() {
            return Err(HttpError::Protocol("response too large".into()));
        }
        out.extend_from_slice(data.get(..size).ok_or_else(|| HttpError::Protocol("short chunk".into()))?);
        if data.get(size..size + 2) != Some(b"\r\n") {
            return Err(HttpError::Protocol("bad chunk terminator".into()));
        }
        data = &data[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_hub_addresses_are_rejected() {
        for address in ["[::1]ignored", "[not-ipv6]:4747", "host:0", "host?token=secret", "user@host",
            "host/path?query", "host/path#fragment", "host/\r\nX: injected", "ftp://host"] {
            assert!(Base::parse(address).is_err(), "accepted {address:?}");
        }
        for address in ["http://[fd7a:115c:a1e0::1]:4747/", "fd7a:115c:a1e0::1", "laptop.tail.ts.net:4747/prefix"] {
            let parsed = Base::parse(address).unwrap();
            assert_eq!(Base::parse(&parsed.url()).unwrap(), parsed);
        }
    }

    #[test]
    fn partial_chunk_framing_is_rejected() {
        assert_eq!(dechunk(b"2\r\nok\r\n0\r\n\r\n").unwrap(), b"ok");
        assert_eq!(dechunk(b"2\r\nok\r\n0\r\nX: trailer\r\n\r\n").unwrap(), b"ok");
        for data in [&b"2\r\nokXX0\r\n\r\n"[..], &b"2\r\nok\r\n0\r\n"[..], &b"2\r\nok"[..]] {
            assert!(dechunk(data).is_err());
        }
    }

    #[test]
    fn content_length_bounds_the_read() {
        let mut raw = io::Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nokextra");
        assert_eq!(read_response(&mut raw).unwrap().body, b"ok");
        assert_eq!(raw.position(), raw.get_ref().len() as u64 - 5);
        for length in ["67108865", "invalid", "-1"] {
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\n");
            assert!(matches!(read_response(&mut head.as_bytes()), Err(HttpError::Protocol(_))));
        }
        let mut truncated = &b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nok"[..];
        assert!(matches!(read_response(&mut truncated), Err(HttpError::Protocol(_))));
        assert!(read_head(&mut io::repeat(b'x')).is_err());
    }

    #[test]
    fn content_length_does_not_wait_for_eof() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Base::parse(&listener.local_addr().unwrap().to_string()).unwrap();
        let base = Base { host: "localhost".into(), ..base };
        let (tx, rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_head(&mut BufReader::new(&mut stream)).unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").unwrap();
            let _ = rx.recv_timeout(Duration::from_secs(3));
        });
        let response = request(&base, "GET", "/", "token", None, Duration::from_secs(1));
        tx.send(()).unwrap();
        server.join().unwrap();
        assert_eq!(response.unwrap().body, b"ok");
    }

    #[test]
    fn overall_deadline_stops_a_trickling_response() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Base::parse(&listener.local_addr().unwrap().to_string()).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_head(&mut BufReader::new(&mut stream)).unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n").unwrap();
            for _ in 0..20 {
                if stream.write_all(b"x").is_err() { break; }
                std::thread::sleep(Duration::from_millis(40));
            }
        });
        let started = Instant::now();
        let response = request(&base, "GET", "/", "token", None, Duration::from_millis(250));
        let elapsed = started.elapsed();
        server.join().unwrap();
        assert!(matches!(response, Err(HttpError::Io(_))));
        assert!(elapsed < Duration::from_millis(650), "deadline took {elapsed:?}");
    }

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
        assert_eq!(read_response(&mut io::Cursor::new(raw)).unwrap().body, b"Wikipedia");
        let raw = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 3\r\n\r\nbad";
        assert!(matches!(read_response(&mut io::Cursor::new(raw)), Err(HttpError::Status(401, _))));
    }
}
