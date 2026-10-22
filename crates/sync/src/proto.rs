//! Wire format shared by hub and client.

use patch_core::Record;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncRequest {
    /// Highest hub sequence number the client has seen.
    pub since: u64,
    /// Records with local changes.
    pub changes: Vec<Record>,
    #[serde(default)]
    pub node: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncResponse {
    pub hub: String,
    pub seq: u64,
    /// Every record that changed on the hub after `since`.
    pub changes: Vec<Record>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WaitResponse {
    pub hub: String,
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubInfo {
    pub hub: String,
    pub name: String,
    pub version: String,
    pub seq: u64,
    pub records: usize,
    pub urls: Vec<String>,
}
