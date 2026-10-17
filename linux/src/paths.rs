use std::env;
use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return home();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home().join(rest);
    }
    PathBuf::from(path)
}

/// Directory containing this binary (resolved; falls back to `.`).
pub fn executable_dir() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Default: `config.toml` next to the executable. `MACSYNC_CONFIG` overrides.
pub fn config_path() -> PathBuf {
    if let Some(path) = env::var_os("MACSYNC_CONFIG") {
        return PathBuf::from(path);
    }
    executable_dir().join("config.toml")
}

/// Data directory: next to the executable (or `MACSYNC_HOME` if set).
pub fn data_dir() -> PathBuf {
    if let Some(path) = env::var_os("MACSYNC_HOME") {
        return PathBuf::from(path);
    }
    executable_dir()
}

pub fn state_path() -> PathBuf {
    data_dir().join("state.toml")
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

pub fn log_file(pair_id: &str) -> PathBuf {
    logs_dir().join(format!("{pair_id}.log"))
}

/// `user@host:path` (or `host:path`). Returns None when there is no host separator.
pub fn parse_remote_source(source: &str) -> Option<(String, String)> {
    let source = source.trim();
    if source.is_empty() {
        return None;
    }
    // Avoid treating Windows-style or bare absolute paths as remote.
    if source.starts_with('/') {
        return None;
    }
    let colon = source.find(':')?;
    if colon == 0 {
        return None;
    }
    let host = source[..colon].to_string();
    let path = source[colon + 1..].to_string();
    if host.contains('/') {
        return None;
    }
    Some((host, path))
}

pub fn is_absolute_local(path: &Path) -> bool {
    path.is_absolute()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_source_split() {
        let (host, path) = parse_remote_source("backup@nas:/volume1/photos").unwrap();
        assert_eq!(host, "backup@nas");
        assert_eq!(path, "/volume1/photos");
        assert!(parse_remote_source("/local/path").is_none());
        assert!(parse_remote_source("noslash").is_none());
    }

    #[test]
    fn tilde_expands() {
        let p = expand_tilde("~/backups");
        assert!(p.starts_with(home()));
        assert!(p.ends_with("backups"));
    }
}
