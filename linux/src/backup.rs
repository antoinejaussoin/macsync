use chrono::Local;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{Config, FolderPair};
use crate::logutil::{append_line, PairLogger};
use crate::retention::{
    apply_retention, ensure_relative_latest, format_stamp, latest_snapshot_path,
    make_snapshot_read_only, parse_stamp,
};
use crate::rsync::{
    backup_arguments, display_arguments, is_stats_line, parse_progress, rsync_exit_meaning,
    summarize_stats, BackupArgParams, TransferProgress,
};

const INPROGRESS_PREFIX: &str = ".inprogress-";

/// rsync exit codes that still yield a usable partial snapshot (missing/vanished files).
fn rsync_exit_ok(code: i32) -> bool {
    matches!(code, 0 | 23 | 24)
}

#[derive(Debug)]
pub struct BackupError(pub String);

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackupError {}

pub fn run_backup(
    cfg: &Config,
    pair: &FolderPair,
    log: &PairLogger,
    cancel: &Arc<AtomicBool>,
) -> Result<String, BackupError> {
    let dest = PathBuf::from(&pair.destination);
    if !dest.is_absolute() {
        return Err(BackupError(format!(
            "destination must be an absolute local path: {}",
            pair.destination
        )));
    }
    if pair.destination.contains(':') && !pair.destination.starts_with('/') {
        return Err(BackupError(
            "destination must be a local directory on this machine".into(),
        ));
    }
    if !dest.exists() {
        return Err(BackupError(format!(
            "destination does not exist: {}",
            dest.display()
        )));
    }
    if !dest.is_dir() {
        return Err(BackupError(format!(
            "destination is not a directory: {}",
            dest.display()
        )));
    }
    let probe = dest.join(".macsync-write-test");
    match fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
        }
        Err(e) => {
            return Err(BackupError(format!(
                "destination not writable by this user: {} ({e})",
                dest.display()
            )));
        }
    }

    let stamp = format_stamp(Local::now());
    let inprogress = prepare_inprogress(&dest, &stamp, log)?;

    let link_dest = latest_snapshot_path(&dest).map(|p| p.to_string_lossy().into_owned());
    if let Some(ref prev) = link_dest {
        log.info(&format!("link-dest: {prev}"));
    } else {
        log.info("link-dest: none (first snapshot or no latest)");
    }

    let args = backup_arguments(&BackupArgParams {
        source: &pair.source,
        target: &inprogress.to_string_lossy(),
        excludes: &pair.excludes,
        read_only: pair.read_only,
        link_dest: link_dest.as_deref(),
        ssh: &cfg.ssh,
        remote_rsync_path: &cfg.remote_rsync_path,
    });

    log.info(&format!(
        "Backup starting for '{}' → {}",
        pair.name,
        dest.display()
    ));
    log.info(&format!(
        "rsync {} {}",
        cfg.rsync_path,
        display_arguments(&args)
    ));

    let started = Instant::now();
    let (status, stats_summary) = run_rsync_streaming(&cfg.rsync_path, &args, log, cancel)?;
    let elapsed = started.elapsed();
    if cancel.load(Ordering::SeqCst) {
        // Keep .inprogress so the next run can resume into it.
        log.warn("backup cancelled; leaving incomplete snapshot for resume");
        return Err(BackupError("backup cancelled".into()));
    }
    if !rsync_exit_ok(status) {
        // Keep .inprogress for resume on the next attempt.
        log.error(&format!(
            "rsync exited {status} ({}); incomplete snapshot kept at {}",
            rsync_exit_meaning(status),
            inprogress.display()
        ));
        return Err(BackupError(format!(
            "rsync exited {status} ({})",
            rsync_exit_meaning(status)
        )));
    }
    if status != 0 {
        log.warn(&format!(
            "rsync exited {status} ({}); finalizing snapshot anyway (files may reappear next run)",
            rsync_exit_meaning(status)
        ));
    }

    let final_path = dest.join(&stamp);
    if final_path.exists() {
        return Err(BackupError(format!(
            "snapshot already exists: {}",
            final_path.display()
        )));
    }
    fs::rename(&inprogress, &final_path).map_err(|e| {
        BackupError(format!(
            "could not rename {} → {}: {e}",
            inprogress.display(),
            final_path.display()
        ))
    })?;

    if pair.read_only {
        if let Err(e) = make_snapshot_read_only(&final_path) {
            log.warn(&format!("could not make snapshot read-only: {e}"));
        }
    }

    ensure_relative_latest(&dest, &stamp)
        .map_err(|e| BackupError(format!("could not update latest symlink: {e}")))?;

    match apply_retention(&dest, &pair.retention) {
        Ok(removed) => {
            for name in removed {
                log.info(&format!("Retention: removed {name}"));
            }
        }
        Err(e) => log.warn(&format!("Retention failed: {e}")),
    }

    log.info(&format!(
        "Backup finished: {stamp} in {} — {stats_summary}",
        format_duration(elapsed)
    ));
    Ok(stamp)
}

