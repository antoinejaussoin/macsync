use chrono::{Datelike, Local, NaiveDateTime};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{KeepYearly, RetentionPolicy};

/// Snapshot directory name: `YYYY-MM-DDTHHMMSS` (local time).
pub const STAMP_FORMAT: &str = "%Y-%m-%dT%H%M%S";

pub fn format_stamp(dt: chrono::DateTime<Local>) -> String {
    dt.format(STAMP_FORMAT).to_string()
}

pub fn parse_stamp(name: &str) -> Option<NaiveDateTime> {
    if name.len() != 17 {
        return None;
    }
    NaiveDateTime::parse_from_str(name, STAMP_FORMAT).ok()
}

pub fn list_snapshots(dest: &Path) -> std::io::Result<Vec<(String, NaiveDateTime)>> {
    let mut out = Vec::new();
    if !dest.exists() {
        return Ok(out);
    }
    for entry in fs::read_dir(dest)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ft = entry.file_type()?;
        if ft.is_symlink() || !ft.is_dir() {
            continue;
        }
        if name.starts_with('.') {
            continue;
        }
        if let Some(dt) = parse_stamp(&name) {
            out.push((name.into_owned(), dt));
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    Ok(out)
}

pub fn latest_snapshot_name(dest: &Path) -> Option<String> {
    let link = dest.join("latest");
    let target = fs::read_link(&link).ok()?;
    let name = target
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| target.to_string_lossy().into_owned());
    if parse_stamp(&name).is_none() {
        return None;
    }
    let folder = dest.join(&name);
    if folder.is_dir() {
        Some(name)
    } else {
        None
    }
}

pub fn latest_snapshot_path(dest: &Path) -> Option<PathBuf> {
    latest_snapshot_name(dest).map(|n| dest.join(n))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub name: String,
    pub keep: bool,
}

/// Mirrors Swift `retentionPlan`: newest first; always keep `latest`; tier buckets.
pub fn retention_plan(
    snapshots: &[(String, NaiveDateTime)],
    policy: &RetentionPolicy,
    latest: Option<&str>,
) -> Vec<Decision> {
    let mut sorted = snapshots.to_vec();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));

    let mut kept: Vec<HashSet<&'static str>> = vec![HashSet::new(); sorted.len()];
    let mut mark = |index: usize, tier: &'static str| {
        kept[index].insert(tier);
    };

    if let Some(latest_name) = latest {
        for (index, (name, _)) in sorted.iter().enumerate() {
            if name == latest_name {
                mark(index, "latest");
            }
        }
    }

    let last_count = sorted.len().min(policy.keep_last as usize);
    for index in 0..last_count {
        mark(index, "last");
    }

    let yearly_limit = match &policy.keep_yearly {
        KeepYearly::Forever => u32::MAX,
        KeepYearly::Years(n) => *n,
    };

    let tiers: [(&'static str, u32, Box<dyn Fn(&NaiveDateTime) -> String>); 4] = [
        ("daily", policy.keep_daily, Box::new(|dt| {
            format!("{}-{}-{}", dt.year(), dt.month(), dt.day())
        })),
        ("weekly", policy.keep_weekly, Box::new(|dt| {
            let week = dt.iso_week();
            format!("{}-{}", week.year(), week.week())
        })),
        ("monthly", policy.keep_monthly, Box::new(|dt| {
            format!("{}-{}", dt.year(), dt.month())
        })),
        ("yearly", yearly_limit, Box::new(|dt| format!("{}", dt.year()))),
    ];

    for (tier, limit, bucket) in tiers {
        if limit == 0 {
            continue;
        }
        let mut last_bucket: Option<String> = None;
        let mut buckets_seen = 0u32;
        for (index, (_, dt)) in sorted.iter().enumerate() {
            if buckets_seen >= limit {
                break;
            }
            let value = bucket(dt);
            if last_bucket.as_ref() != Some(&value) {
                last_bucket = Some(value);
                buckets_seen += 1;
                mark(index, tier);
            }
        }
    }

    sorted
        .into_iter()
        .enumerate()
        .map(|(index, (name, _))| Decision {
            name,
            keep: !kept[index].is_empty(),
        })
        .collect()
}

#[cfg(test)]
pub fn select_kept(
    stamps: &[(String, NaiveDateTime)],
    policy: &RetentionPolicy,
    latest: Option<&str>,
) -> HashSet<String> {
    retention_plan(stamps, policy, latest)
        .into_iter()
        .filter(|d| d.keep)
        .map(|d| d.name)
        .collect()
}

