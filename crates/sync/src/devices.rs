//! Durable device credentials. Public device rows never contain credentials.
use patch_core::{doc::write_atomic, hlc::now_ms, id::random_string};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub paired_at: u64,
    pub last_seen: u64,
    pub address: String,
    pub revoked: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeviceIdentity {
    #[serde(default)]
    pub pairing: bool,
    pub name: String,
    pub platform: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    #[serde(flatten)]
    device: Device,
    credential: String,
    legacy_token: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Registry {
    pub code: String,
    entries: Vec<Entry>,
    #[serde(skip)]
    file: PathBuf,
}

fn eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (a, b)| acc | (a ^ b))
            == 0
}

impl Registry {
    pub fn load(dir: &Path, code: String) -> io::Result<Self> {
        let file = dir.join("devices.json");
        let mut registry = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice::<Self>(&bytes).map_err(io::Error::other)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self {
                code,
                entries: vec![],
                file: file.clone(),
            },
            Err(e) => return Err(e),
        };
        registry.file = file;
        registry.save()?;
        Ok(registry)
    }

    fn save(&self) -> io::Result<()> {
        write_atomic(&self.file, &serde_json::to_vec(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.file, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn devices(&self) -> Vec<Device> {
        let mut devices = std::collections::BTreeMap::new();
        for entry in &self.entries {
            devices.insert(entry.device.id.clone(), entry.device.clone());
        }
        devices.into_values().collect()
    }

    pub fn known_token(&self, token: &str) -> bool {
        !token.is_empty()
            && (eq(token, &self.code)
                || self.entries.iter().any(|e| {
                    !e.device.revoked && (eq(token, &e.credential) || eq(token, &e.legacy_token))
                }))
    }

    // shortcut: legacy shared tokens use node/address identity; upgrade old clients for cryptographic revocation.
    pub fn authorize(
        &mut self,
        token: &str,
        node: Option<&str>,
        identity: Option<&DeviceIdentity>,
        address: &str,
        register: bool,
    ) -> Result<Option<String>, (u16, String)> {
        if node.is_some_and(|n| n.is_empty() || n.len() > 256 || n.chars().any(char::is_control)) {
            return Err((400, "Invalid device id".into()));
        }
        let index = self
            .entries
            .iter()
            .position(|e| eq(token, &e.credential))
            .or_else(|| {
                self.entries.iter().position(|e| {
                    eq(token, &e.legacy_token)
                        && match node {
                            Some(node) => e.device.id == node,
                            None => e.device.address == address,
                        }
                })
            });
        if let Some(identity) = identity {
            if identity.name.len() > 256
                || identity.platform.len() > 64
                || identity.name.chars().any(char::is_control)
                || identity.platform.chars().any(char::is_control)
            {
                return Err((400, "Invalid device identity".into()));
            }
        }
        let mut next = self.clone();
        let index = match index {
            Some(i) if !next.entries[i].device.revoked => i,
            Some(_)
                if !register || !identity.is_some_and(|i| i.pairing) || !eq(token, &self.code) =>
            {
                return Err((401, "Access revoked, pair again".into()))
            }
            Some(_) | None if register && eq(token, &self.code) => {
                if self.entries.len() >= 1024 {
                    return Err((403, "Device limit reached".into()));
                }
                let id = node
                    .filter(|n| !n.is_empty() && n.len() <= 256)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("legacy-{}", random_string(20)));
                let identity = identity.cloned().unwrap_or_default();
                next.entries.push(Entry {
                    device: Device {
                        id,
                        name: if identity.name.trim().is_empty() {
                            "Unknown device".into()
                        } else {
                            identity.name
                        },
                        platform: identity.platform,
                        paired_at: now_ms(),
                        last_seen: now_ms(),
                        address: address.into(),
                        revoked: false,
                    },
                    credential: format!("device-{}", random_string(48)),
                    legacy_token: token.into(),
                });
                next.entries.len() - 1
            }
            _ => return Err((401, "Access revoked, pair again".into())),
        };
        let entry = &mut next.entries[index];
        if let Some(identity) = identity {
            if !identity.name.trim().is_empty() {
                entry.device.name = identity.name.clone();
            }
            if !identity.platform.trim().is_empty() {
                entry.device.platform = identity.platform.clone();
            }
        }
        entry.device.last_seen = now_ms();
        entry.device.address = address.into();
        let credential = (!eq(token, &entry.credential)).then(|| entry.credential.clone());
        next.save()
            .map_err(|_| (500, "Failed to save device registry".into()))?;
        *self = next;
        Ok(credential)
    }

    pub fn revoke(&mut self, id: &str) -> io::Result<()> {
        let mut next = self.clone();
        if !next.entries.iter().any(|e| e.device.id == id) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "Device not found"));
        }
        for entry in next.entries.iter_mut().filter(|e| e.device.id == id) {
            entry.device.revoked = true;
        }
        next.save()?;
        *self = next;
        Ok(())
    }

    pub fn rotate(&mut self) -> io::Result<String> {
        let mut next = self.clone();
        next.code = patch_core::id::pairing_token();
        next.save()?;
        *self = next;
        Ok(self.code.clone())
    }
}
