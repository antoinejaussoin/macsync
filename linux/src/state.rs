use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::schedule::now_local;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub pairs: HashMap<String, PairState>,
    /// Anchor used for catch-up after downtime (ISO local naive).
    #[serde(default)]
    pub last_scheduler_tick: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PairState {
    pub last_backup: Option<String>,
    pub last_integrity: Option<String>,
    pub last_backup_ok: Option<bool>,
    pub last_integrity_ok: Option<bool>,
    /// Last time we sent a mount/destination alert (ISO local naive).
    pub last_mount_alert: Option<String>,
}

impl AppState {
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(path, text)
    }

    pub fn pair_mut(&mut self, id: &str) -> &mut PairState {
        self.pairs.entry(id.to_string()).or_default()
    }

    pub fn scheduler_tick(&self) -> Option<NaiveDateTime> {
        self.last_scheduler_tick
            .as_ref()
            .and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok())
    }

    pub fn set_scheduler_tick(&mut self, dt: NaiveDateTime) {
        self.last_scheduler_tick = Some(dt.format("%Y-%m-%dT%H:%M:%S").to_string());
    }

    pub fn mark_backup(&mut self, id: &str, ok: bool) {
        let p = self.pair_mut(id);
        p.last_backup = Some(now_local().format("%Y-%m-%dT%H:%M:%S").to_string());
        p.last_backup_ok = Some(ok);
    }

    pub fn mark_integrity(&mut self, id: &str, ok: bool) {
        let p = self.pair_mut(id);
        p.last_integrity = Some(now_local().format("%Y-%m-%dT%H:%M:%S").to_string());
        p.last_integrity_ok = Some(ok);
    }
}
