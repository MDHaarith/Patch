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
    /// The client's cursor predates a hub restore; re-push local records.
    /// Older hubs omit this, and older clients ignore it.
    #[serde(default)]
    pub reset: bool,
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

/// Validate the canonical encoding used for lexicographic stamp ordering.
pub(crate) fn stamp_wall(stamp: &patch_core::Stamp) -> Option<u64> {
    let (wall, counter) = stamp.parts()?;
    let node = stamp.0.splitn(3, '-').nth(2)?;
    (counter <= 9999 && !node.is_empty() && patch_core::Stamp::new(wall, counter, node) == *stamp)
        .then_some(wall)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_reset_remains_compatible_with_legacy_hubs_and_clients() {
        let old = r#"{"hub":"hub","seq":7,"changes":[]}"#;
        let mut response: SyncResponse = serde_json::from_str(old).unwrap();
        assert!(!response.reset, "old hubs do not send the optional flag");
        response.reset = true;
        #[derive(Deserialize)]
        struct LegacyResponse {
            hub: String,
            seq: u64,
            changes: Vec<Record>,
        }
        let old_client: LegacyResponse = serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
        assert_eq!(old_client.hub, "hub");
        assert_eq!(old_client.seq, 7);
        assert!(old_client.changes.is_empty());
    }
}
