use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::paths::{expand_tilde, is_absolute_local, parse_remote_source};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default = "default_rsync_path")]
    pub rsync_path: String,
    /// GNU rsync on the remote (passed as `--rsync-path=`). Required when the
    /// remote's default `rsync` is Apple's openrsync, which rejects `-A`.
    #[serde(default)]
    pub remote_rsync_path: String,
    #[serde(default)]
    pub ssh: SshConfig,
    #[serde(default)]
    pub pair: Vec<FolderPair>,
}

fn default_rsync_path() -> String {
    "/usr/bin/rsync".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SshConfig {
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    #[serde(default = "default_identity")]
    pub identity_file: String,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

fn default_ssh_port() -> u16 {
    22
}

fn default_identity() -> String {
    // Empty → do not pass -i; use IdentityFile / keys from ~/.ssh/config (Host aliases).
    String::new()
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            port: default_ssh_port(),
            identity_file: default_identity(),
            extra_args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderPair {
    pub id: String,
    pub name: String,
    pub source: String,
    pub destination: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub backup_schedule: String,
    #[serde(default)]
    pub integrity_schedule: String,
    #[serde(default)]
    pub excludes: Vec<String>,
    #[serde(default = "default_true")]
    pub read_only: bool,
    #[serde(default)]
    pub retention: RetentionPolicy,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetentionPolicy {
    #[serde(default = "default_keep_last")]
    pub keep_last: u32,
    #[serde(default)]
    pub keep_daily: u32,
    #[serde(default)]
    pub keep_weekly: u32,
    #[serde(default)]
    pub keep_monthly: u32,
    #[serde(default = "default_keep_yearly")]
    pub keep_yearly: KeepYearly,
}

fn default_keep_last() -> u32 {
    30
}

fn default_keep_yearly() -> KeepYearly {
    KeepYearly::Forever
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            keep_last: default_keep_last(),
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 0,
            keep_yearly: KeepYearly::Forever,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeepYearly {
    Forever,
    Years(u32),
}

impl Serialize for KeepYearly {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            KeepYearly::Forever => serializer.serialize_str("forever"),
            KeepYearly::Years(n) => serializer.serialize_u32(*n),
        }
    }
}

impl<'de> Deserialize<'de> for KeepYearly {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = KeepYearly;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("\"forever\" or a non-negative integer")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if v.eq_ignore_ascii_case("forever") {
                    Ok(KeepYearly::Forever)
                } else if let Ok(n) = v.parse::<u32>() {
                    Ok(KeepYearly::Years(n))
                } else {
                    Err(E::custom(format!("invalid keep_yearly: {v}")))
                }
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(KeepYearly::Years(v as u32))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                if v < 0 {
                    return Err(E::custom("keep_yearly must be non-negative"));
                }
                Ok(KeepYearly::Years(v as u32))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|e| {
            ConfigError(format!("cannot read config {}: {e}", path.display()))
        })?;
        let mut cfg: Config = toml::from_str(&text)
            .map_err(|e| ConfigError(format!("invalid config {}: {e}", path.display())))?;
        cfg.normalize();
        cfg.validate()?;
        Ok(cfg)
    }

    fn normalize(&mut self) {
        self.rsync_path = expand_tilde(&self.rsync_path)
            .to_string_lossy()
            .into_owned();
        self.ssh.identity_file = expand_tilde(&self.ssh.identity_file)
            .to_string_lossy()
            .into_owned();
        for pair in &mut self.pair {
            pair.destination = expand_tilde(&pair.destination)
                .to_string_lossy()
                .into_owned();
            pair.source = pair.source.trim().to_string();
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.pair.is_empty() {
            return Err(ConfigError(
                "config has no [[pair]] entries; refusing to start".into(),
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for pair in &self.pair {
            if pair.id.trim().is_empty() {
                return Err(ConfigError("pair id must not be empty".into()));
            }
            if !ids.insert(pair.id.clone()) {
                return Err(ConfigError(format!("duplicate pair id '{}'", pair.id)));
            }
            if parse_remote_source(&pair.source).is_none() {
                return Err(ConfigError(format!(
                    "pair '{}': source must be user@host:path (got '{}')",
                    pair.id, pair.source
                )));
            }
            let dest = PathBuf::from(&pair.destination);
            if !is_absolute_local(&dest) {
                return Err(ConfigError(format!(
                    "pair '{}': destination must be an absolute local path (got '{}')",
                    pair.id, pair.destination
                )));
            }
        }
        Ok(())
    }

    pub fn pair_by_id(&self, id: &str) -> Option<&FolderPair> {
        self.pair.iter().find(|p| p.id == id)
    }
}

pub const EXAMPLE_CONFIG: &str = include_str!("../config.example.toml");

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn loads_example_defaults() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "{}", EXAMPLE_CONFIG).unwrap();
        let cfg = Config::load(f.path()).unwrap();
        assert_eq!(cfg.rsync_path, "/usr/bin/rsync");
        assert_eq!(cfg.ssh.port, 22);
        assert_eq!(cfg.pair.len(), 2);
        assert_eq!(cfg.pair[0].id, "photos");
        assert_eq!(cfg.pair[0].retention.keep_last, 30);
        assert_eq!(cfg.pair[0].retention.keep_yearly, KeepYearly::Forever);
        assert!(cfg.pair[0].read_only);
        assert_eq!(cfg.pair[1].id, "docs");
        assert_eq!(cfg.pair[1].integrity_schedule, "");
        assert_eq!(cfg.pair[1].retention.keep_yearly, KeepYearly::Years(2));
    }

    #[test]
    fn rejects_local_destination_relpath() {
        let toml = r#"
[[pair]]
id = "a"
name = "A"
source = "u@h:/src"
destination = "relative/path"
"#;
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "{toml}").unwrap();
        let err = Config::load(f.path()).unwrap_err();
        assert!(err.0.contains("absolute"));
    }

    #[test]
    fn keep_yearly_number() {
        let toml = r#"
[[pair]]
id = "a"
name = "A"
source = "u@h:/src"
destination = "/tmp/a"
[pair.retention]
keep_yearly = 5
"#;
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "{toml}").unwrap();
        let cfg = Config::load(f.path()).unwrap();
        assert_eq!(cfg.pair[0].retention.keep_yearly, KeepYearly::Years(5));
    }
}
