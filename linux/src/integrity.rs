use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use crate::config::{Config, FolderPair};
use crate::logutil::{append_line, PairLogger};
use crate::retention::latest_snapshot_name;
use crate::rsync::{
    display_arguments, integrity_arguments, rsync_exit_meaning, IntegrityArgParams,
};

#[derive(Debug)]
pub struct IntegrityError(pub String);

impl std::fmt::Display for IntegrityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for IntegrityError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrityOutcome {
    Success,
    Warning,
    Failed,
}

#[derive(Debug, Clone)]
pub struct IntegrityResult {
    pub outcome: IntegrityOutcome,
    pub message: String,
    #[allow(dead_code)]
    pub snapshot: String,
}

#[derive(Debug, Default)]
pub(crate) struct Differences {
    silent: Vec<String>,
    changed: Vec<String>,
    missing: Vec<String>,
    extra: Vec<String>,
    #[allow(dead_code)]
    metadata_only: usize,
}

pub fn classify_itemized(lines: &[String]) -> Differences {
    let mut differences = Differences::default();
    for line in lines {
        let Some((item, name)) = line.split_once('|') else {
            continue;
        };
        let item = item.trim();
        let name = name.to_string();
        if item.starts_with("*deleting") {
            if !name.ends_with('/') {
                differences.extra.push(name);
            }
            continue;
        }
        let flags: Vec<char> = item.chars().collect();
        if flags.len() < 5 || flags[1] == 'd' {
            continue;
        }
        if flags[2] == '+' && flags[3] == '+' && flags[4] == '+' {
            differences.missing.push(name);
        } else if flags[0] == '>' || flags[0] == 'c' {
            if flags[2] == 'c' && flags[3] == '.' && flags[4] == '.' {
                differences.silent.push(name);
            } else {
                differences.changed.push(name);
            }
        } else {
            differences.metadata_only += 1;
        }
    }
    differences
}

fn record_from(differences: Differences, snapshot: &str) -> IntegrityResult {
    if !differences.silent.is_empty() {
        IntegrityResult {
            outcome: IntegrityOutcome::Failed,
            message: format!(
                "{} file(s) differ with the same size and date: possible corruption or tampering",
                differences.silent.len()
            ),
            snapshot: snapshot.to_string(),
        }
    } else if differences.changed.is_empty()
        && differences.missing.is_empty()
        && differences.extra.is_empty()
    {
        IntegrityResult {
            outcome: IntegrityOutcome::Success,
            message: "Latest snapshot matches the source byte for byte".into(),
            snapshot: snapshot.to_string(),
        }
    } else {
        IntegrityResult {
            outcome: IntegrityOutcome::Warning,
            message: format!(
                "Source changed since the snapshot: {} edited, {} new, {} deleted",
                differences.changed.len(),
                differences.missing.len(),
                differences.extra.len()
            ),
            snapshot: snapshot.to_string(),
        }
    }
}

pub fn run_integrity(
    cfg: &Config,
    pair: &FolderPair,
    log: &PairLogger,
    cancel: &Arc<AtomicBool>,
) -> Result<IntegrityResult, IntegrityError> {
    let dest = PathBuf::from(&pair.destination);
    let snapshot = latest_snapshot_name(&dest)
        .ok_or_else(|| IntegrityError("No snapshot to check yet".into()))?;
    let snapshot_path = dest.join(&snapshot);

    let args = integrity_arguments(&IntegrityArgParams {
        source: &pair.source,
        snapshot: &snapshot_path.to_string_lossy(),
        excludes: &pair.excludes,
        read_only: pair.read_only,
        ssh: &cfg.ssh,
        remote_rsync_path: &cfg.remote_rsync_path,
    });

    log.info(&format!(
        "Integrity check starting for '{}' (snapshot {snapshot})",
        pair.name
    ));
    log.info(&format!(
        "rsync {} {}",
        cfg.rsync_path,
        display_arguments(&args)
    ));

    let mut child = Command::new(&cfg.rsync_path)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| IntegrityError(format!("could not start rsync: {e}")))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let lines_handle = thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(out) = stdout {
            let reader = BufReader::new(out);
            for line in reader.lines().flatten() {
                if line.contains('|') {
                    lines.push(line);
                }
            }
        }
        lines
    });

    let log_path = log.path().to_path_buf();
    let err_handle = thread::spawn(move || {
        if let Some(err) = stderr {
            let reader = BufReader::new(err);
            for line in reader.lines().flatten() {
                if !line.trim().is_empty() {
                    append_line(&log_path, &format!("rsync: {line}"));
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
            Err(e) => return Err(IntegrityError(format!("wait failed: {e}"))),
        }
    }

    let status = child
        .wait()
        .map_err(|e| IntegrityError(format!("wait failed: {e}")))?;
    let lines = lines_handle
        .join()
        .map_err(|_| IntegrityError("stdout thread panicked".into()))?;
    let _ = err_handle.join();

    if cancel.load(Ordering::SeqCst) {
        return Err(IntegrityError("integrity check cancelled".into()));
    }

    let code = status.code().unwrap_or(-1);
    if code != 0 && code != 23 && code != 24 {
        return Err(IntegrityError(format!(
            "rsync exited {code} ({})",
            rsync_exit_meaning(code)
        )));
    }

    let result = record_from(classify_itemized(&lines), &snapshot);
    match result.outcome {
        IntegrityOutcome::Success => log.info(&result.message),
        IntegrityOutcome::Warning => log.warn(&result.message),
        IntegrityOutcome::Failed => log.error(&result.message),
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_silent_corruption() {
        let lines = vec![">fc.......|secret.bin".into()];
        let d = classify_itemized(&lines);
        assert_eq!(d.silent.len(), 1);
    }

    #[test]
    fn classify_changed() {
        let lines = vec![">f.st......|photo.jpg".into()];
        let d = classify_itemized(&lines);
        assert_eq!(d.changed.len(), 1);
    }
}
