use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::retention::Retention;
use crate::schedule;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// Overrides the Homebrew rsync lookup. Must still be rsync 3.x.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rsync_path: Option<PathBuf>,
    #[serde(rename = "pair")]
    pub pairs: Vec<Pair>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pair {
    pub id: String,
    pub name: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub enabled: bool,
    /// 5-field cron; empty means manual only.
    pub backup_schedule: String,
    /// 5-field cron; empty means manual only.
    pub integrity_schedule: String,
    pub excludes: Vec<String>,
    pub read_only: bool,
    pub retention: Retention,
}

impl Default for Pair {
    fn default() -> Self {
        Pair {
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            name: "New backup".into(),
            source: PathBuf::new(),
            destination: PathBuf::new(),
            enabled: true,
            backup_schedule: "0 2 * * *".into(),
            integrity_schedule: String::new(),
            excludes: vec![".DS_Store".into()],
            read_only: true,
            retention: Retention::default(),
        }
    }
}

impl Pair {
    /// Problems that make the pair unsavable. Folder existence is checked at run time.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.name.trim().is_empty() {
            errors.push("Name is empty".to_string());
        }
        if let Err(e @ (paths::PathError::NotSet(_) | paths::PathError::Unsafe(_))) =
            paths::check_pair_paths(&self.source, &self.destination)
        {
            errors.push(e.to_string())
        }
        if let Err(e) = schedule::validate(&self.backup_schedule) {
            errors.push(format!("Backup schedule: {e}"));
        }
        if let Err(e) = schedule::validate(&self.integrity_schedule) {
            errors.push(format!("Integrity schedule: {e}"));
        }
        errors
    }

    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            "Untitled"
        } else {
            &self.name
        }
    }
}

pub fn config_path() -> PathBuf {
    paths::app_support_dir().join("config.toml")
}

impl Config {
    /// A config that cannot be parsed is moved aside so it is never silently overwritten.
    pub fn load() -> (Config, Option<String>) {
        let path = config_path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Config::default(), None),
            Err(e) => {
                return (
                    Config::default(),
                    Some(format!("Could not read {}: {e}", path.display())),
                );
            }
        };
        match toml::from_str::<Config>(&text) {
            Ok(config) => (config, None),
            Err(e) => {
                let aside = path.with_extension(format!("toml.broken-{}", chrono::Local::now().format("%Y%m%d%H%M%S")));
                let _ = fs::rename(&path, &aside);
                (
                    Config::default(),
                    Some(format!("Config was invalid and was moved to {}: {e}", aside.display())),
                )
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_atomic(&config_path(), &text)
    }

    pub fn pair(&self, id: &str) -> Option<&Pair> {
        self.pairs.iter().find(|p| p.id == id)
    }
}

pub fn write_atomic(path: &std::path::Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).map_err(|e| format!("Could not write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("Could not replace {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention::KeepYearly;

    #[test]
    fn toml_round_trip() {
        let mut config = Config::default();
        let mut pair = Pair {
            name: "Documents".into(),
            source: "/Users/a/Documents".into(),
            destination: "/Volumes/Backup/Documents".into(),
            ..Pair::default()
        };
        pair.retention.keep_yearly = KeepYearly::Count(5);
        config.pairs.push(pair);
        config.pairs.push(Pair::default());
        let text = toml::to_string_pretty(&config).unwrap();
        assert!(text.contains("[[pair]]"));
        assert!(text.contains("keep_yearly = 5"));
        assert!(text.contains("keep_yearly = \"forever\""));
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let config: Config =
            toml::from_str("[[pair]]\nid = \"x\"\nname = \"Docs\"\n[pair.retention]\nkeep_last = 10\n").unwrap();
        let pair = &config.pairs[0];
        assert_eq!(pair.retention.keep_last, 10);
        assert_eq!(pair.retention.keep_monthly, 3);
        assert_eq!(pair.retention.keep_yearly, KeepYearly::Forever);
        assert!(pair.read_only);
    }
}
