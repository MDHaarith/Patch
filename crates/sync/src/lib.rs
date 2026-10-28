//! Sync between Patch devices over a Tailscale tailnet.
//!
//! One device (the Linux desktop app, or the headless `patch-hub` on any
//! always-on machine) runs the [`hub::Hub`]. Every app instance runs a
//! [`client::SyncClient`] that pushes local changes, pulls remote ones and
//! keeps a long-poll open so edits show up on other devices within a second.
//!
//! Tailscale (WireGuard) encrypts and authenticates the transport; the hub
//! additionally refuses peers outside the tailnet and requires a pairing
//! token, so other apps on a shared tailnet can't read your board.

pub mod client;
pub mod devices;
pub use devices::{Device, DeviceIdentity};
pub mod http;
pub mod hub;
pub mod net;
pub mod proto;

pub use client::{SyncClient, SyncConfig, SyncEvent, SyncStatus};
pub use hub::{Hub, HubConfig};

pub const DEFAULT_PORT: u16 = 4747;