pub fn apply_retention(dest: &Path, policy: &RetentionPolicy) -> std::io::Result<Vec<String>> {
    let stamps = list_snapshots(dest)?;
    let latest = latest_snapshot_name(dest);
    let plan = retention_plan(&stamps, policy, latest.as_deref());
    let mut deleted = Vec::new();
    for decision in plan.into_iter().rev() {
        if decision.keep {
            continue;
        }
        let path = dest.join(&decision.name);
        remove_tree(&path)?;
        deleted.push(decision.name);
    }
    Ok(deleted)
}

fn remove_tree(path: &Path) -> std::io::Result<()> {
    chmod_tree_writable(path)?;
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn chmod_tree_writable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if !path.exists() {
        return Ok(());
    }
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        let mut perms = meta.permissions();
        perms.set_mode(perms.mode() | 0o700);
        fs::set_permissions(path, perms)?;
        for entry in fs::read_dir(path)? {
            chmod_tree_writable(&entry?.path())?;
        }
    }
    Ok(())
}

/// Clear write bits on directories under a snapshot (matches Mac `makeReadOnly`).
pub fn make_snapshot_read_only(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fn walk(p: &Path) -> std::io::Result<()> {
        let meta = fs::symlink_metadata(p)?;
        if meta.file_type().is_symlink() {
            return Ok(());
        }
        if meta.is_dir() {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() & !0o222);
            fs::set_permissions(p, perms)?;
            for entry in fs::read_dir(p)? {
                walk(&entry?.path())?;
            }
        }
        Ok(())
    }
    walk(path)
}

pub fn ensure_relative_latest(dest: &Path, stamp_name: &str) -> std::io::Result<()> {
    let tmp = dest.join(".latest.tmp");
    let link = dest.join("latest");
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(stamp_name, &tmp)?;
    fs::rename(&tmp, &link)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KeepYearly;
    use chrono::TimeZone;

    fn stamp(s: &str) -> (String, NaiveDateTime) {
        (s.to_string(), parse_stamp(s).unwrap())
    }

    #[test]
    fn keep_last_only() {
        let stamps = vec![
            stamp("2024-01-03T120000"),
            stamp("2024-01-02T120000"),
            stamp("2024-01-01T120000"),
        ];
        let policy = RetentionPolicy {
            keep_last: 2,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 0,
            keep_yearly: KeepYearly::Years(0),
        };
        let keep = select_kept(&stamps, &policy, None);
        assert_eq!(keep.len(), 2);
        assert!(keep.contains("2024-01-03T120000"));
        assert!(keep.contains("2024-01-02T120000"));
    }

    #[test]
    fn keep_monthly() {
        let stamps = vec![
            stamp("2024-03-15T120000"),
            stamp("2024-03-01T120000"),
            stamp("2024-02-20T120000"),
            stamp("2024-01-10T120000"),
        ];
        let policy = RetentionPolicy {
            keep_last: 0,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 2,
            keep_yearly: KeepYearly::Years(0),
        };
        let keep = select_kept(&stamps, &policy, None);
        assert!(keep.contains("2024-03-15T120000"));
        assert!(keep.contains("2024-02-20T120000"));
        assert!(!keep.contains("2024-01-10T120000"));
        assert!(!keep.contains("2024-03-01T120000"));
    }

    #[test]
    fn always_keeps_latest() {
        let stamps = vec![
            stamp("2024-01-03T120000"),
            stamp("2024-01-02T120000"),
            stamp("2024-01-01T120000"),
        ];
        let policy = RetentionPolicy {
            keep_last: 0,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 0,
            keep_yearly: KeepYearly::Years(0),
        };
        let keep = select_kept(&stamps, &policy, Some("2024-01-01T120000"));
        assert!(keep.contains("2024-01-01T120000"));
        assert_eq!(keep.len(), 1);
    }

    #[test]
    fn stamp_roundtrip() {
        let now = Local.with_ymd_and_hms(2024, 6, 15, 14, 30, 45).unwrap();
        let s = format_stamp(now);
        assert_eq!(s, "2024-06-15T143045");
        assert_eq!(parse_stamp(&s).unwrap(), now.naive_local());
        assert!(parse_stamp("short").is_none());
    }
}