/// Reuse the newest `.inprogress-*` if any (resume), else create a fresh one.
/// Older incomplete dirs are removed so only one resume target remains.
fn prepare_inprogress(
    dest: &Path,
    stamp: &str,
    log: &PairLogger,
) -> Result<PathBuf, BackupError> {
    let target_name = format!("{INPROGRESS_PREFIX}{stamp}");
    let target = dest.join(&target_name);

    let mut pending: Vec<(String, Option<chrono::NaiveDateTime>)> = Vec::new();
    if let Ok(entries) = fs::read_dir(dest) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(INPROGRESS_PREFIX) {
                continue;
            }
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_symlink() || !ft.is_dir() {
                continue;
            }
            let stamp_part = name[INPROGRESS_PREFIX.len()..].to_string();
            let parsed = parse_stamp(&stamp_part);
            pending.push((name, parsed));
        }
    }

    // Newest stamp first; unparsable names sort last.
    pending.sort_by(|a, b| match (&a.1, &b.1) {
        (Some(ta), Some(tb)) => tb.cmp(ta),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.0.cmp(&a.0),
    });

    if let Some((reuse_name, _)) = pending.first() {
        let reuse_path = dest.join(reuse_name);
        if reuse_path != target {
            log.info(&format!(
                "Resuming incomplete snapshot {reuse_name} → {target_name}"
            ));
            fs::rename(&reuse_path, &target).map_err(|e| {
                BackupError(format!(
                    "could not rename {} → {}: {e}",
                    reuse_path.display(),
                    target.display()
                ))
            })?;
        } else {
            log.info(&format!("Resuming incomplete snapshot {target_name}"));
        }
        for (old_name, _) in pending.iter().skip(1) {
            let old = dest.join(old_name);
            log.info(&format!("Removing older incomplete snapshot {old_name}"));
            if let Err(e) = fs::remove_dir_all(&old) {
                log.warn(&format!("could not remove {}: {e}", old.display()));
            }
        }
        return Ok(target);
    }

    fs::create_dir_all(&target).map_err(|e| {
        BackupError(format!("could not create {}: {e}", target.display()))
    })?;
    Ok(target)
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Read rsync stdout/stderr, splitting on both `\n` and `\r` so progress2 updates are seen.
fn run_rsync_streaming(
    rsync_path: &str,
    args: &[String],
    log: &PairLogger,
    cancel: &Arc<AtomicBool>,
) -> Result<(i32, String), BackupError> {
    let mut child = Command::new(rsync_path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| BackupError(format!("could not start rsync: {e}")))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let log_path = log.path().to_path_buf();
    let stats_lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let last_progress_log = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(10)));
    let last_progress: Arc<Mutex<Option<TransferProgress>>> = Arc::new(Mutex::new(None));

    let make_reader = |
        mut stream: Box<dyn Read + Send>,
        log_path: PathBuf,
        stats_lines: Arc<Mutex<Vec<String>>>,
        last_progress_log: Arc<Mutex<Instant>>,
        last_progress: Arc<Mutex<Option<TransferProgress>>>,
    | {
        thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            let mut pending: Vec<u8> = Vec::new();
            loop {
                let n = match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                for &byte in &buf[..n] {
                    if byte == b'\n' || byte == b'\r' {
                        if pending.is_empty() {
                            continue;
                        }
                        let line = String::from_utf8_lossy(&pending).into_owned();
                        pending.clear();
                        handle_rsync_line(
                            &line,
                            &log_path,
                            &stats_lines,
                            &last_progress_log,
                            &last_progress,
                        );
                    } else {
                        pending.push(byte);
                    }
                }
            }
            if !pending.is_empty() {
                let line = String::from_utf8_lossy(&pending).into_owned();
                handle_rsync_line(
                    &line,
                    &log_path,
                    &stats_lines,
                    &last_progress_log,
                    &last_progress,
                );
            }
        })
    };

    let out_handle = if let Some(out) = stdout {
        Some(make_reader(
            Box::new(out),
            log_path.clone(),
            stats_lines.clone(),
            last_progress_log.clone(),
            last_progress.clone(),
        ))
    } else {
        None
    };
    let err_handle = if let Some(err) = stderr {
        Some(make_reader(
            Box::new(err),
            log_path.clone(),
            stats_lines.clone(),
            last_progress_log.clone(),
            last_progress.clone(),
        ))
    } else {
        None
    };

    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            break;
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => thread::sleep(Duration::from_millis(200)),
            Err(e) => return Err(BackupError(format!("wait failed: {e}"))),
        }
    }

    let status = child
        .wait()
        .map_err(|e| BackupError(format!("wait failed: {e}")))?;
    if let Some(h) = out_handle {
        let _ = h.join();
    }
    if let Some(h) = err_handle {
        let _ = h.join();
    }

    // Log final progress once more if we have it
    if let Ok(guard) = last_progress.lock() {
        if let Some(ref p) = *guard {
            let line = p.summary_line();
            append_line(&log_path, &line);
            println!("{line}");
            let _ = std::io::stdout().flush();
        }
    }

    let stats = stats_lines.lock().unwrap().clone();
    let summary = summarize_stats(&stats);
    if summary != "Finished" || !stats.is_empty() {
        for line in &stats {
            // Already logged individually if they looked like normal lines; ensure summary is clear
            let _ = line;
        }
        log.info(&format!("rsync stats: {summary}"));
    }

    Ok((status.code().unwrap_or(-1), summary))
}

fn handle_rsync_line(
    line: &str,
    log_path: &Path,
    stats_lines: &Mutex<Vec<String>>,
    last_progress_log: &Mutex<Instant>,
    last_progress: &Mutex<Option<TransferProgress>>,
) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Some(progress) = parse_progress(trimmed) {
        if let Ok(mut g) = last_progress.lock() {
            *g = Some(progress.clone());
        }
        // Throttle log noise: at most every 3s (progress2 updates many times/sec via \r).
        let should_log = last_progress_log
            .lock()
            .map(|mut t| {
                if t.elapsed() >= Duration::from_secs(3) {
                    *t = Instant::now();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if should_log {
            let msg = progress.summary_line();
            append_line(log_path, &msg);
            println!("{msg}");
            let _ = std::io::stdout().flush();
        }
        return;
    }
    if is_stats_line(trimmed) {
        if let Ok(mut s) = stats_lines.lock() {
            s.push(trimmed.to_string());
        }
        append_line(log_path, trimmed);
        println!("{trimmed}");
        return;
    }
    // Other rsync chatter (errors, warnings)
    append_line(log_path, &format!("rsync: {trimmed}"));
    eprintln!("rsync: {trimmed}");
}
