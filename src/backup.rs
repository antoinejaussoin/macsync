use std::ffi::CStr;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use chrono::{Local, NaiveDateTime};

use crate::config::Pair;
use crate::paths::{self, PathError};
use crate::retention::{self, Retention, STAMP_FORMAT};
use crate::rsync::{self, BackupArgs, CancelHandle, Progress, RsyncInfo};

pub const LATEST: &str = "latest";
pub const IN_PROGRESS_PREFIX: &str = ".inprogress-";

/// Completed snapshot folders in `dest`, newest first. Anything that is not a
/// stamp-named real directory (symlinks, `.inprogress-*`, user files) is ignored.
pub fn list_snapshots(dest: &Path) -> io::Result<Vec<(String, NaiveDateTime)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dest)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(time) = retention::parse_stamp(&name) else {
            continue;
        };
        if entry.file_type()?.is_dir() {
            out.push((name, time));
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.1));
    Ok(out)
}

/// Snapshot name `latest` points at, if it is a valid, existing snapshot.
pub fn latest_name(dest: &Path) -> Option<String> {
    let target = fs::read_link(dest.join(LATEST)).ok()?;
    let name = target.file_name()?.to_str()?.to_owned();
    (retention::parse_stamp(&name).is_some() && dest.join(&name).is_dir()).then_some(name)
}

/// Relative symlink, swapped in with a rename so `latest` is never missing.
pub fn set_latest(dest: &Path, name: &str) -> io::Result<()> {
    let tmp = dest.join(".latest.tmp");
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(name, &tmp)?;
    fs::rename(&tmp, dest.join(LATEST))
}

/// Files are already read-only (copied with `--chmod=Fa-w`); this locks the folders
/// so nothing can be added, renamed or deleted inside the snapshot.
pub fn make_read_only(dir: &Path) -> io::Result<()> {
    for entry in walkdir::WalkDir::new(dir).follow_links(false) {
        let entry = entry.map_err(io::Error::other)?;
        if entry.file_type().is_dir() {
            let mut perms = entry.metadata().map_err(io::Error::other)?.permissions();
            perms.set_mode(perms.mode() & !0o222);
            fs::set_permissions(entry.path(), perms)?;
        }
    }
    Ok(())
}

pub fn remove_tree(dir: &Path) -> io::Result<()> {
    for entry in walkdir::WalkDir::new(dir).follow_links(false).into_iter().flatten() {
        if entry.file_type().is_dir()
            && let Ok(meta) = entry.metadata()
        {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o700);
            let _ = fs::set_permissions(entry.path(), perms);
        }
    }
    fs::remove_dir_all(dir)
}

fn cleanup_in_progress(dest: &Path, log: &mut File) {
    let Ok(entries) = fs::read_dir(dest) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(IN_PROGRESS_PREFIX) && entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = writeln!(log, "Removing incomplete snapshot {name}");
            if let Err(e) = remove_tree(&entry.path()) {
                let _ = writeln!(log, "  could not remove: {e}");
            }
        }
    }
}

/// Removes snapshots the retention ladder does not keep, oldest first.
pub fn prune(dest: &Path, policy: &Retention, log: &mut File) -> Result<Vec<String>, String> {
    let snapshots = list_snapshots(dest).map_err(|e| format!("Could not list snapshots: {e}"))?;
    let latest = latest_name(dest);
    let mut removed = Vec::new();
    for decision in retention::plan(&snapshots, policy, latest.as_deref()).iter().rev() {
        if decision.keep() {
            continue;
        }
        let _ = writeln!(log, "Retention: removing {}", decision.name);
        remove_tree(&dest.join(&decision.name)).map_err(|e| format!("Could not remove {}: {e}", decision.name))?;
        removed.push(decision.name.clone());
    }
    Ok(removed)
}

pub struct FsInfo {
    pub fs_type: String,
    pub free_bytes: u64,
}

pub fn fs_info(path: &Path) -> Option<FsInfo> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let fs_type = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    Some(FsInfo {
        fs_type,
        free_bytes: st.f_bavail.saturating_mul(st.f_bsize as u64),
    })
}

pub fn filesystem_supports_hard_links(fs_type: &str) -> bool {
    !matches!(fs_type, "msdos" | "exfat" | "ntfs")
}

#[derive(Debug)]
pub enum BackupError {
    DestinationMissing(String),
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackupError::DestinationMissing(m) | BackupError::Failed(m) => f.write_str(m),
            BackupError::Cancelled => f.write_str("Cancelled"),
        }
    }
}

pub struct BackupResult {
    pub snapshot: String,
    pub pruned: Vec<String>,
    pub summary: String,
    pub warning: Option<String>,
}

