//! One Patch per data directory.
//!
//! The first process takes an exclusive lock on `<data>/patch.lock` and
//! listens on a Unix socket in the runtime directory. Later launches find the
//! lock taken, send their request ("show the window", "quit") over the socket
//! and exit. The lock, not the socket, decides who is first, so two launches
//! racing each other can't both become the primary, and a socket file left
//! behind by a crash is simply replaced.

use crate::paths::{ensure_private_dir, Paths};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A request from another launch of `patch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Show and raise the main window.
    Show,
    /// Save and exit.
    Quit,
    /// Only check that the instance is alive (`patch --background` while it
    /// already runs).
    Ping,
}

impl Request {
    pub fn as_str(self) -> &'static str {
        match self {
            Request::Show => "show",
            Request::Quit => "quit",
            Request::Ping => "ping",
        }
    }

    pub fn parse(s: &str) -> Option<Request> {
        match s.trim() {
            "show" => Some(Request::Show),
            "quit" => Some(Request::Quit),
            "ping" => Some(Request::Ping),
            _ => None,
        }
    }
}

/// Held by the running instance for its whole lifetime.
pub struct Primary {
    _lock: File,
    socket: PathBuf,
    listener: Option<UnixListener>,
}

/// Becomes the primary instance, or returns `None` when another process
/// already is.
pub fn acquire(paths: &Paths) -> io::Result<Option<Primary>> {
    std::fs::create_dir_all(&paths.data)?;
    let lock = OpenOptions::new().create(true).truncate(false).write(true).open(paths.lock_file())?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(e)) => return Err(e),
    }
    // Only the lock holder gets here, so whatever socket file exists is stale.
    let socket = paths.socket();
    if let Some(dir) = socket.parent() {
        ensure_private_dir(dir)?;
    }
    match std::fs::remove_file(&socket) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    Ok(Some(Primary { _lock: lock, socket, listener: Some(listener) }))
}

impl Primary {
    /// Answers requests from later launches on a background thread. `handle`
    /// runs on that thread.
    pub fn serve(&mut self, handle: impl Fn(Request) + Send + 'static) -> io::Result<()> {
        let Some(listener) = self.listener.take() else { return Ok(()) };
        std::thread::Builder::new().name("patch-instance".into()).spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                if let Some(req) = read_request(&stream) {
                    let mut out = &stream;
                    let _ = out.write_all(b"ok\n");
                    handle(req);
                }
            }
        })?;
        Ok(())
    }
}

impl Drop for Primary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn read_request(stream: &UnixStream) -> Option<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut line = String::new();
    BufReader::new(stream.take(64)).read_line(&mut line).ok()?;
    Request::parse(&line)
}

/// Delivers `req` to the running instance. Retries for a moment because the
/// lock is taken slightly before the socket starts listening.
pub fn send(socket: &Path, req: Request) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut stream = loop {
        match UnixStream::connect(socket) {
            Ok(s) => break s,
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(format!("{}\n", req.as_str()).as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream.take(64)).read_line(&mut reply)?;
    if reply.trim() == "ok" {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, format!("unexpected reply {reply:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn temp_paths(name: &str) -> Paths {
        let root = std::env::temp_dir().join(format!("patch-instance-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut p = Paths::from_env(Some(root.join("data")));
        p.runtime = root.join("run");
        p
    }

    #[test]
    fn second_launch_reaches_first() {
        let paths = temp_paths("reach");
        let mut primary = acquire(&paths).unwrap().expect("first launch is primary");
        assert!(acquire(&paths).unwrap().is_none(), "second launch must not become primary");

        let (tx, rx) = mpsc::channel();
        primary.serve(move |r| tx.send(r).unwrap()).unwrap();
        send(&paths.socket(), Request::Show).unwrap();
        send(&paths.socket(), Request::Quit).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Request::Show);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Request::Quit);

        drop(primary);
        assert!(!paths.socket().exists(), "socket is removed on exit");
        assert!(acquire(&paths).unwrap().is_some(), "lock is released on exit");
        let _ = std::fs::remove_dir_all(paths.data.parent().unwrap());
    }

    #[test]
    fn stale_socket_is_replaced() {
        let paths = temp_paths("stale");
        ensure_private_dir(paths.socket().parent().unwrap()).unwrap();
        std::fs::write(paths.socket(), b"left behind by a crash").unwrap();
        let mut primary = acquire(&paths).unwrap().expect("primary");
        let (tx, rx) = mpsc::channel();
        primary.serve(move |r| tx.send(r).unwrap()).unwrap();
        send(&paths.socket(), Request::Ping).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Request::Ping);
        drop(primary);
        let _ = std::fs::remove_dir_all(paths.data.parent().unwrap());
    }

    #[test]
    fn garbage_is_ignored() {
        assert_eq!(Request::parse("show\n"), Some(Request::Show));
        assert_eq!(Request::parse("rm -rf /"), None);
    }
}
