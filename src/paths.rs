use std::fmt;
use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `MACSYNC_HOME` relocates config, state and logs (used for testing a second instance).
pub fn app_support_dir() -> PathBuf {
    match std::env::var_os("MACSYNC_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => home().join("Library/Application Support/MacSync"),
    }
}

pub fn logs_dir() -> PathBuf {
    match std::env::var_os("MACSYNC_HOME") {
        Some(dir) => PathBuf::from(dir).join("Logs"),
        None => home().join("Library/Logs/MacSync"),
    }
}

pub fn log_file(pair_id: &str) -> PathBuf {
    logs_dir().join(format!("{pair_id}.log"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    NotSet(&'static str),
    SourceMissing(PathBuf),
    DestinationMissing(PathBuf),
    Unsafe(String),
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::NotSet(what) => write!(f, "{what} folder is not set"),
            PathError::SourceMissing(p) => write!(f, "Source folder not found: {}", p.display()),
            PathError::DestinationMissing(p) => write!(
                f,
                "Destination not available: {} (is the drive connected?)",
                p.display()
            ),
            PathError::Unsafe(msg) => f.write_str(msg),
        }
    }
}

/// Resolves both folders and refuses combinations where `--delete` or snapshot
/// pruning could touch the source. The destination must already exist so an
/// unmounted volume never turns into a new folder under `/Volumes` on the boot disk.
pub fn check_pair_paths(source: &Path, destination: &Path) -> Result<(PathBuf, PathBuf), PathError> {
    if source.as_os_str().is_empty() {
        return Err(PathError::NotSet("Source"));
    }
    if destination.as_os_str().is_empty() {
        return Err(PathError::NotSet("Destination"));
    }
    if !source.is_absolute() || !destination.is_absolute() {
        return Err(PathError::Unsafe(
            "Source and destination must be absolute paths".into(),
        ));
    }
    lexical_safety(source, destination)?;

    if !source.is_dir() {
        return Err(PathError::SourceMissing(source.to_path_buf()));
    }
    if !destination.is_dir() {
        return Err(PathError::DestinationMissing(destination.to_path_buf()));
    }
    let src = source
        .canonicalize()
        .map_err(|_| PathError::SourceMissing(source.to_path_buf()))?;
    let dst = destination
        .canonicalize()
        .map_err(|_| PathError::DestinationMissing(destination.to_path_buf()))?;
    lexical_safety(&src, &dst)?;
    Ok((src, dst))
}

fn lexical_safety(source: &Path, destination: &Path) -> Result<(), PathError> {
    if destination == Path::new("/") {
        return Err(PathError::Unsafe("Destination cannot be the root folder /".into()));
    }
    if destination == home() {
        return Err(PathError::Unsafe(
            "Destination cannot be your home folder itself".into(),
        ));
    }
    if source == destination {
        return Err(PathError::Unsafe("Source and destination are the same folder".into()));
    }
    if destination.starts_with(source) {
        return Err(PathError::Unsafe("Destination is inside the source folder".into()));
    }
    if source.starts_with(destination) {
        return Err(PathError::Unsafe("Source is inside the destination folder".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_pairs() {
        let unsafe_err =
            |s: &str, d: &str| matches!(check_pair_paths(Path::new(s), Path::new(d)), Err(PathError::Unsafe(_)));
        assert!(unsafe_err("/Users/a/Docs", "/"));
        assert!(unsafe_err("/Users/a/Docs", "/Users/a/Docs"));
        assert!(unsafe_err("/Users/a/Docs", "/Users/a/Docs/backup"));
        assert!(unsafe_err("/Volumes/B/x/Docs", "/Volumes/B/x"));
        assert!(unsafe_err("relative", "/Volumes/B"));
        assert!(matches!(
            check_pair_paths(Path::new(""), Path::new("/Volumes/B")),
            Err(PathError::NotSet(_))
        ));
    }

    #[test]
    fn similar_prefix_is_not_containment() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("Docs");
        let dst = tmp.path().join("Docs-backup");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        assert!(check_pair_paths(&src, &dst).is_ok());
    }

    #[test]
    fn missing_destination_is_reported_as_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let err = check_pair_paths(tmp.path(), Path::new("/Volumes/__macsync_missing__")).unwrap_err();
        assert!(matches!(err, PathError::DestinationMissing(_)));
    }
}