pub fn run_backup(
    pair: &Pair,
    rsync_info: &RsyncInfo,
    log: &mut File,
    cancel: &CancelHandle,
    on_progress: impl FnMut(Progress),
) -> Result<BackupResult, BackupError> {
    let (source, dest) = paths::check_pair_paths(&pair.source, &pair.destination).map_err(|e| match e {
        PathError::DestinationMissing(_) => BackupError::DestinationMissing(e.to_string()),
        other => BackupError::Failed(other.to_string()),
    })?;
    if let Some(info) = fs_info(&dest)
        && !filesystem_supports_hard_links(&info.fs_type)
    {
        return Err(BackupError::Failed(format!(
            "The destination drive is formatted as {}, which has no hard links. Use an APFS or Mac OS Extended drive.",
            info.fs_type
        )));
    }
    match fs::read_dir(&source) {
        Ok(mut entries) => {
            if entries.next().is_none() {
                return Err(BackupError::Failed(
                    "Source folder is empty; not creating an empty snapshot".into(),
                ));
            }
        }
        Err(e) => {
            return Err(BackupError::Failed(format!(
                "Cannot read the source folder ({e}). Grant MacSync Full Disk Access in System Settings > Privacy & Security."
            )));
        }
    }

    cleanup_in_progress(&dest, log);

    let mut stamp = Local::now().format(STAMP_FORMAT).to_string();
    while dest.join(&stamp).exists() {
        std::thread::sleep(std::time::Duration::from_millis(1100));
        stamp = Local::now().format(STAMP_FORMAT).to_string();
    }
    let work = dest.join(format!("{IN_PROGRESS_PREFIX}{stamp}"));
    let link_name = latest_name(&dest).or_else(|| {
        list_snapshots(&dest)
            .ok()
            .and_then(|s| s.first().map(|(n, _)| n.clone()))
    });
    let link_dest = link_name.as_ref().map(|n| dest.join(n));

    let args = rsync::backup_args(&BackupArgs {
        source: &source,
        target: &work,
        link_dest: link_dest.as_deref(),
        excludes: &pair.excludes,
        read_only: pair.read_only,
    });
    let _ = writeln!(
        log,
        "Snapshot {stamp} of {} (linking against {})",
        source.display(),
        link_name.as_deref().unwrap_or("nothing: first snapshot")
    );
    let _ = writeln!(log, "{} {}", rsync_info.path.display(), args_display(&args));

    let output = rsync::run(&rsync_info.path, &args, log, cancel, true, on_progress).map_err(BackupError::Failed)?;
    if cancel.is_cancelled() {
        let _ = writeln!(log, "Cancelled; the incomplete snapshot is removed on the next run");
        return Err(BackupError::Cancelled);
    }
    let warning = match output.exit_code {
        0 => None,
        24 => Some(rsync::exit_code_meaning(24).to_string()),
        code => {
            let detail = output.stderr_tail.last().cloned().unwrap_or_default();
            let _ = writeln!(
                log,
                "rsync failed with exit code {code}; latest still points at the previous snapshot"
            );
            return Err(BackupError::Failed(format!(
                "rsync exit {code}: {}{}",
                rsync::exit_code_meaning(code),
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(" ({detail})")
                }
            )));
        }
    };

    fs::rename(&work, dest.join(&stamp))
        .map_err(|e| BackupError::Failed(format!("Could not finalize snapshot: {e}")))?;
    set_latest(&dest, &stamp).map_err(|e| BackupError::Failed(format!("Could not update latest: {e}")))?;
    let pruned = prune(&dest, &pair.retention, log).map_err(BackupError::Failed)?;
    if pair.read_only
        && let Err(e) = make_read_only(&dest.join(&stamp))
    {
        let _ = writeln!(log, "Could not make snapshot read-only: {e}");
    }
    let summary = summarize_stats(&output.stdout_lines);
    let _ = writeln!(
        log,
        "Snapshot {stamp} complete. {summary}. Removed {} old snapshot(s).",
        pruned.len()
    );
    Ok(BackupResult {
        snapshot: stamp,
        pruned,
        summary,
        warning,
    })
}

