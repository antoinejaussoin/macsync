use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::config::write_atomic;
use crate::paths;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub pairs: BTreeMap<String, PairState>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PairState {
    pub last_backup: Option<RunRecord>,
    /// Newest backup that finished successfully, kept even after later failures.
    pub last_success: Option<DateTime<Local>>,
    pub last_integrity: Option<IntegrityRecord>,
    pub stats: Option<Stats>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Success,
    Warning,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub started: DateTime<Local>,
    pub finished: DateTime<Local>,
    pub outcome: Outcome,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(default)]
    pub pruned: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IntegrityRecord {
    pub started: Option<DateTime<Local>>,
    pub finished: Option<DateTime<Local>>,
    pub outcome: Option<Outcome>,
    pub message: String,
    pub snapshot: String,
    /// Same size and modification time, different checksum: corruption or tampering.
    pub silent: Vec<String>,
    pub silent_count: usize,
    /// Edited in the source since the snapshot.
    pub changed: Vec<String>,
    pub changed_count: usize,
    /// In the source but not the snapshot.
    pub missing: Vec<String>,
    pub missing_count: usize,
    /// In the snapshot but no longer in the source.
    pub extra: Vec<String>,
    pub extra_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    pub measured: Option<DateTime<Local>>,
    pub source_bytes: Option<u64>,
    /// All snapshots together, each hard-linked file counted once.
    pub destination_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub filesystem: Option<String>,
    pub snapshot_count: usize,
    pub oldest: Option<String>,
    pub newest: Option<String>,
    pub error: Option<String>,
}

pub fn state_path() -> PathBuf {
    paths::app_support_dir().join("state.toml")
}

impl State {
    pub fn load() -> State {
        fs::read_to_string(state_path())
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_atomic(&state_path(), &text)
    }

    pub fn pair_mut(&mut self, id: &str) -> &mut PairState {
        self.pairs.entry(id.to_string()).or_default()
    }
}
