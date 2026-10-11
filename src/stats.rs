use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Local;

use crate::backup;
use crate::config::Pair;
use crate::state::Stats;

pub fn measure(pair: &Pair) -> Stats {
    let mut stats = Stats {
        measured: Some(Local::now()),
        ..Stats::default()
    };
    let mut errors = Vec::new();

    if pair.source.is_dir() {
        match du_total(std::slice::from_ref(&pair.source)) {
            Ok(b) => stats.source_bytes = Some(b),
            Err(e) => errors.push(format!("source: {e}")),
        }
    } else {
        errors.push("source folder not found".into());
    }

    if pair.destination.is_dir() {
        if let Some(info) = backup::fs_info(&pair.destination) {
            stats.free_bytes = Some(info.free_bytes);
            stats.filesystem = Some(info.fs_type);
        }
        match backup::list_snapshots(&pair.destination) {
            Ok(snaps) => {
                stats.snapshot_count = snaps.len();
                stats.newest = snaps.first().map(|s| s.0.clone());
                stats.oldest = snaps.last().map(|s| s.0.clone());
                let dirs: Vec<PathBuf> = snaps.iter().map(|(n, _)| pair.destination.join(n)).collect();
                match du_total(&dirs) {
                    Ok(b) => stats.destination_bytes = Some(b),
                    Err(e) => errors.push(format!("destination: {e}")),
                }
            }
            Err(e) => errors.push(format!("destination: {e}")),
        }
    } else {
        errors.push("destination not available".into());
    }

    if !errors.is_empty() {
        stats.error = Some(errors.join("; "));
    }
    stats
}

/// Disk usage of all paths together. One `du` run counts each hard-linked inode
/// once, so this is the real space taken by a set of snapshots.
fn du_total(paths: &[PathBuf]) -> Result<u64, String> {
    if paths.is_empty() {
        return Ok(0);
    }
    let output = Command::new("/usr/bin/du")
        .arg("-skxc")
        .args(paths)
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let total_line = stdout.lines().rev().find(|l| l.ends_with("total"));
    let kb = total_line
        .and_then(|l| l.split_whitespace().next())
        .and_then(|n| n.parse::<u64>().ok())
        .ok_or_else(|| {
            let err = String::from_utf8_lossy(&output.stderr);
            err.lines().next().unwrap_or("du failed").to_string()
        })?;
    Ok(kb * 1024)
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn short_path(path: &Path) -> String {
    let home = crate::paths::home();
    match path.strip_prefix(&home) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(3_400_000), "3.4 MB");
        assert_eq!(format_bytes(1_500_000_000_000), "1.5 TB");
    }

    #[test]
    fn hard_links_count_once() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("big"), vec![1u8; 4_000_000]).unwrap();
        std::fs::hard_link(a.join("big"), b.join("big")).unwrap();
        let one = du_total(std::slice::from_ref(&a)).unwrap();
        let both = du_total(&[a, b]).unwrap();
        assert!(one >= 4_000_000);
        assert!(both < one + 100_000, "linked file counted twice: {one} vs {both}");
    }
}