pub fn args_display(args: &[std::ffi::OsString]) -> String {
    args.iter()
        .map(|a| {
            let s = a.to_string_lossy();
            if s.contains(' ') {
                format!("'{s}'")
            } else {
                s.into_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Turns `--info=stats2` lines into "12 files copied (3.4 MB) of 1,031".
pub fn summarize_stats(lines: &[String]) -> String {
    let value = |prefix: &str| {
        lines.iter().find_map(|l| {
            l.trim()
                .strip_prefix(prefix)
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|n| n.replace(',', "").parse::<u64>().ok())
        })
    };
    let copied = value("Number of regular files transferred:");
    let total = value("Number of files:");
    let bytes = value("Total transferred file size:");
    match (copied, total, bytes) {
        (Some(c), Some(t), Some(b)) => format!("{c} files copied ({}) of {t}", crate::stats::format_bytes(b)),
        (Some(c), _, _) => format!("{c} files copied"),
        _ => "Finished".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_snapshot(dest: &Path, name: &str) {
        fs::create_dir_all(dest.join(name).join("sub")).unwrap();
        fs::write(dest.join(name).join("sub/file.txt"), name).unwrap();
    }

    #[test]
    fn lists_only_stamp_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        touch_snapshot(d, "2026-10-10T020000");
        touch_snapshot(d, "2026-10-11T020000");
        touch_snapshot(d, ".inprogress-2026-10-12T020000");
        fs::write(d.join("2026-10-09T020000"), "a file, not a snapshot").unwrap();
        set_latest(d, "2026-10-11T020000").unwrap();
        let names: Vec<String> = list_snapshots(d).unwrap().into_iter().map(|s| s.0).collect();
        assert_eq!(names, ["2026-10-11T020000", "2026-10-10T020000"]);
        assert_eq!(latest_name(d).as_deref(), Some("2026-10-11T020000"));
        assert_eq!(fs::read_link(d.join(LATEST)).unwrap(), Path::new("2026-10-11T020000"));
    }

    #[test]
    fn prune_removes_read_only_snapshots_and_keeps_latest() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        for day in 1..=5 {
            let name = format!("2026-10-0{day}T020000");
            touch_snapshot(d, &name);
            make_read_only(&d.join(&name)).unwrap();
        }
        set_latest(d, "2026-10-05T020000").unwrap();
        let policy = Retention {
            keep_last: 2,
            keep_monthly: 0,
            keep_yearly: crate::retention::KeepYearly::Count(0),
            ..Retention::default()
        };
        let mut log = tempfile::tempfile().unwrap();
        let removed = prune(d, &policy, &mut log).unwrap();
        assert_eq!(removed, ["2026-10-01T020000", "2026-10-02T020000", "2026-10-03T020000"]);
        let left: Vec<String> = list_snapshots(d).unwrap().into_iter().map(|s| s.0).collect();
        assert_eq!(left, ["2026-10-05T020000", "2026-10-04T020000"]);
    }

    #[test]
    fn read_only_blocks_new_files() {
        let tmp = tempfile::tempdir().unwrap();
        touch_snapshot(tmp.path(), "2026-10-01T020000");
        let snap = tmp.path().join("2026-10-01T020000");
        make_read_only(&snap).unwrap();
        assert!(fs::write(snap.join("sub/new.txt"), "x").is_err());
        assert!(fs::remove_file(snap.join("sub/file.txt")).is_err());
        remove_tree(&snap).unwrap();
        assert!(!snap.exists());
    }

    #[test]
    fn stats_summary() {
        let lines: Vec<String> = [
            "Number of files: 1,031 (reg: 1,000, dir: 31)",
            "Number of regular files transferred: 12",
            "Total file size: 9,999,999 bytes",
            "Total transferred file size: 3,400,000 bytes",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(summarize_stats(&lines), "12 files copied (3.4 MB) of 1031");
    }

    /// Needs Homebrew rsync: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn end_to_end_snapshots_share_unchanged_files() {
        use std::os::unix::fs::MetadataExt;
        let rsync_info = rsync::locate(None).expect("brew install rsync");
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("My Source");
        let dst = tmp.path().join("Backup");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("same.txt"), "unchanged").unwrap();
        fs::write(src.join("nested/edit.txt"), "v1").unwrap();
        let pair = Pair {
            source: src.clone(),
            destination: dst.clone(),
            ..Pair::default()
        };
        let mut log = tempfile::tempfile().unwrap();
        let cancel = CancelHandle::default();

        let first = run_backup(&pair, &rsync_info, &mut log, &cancel, |_| {}).unwrap();
        fs::write(src.join("nested/edit.txt"), "v2 longer").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let second = run_backup(&pair, &rsync_info, &mut log, &cancel, |_| {}).unwrap();

        let a = dst.join(&first.snapshot);
        let b = dst.join(&second.snapshot);
        assert_eq!(latest_name(&dst).as_deref(), Some(second.snapshot.as_str()));
        assert_eq!(
            fs::metadata(a.join("same.txt")).unwrap().ino(),
            fs::metadata(b.join("same.txt")).unwrap().ino()
        );
        assert_ne!(
            fs::metadata(a.join("nested/edit.txt")).unwrap().ino(),
            fs::metadata(b.join("nested/edit.txt")).unwrap().ino()
        );
        assert_eq!(fs::read_to_string(a.join("nested/edit.txt")).unwrap(), "v1");
        assert!(
            fs::write(b.join("new.txt"), "x").is_err(),
            "snapshot folders are read-only"
        );

        let report = crate::integrity::run_integrity(&pair, &rsync_info, &mut log, &cancel).unwrap();
        assert_eq!(report.outcome, Some(crate::state::Outcome::Success), "{report:?}");

        // Flip bytes in the backup copy while keeping size and modification time.
        let tampered = b.join("nested/edit.txt");
        let mtime = fs::metadata(&tampered).unwrap().modified().unwrap();
        fs::set_permissions(&tampered, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&tampered, "v2 LONGER").unwrap();
        File::options()
            .write(true)
            .open(&tampered)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        fs::set_permissions(&tampered, fs::Permissions::from_mode(0o444)).unwrap();

        let report = crate::integrity::run_integrity(&pair, &rsync_info, &mut log, &cancel).unwrap();
        assert_eq!(report.outcome, Some(crate::state::Outcome::Failed), "{report:?}");
        assert_eq!(report.silent, ["nested/edit.txt"]);
    }
}
