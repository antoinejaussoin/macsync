use chrono::Local;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use crate::config::{Config, FolderPair};
use crate::logutil::{append_line, PairLogger};
use crate::retention::{
    apply_retention, ensure_relative_latest, format_stamp, latest_snapshot_path,
    make_snapshot_read_only,
};
use crate::rsync::{
    backup_arguments, display_arguments, parse_progress_line, rsync_exit_meaning, BackupArgParams,
};

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
    let inprogress_name = format!(".inprogress-{stamp}");
    let inprogress = dest.join(&inprogress_name);
    if inprogress.exists() {
        return Err(BackupError(format!(
            "in-progress directory already exists: {}",
            inprogress.display()
        )));
    }
    fs::create_dir_all(&inprogress).map_err(|e| {
        BackupError(format!("could not create {}: {e}", inprogress.display()))
    })?;

    let link_dest = latest_snapshot_path(&dest).map(|p| p.to_string_lossy().into_owned());

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

    let status = run_rsync_streaming(&cfg.rsync_path, &args, log, cancel)?;
    if cancel.load(Ordering::SeqCst) {
        let _ = remove_inprogress(&inprogress);
        return Err(BackupError("backup cancelled".into()));
    }
    if status != 0 {
        let _ = remove_inprogress(&inprogress);
        return Err(BackupError(format!(
            "rsync exited {status} ({})",
            rsync_exit_meaning(status)
        )));
    }

    let final_path = dest.join(&stamp);
    if final_path.exists() {
        let _ = remove_inprogress(&inprogress);
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

    log.info(&format!("Backup finished: {stamp}"));
    Ok(stamp)
}

fn remove_inprogress(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn run_rsync_streaming(
    rsync_path: &str,
    args: &[String],
    log: &PairLogger,
    cancel: &Arc<AtomicBool>,
) -> Result<i32, BackupError> {
    let mut child = Command::new(rsync_path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| BackupError(format!("could not start rsync: {e}")))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let log_path_out = log.path().to_path_buf();
    let log_path_err = log.path().to_path_buf();

    let out_handle = thread::spawn(move || {
        if let Some(out) = stdout {
            let reader = BufReader::new(out);
            let mut last_progress = String::new();
            for line in reader.lines().flatten() {
                if let Some(progress) = parse_progress_line(&line) {
                    if progress != last_progress {
                        append_line(&log_path_out, &format!("progress: {progress}"));
                        println!("progress: {progress}");
                        last_progress = progress;
                    }
                } else if !line.trim().is_empty() {
                    append_line(&log_path_out, &line);
                    println!("{line}");
                }
            }
        }
    });

    let err_handle = thread::spawn(move || {
        if let Some(err) = stderr {
            let reader = BufReader::new(err);
            for line in reader.lines().flatten() {
                if !line.trim().is_empty() {
                    append_line(&log_path_err, &format!("rsync: {line}"));
                    eprintln!("rsync: {line}");
                }
            }
        }
    });

    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            break;
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => thread::sleep(std::time::Duration::from_millis(200)),
            Err(e) => return Err(BackupError(format!("wait failed: {e}"))),
        }
    }

    let status = child
        .wait()
        .map_err(|e| BackupError(format!("wait failed: {e}")))?;
    let _ = out_handle.join();
    let _ = err_handle.join();
    Ok(status.code().unwrap_or(-1))
}
