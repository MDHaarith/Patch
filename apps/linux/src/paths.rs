//! Where the desktop app keeps its files, following the XDG base directory
//! specification.

use std::io;
use std::path::{Path, PathBuf};

/// Reverse-DNS application id: the `.desktop` file name, the Wayland
/// `app_id` / X11 `WM_CLASS`, and the icon name.
pub const APP_ID: &str = "com.mdhaarith.Patch";

#[derive(Clone, Debug)]
pub struct Paths {
    /// Document, preferences and the embedded hub (`$XDG_DATA_HOME/patch`).
    pub data: PathBuf,
    /// `$XDG_CONFIG_HOME`, for the autostart entry.
    pub config_home: PathBuf,
    /// `$XDG_CACHE_HOME/patch`.
    pub cache: PathBuf,
    /// Private per-user directory for the single-instance socket.
    pub runtime: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// The spec says relative paths in these variables are invalid and must be
/// ignored.
fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

impl Paths {
    /// Resolves every location from the environment. `data_override` replaces
    /// the data directory (`patch --data-dir`).
    pub fn from_env(data_override: Option<PathBuf>) -> Paths {
        let data = match data_override {
            Some(d) if d.is_absolute() => d,
            Some(d) => std::env::current_dir().map(|c| c.join(&d)).unwrap_or(d),
            None => xdg_dir("XDG_DATA_HOME", ".local/share").join("patch"),
        };
        Paths {
            data,
            config_home: xdg_dir("XDG_CONFIG_HOME", ".config"),
            cache: xdg_dir("XDG_CACHE_HOME", ".cache").join("patch"),
            runtime: runtime_dir(),
        }
    }

    pub fn prefs_file(&self) -> PathBuf {
        self.data.join("prefs.json")
    }

    /// Desktop-only state: widget position, window size, one-time hints.
    pub fn state_file(&self) -> PathBuf {
        self.data.join("desktop.json")
    }

    pub fn hub_dir(&self) -> PathBuf {
        self.data.join("hub")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.data.join("patch.lock")
    }

    /// One socket per data directory, so a second profile (`--data-dir`, a
    /// different `XDG_DATA_HOME`) runs side by side instead of talking to the
    /// first one.
    pub fn socket(&self) -> PathBuf {
        let name = format!("patch-{:016x}.sock", fnv1a(self.data.as_os_str().as_encoded_bytes()));
        let path = self.runtime.join(&name);
        if path.as_os_str().len() < SUN_PATH_MAX {
            path
        } else {
            private_tmp().join(name)
        }
    }

    pub fn autostart_file(&self) -> PathBuf {
        self.config_home.join("autostart").join(format!("{APP_ID}.desktop"))
    }
}

/// `sun_path` holds 108 bytes including the terminating NUL.
const SUN_PATH_MAX: usize = 108;

/// `$XDG_RUNTIME_DIR`, or a private directory in `/tmp` when the session
/// doesn't provide one (plain `ssh -X`, some containers).
fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(private_tmp)
}

fn private_tmp() -> PathBuf {
    // SAFETY: getuid has no preconditions and can't fail.
    let uid = unsafe { libc::getuid() };
    std::env::temp_dir().join(format!("patch-{uid}"))
}

/// Creates `dir` with mode 0700 and refuses to use it when it belongs to
/// someone else: anyone who can write there could impersonate the running
/// instance.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid has no preconditions and can't fail.
    let uid = unsafe { libc::getuid() };
    if !meta.is_dir() || meta.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} isn't a directory owned by this user", dir.display()),
        ));
    }
    if meta.mode() & 0o077 != 0 && dir.starts_with(std::env::temp_dir()) {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Stable across Rust versions, unlike `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_depends_on_data_dir() {
        let mut a = Paths::from_env(Some("/tmp/a".into()));
        let mut b = Paths::from_env(Some("/tmp/b".into()));
        a.runtime = "/run/user/1000".into();
        b.runtime = "/run/user/1000".into();
        assert_ne!(a.socket(), b.socket());
        assert_eq!(a.socket(), a.socket(), "stable for the same data dir");
        assert!(a.socket().starts_with("/run/user/1000"));
        // Unix socket paths are limited to 108 bytes.
        a.runtime = PathBuf::from("/").join("x".repeat(120));
        assert!(a.socket().as_os_str().len() < SUN_PATH_MAX);
        assert!(a.socket().starts_with(std::env::temp_dir()));
    }

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
