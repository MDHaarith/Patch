//! GitHub release updates, staged beside the executable for atomic replacement.

use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(serde::Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(serde::Deserialize)]
struct Asset {
    name: String,
}

fn version(value: &str) -> Result<[u64; 3], String> {
    let parts = value.strip_prefix('v').unwrap_or(value).split('.');
    let parts = parts
        .map(|s| {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err("Invalid release version".to_string());
            }
            s.parse::<u64>()
                .map_err(|_| "Invalid release version".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    parts
        .try_into()
        .map_err(|_| "Invalid release version".into())
}

fn newer(tag: &str, current: &str) -> Result<bool, String> {
    Ok(version(tag)? > version(current)?)
}

fn curl(url: &str, destination: &Path, max_size: &str) -> Result<(), String> {
    let output = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "15",
            "--max-time",
            "300",
            "--max-filesize",
            max_size,
            "--user-agent",
            "Patch",
            "--output",
        ])
        .arg(destination)
        .arg(url)
        .output()
        .map_err(|e| format!("Cannot run curl: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Download failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Called on a worker thread. Never overwrites the running inode: Linux keeps
/// it alive until exit, and the next launch uses the replacement at this path.
pub fn check() -> Result<String, String> {
    let exe = crate::autostart::current_exe().map_err(|e| e.to_string())?;
    let parent = exe
        .parent()
        .ok_or("Cannot find the installed binary directory")?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let dir = parent.join(format!(".patch-update-{}-{nonce}", std::process::id()));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| {
            format!(
                "Cannot update {}: {e}. Use the installer for this location.",
                exe.display()
            )
        })?;
    let staging = Staging(dir);
    let metadata = staging.0.join("release.json");
    curl(
        "https://api.github.com/repos/MDHaarith/Patch/releases/latest",
        &metadata,
        "1048576",
    )?;
    let release: Release =
        serde_json::from_reader(File::open(metadata).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if !newer(&release.tag_name, env!("CARGO_PKG_VERSION"))? {
        return Ok("Up to date".into());
    }
    let name = format!("patch-linux-x86_64-{}.tar.gz", release.tag_name);
    if !release.assets.iter().any(|asset| asset.name == name) {
        return Err("No Linux x86_64 asset in the latest release".into());
    }
    let archive = staging.0.join("release.tar.gz");
    // Build the URL from the validated version, never from an arbitrary asset URL.
    curl(
        &format!(
            "https://github.com/MDHaarith/Patch/releases/download/{}/{name}",
            release.tag_name
        ),
        &archive,
        "268435456",
    )?;
    let binary = staging.0.join("patch");
    let mut file = File::create(&binary).map_err(|e| e.to_string())?;
    let output = Command::new("tar")
        .args(["-xzOf"])
        .arg(&archive)
        .arg("patch/patch")
        .stdout(Stdio::from(file.try_clone().map_err(|e| e.to_string())?))
        .output()
        .map_err(|e| format!("Cannot run tar: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Cannot unpack update: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // Reject an empty/corrupt archive member before replacing a working app.
    file = File::open(&binary).map_err(|e| e.to_string())?;
    let mut magic = [0; 4];
    file.read_exact(&mut magic).map_err(|e| e.to_string())?;
    if magic != *b"\x7fELF" {
        return Err("Update is not a Linux executable".into());
    }
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    replace(&binary, &exe).map_err(|e| format!("Cannot replace {}: {e}", exe.display()))?;
    Ok(format!(
        "Updated to {}. Quit and reopen Patch to finish.",
        release.tag_name
    ))
}

fn replace(binary: &Path, installed: &Path) -> io::Result<()> {
    fs::rename(binary, installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_release_versions_numerically() {
        assert!(newer("v0.1.14", "0.1.13").unwrap());
        assert!(newer("v0.10.0", "0.9.99").unwrap());
        assert!(newer("v1.0.0", "0.99.99").unwrap());
        assert!(!newer("v0.1.13", "0.1.13").unwrap());
        assert!(!newer("v0.1.12", "0.1.13").unwrap());
        for invalid in [
            "v0.1",
            "v0.1.14-rc1",
            "v0.1.14/other",
            "v0.1.14.0",
            "v+0.1.14",
        ] {
            assert!(newer(invalid, "0.1.13").is_err());
        }
    }

    #[test]
    fn replacement_keeps_the_old_inode_alive() {
        let dir = std::env::temp_dir().join(format!("patch-update-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let installed = dir.join("patch");
        let staged = dir.join("new");
        fs::write(&installed, "old").unwrap();
        let mut running = File::open(&installed).unwrap();
        fs::write(&staged, "new").unwrap();
        replace(&staged, &installed).unwrap();
        let mut old = String::new();
        running.read_to_string(&mut old).unwrap();
        assert_eq!(old, "old");
        assert_eq!(fs::read_to_string(&installed).unwrap(), "new");
        fs::remove_dir_all(dir).unwrap();
    }
}
