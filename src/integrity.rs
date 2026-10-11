use std::fs::File;
use std::io::Write;

use chrono::Local;

use crate::backup::{self, args_display};
use crate::config::Pair;
use crate::paths;
use crate::rsync::{self, CancelHandle, RsyncInfo};
use crate::state::{IntegrityRecord, Outcome};

const LIST_CAP: usize = 200;

#[derive(Debug, Default, PartialEq)]
pub struct Differences {
    pub silent: Vec<String>,
    pub changed: Vec<String>,
    pub missing: Vec<String>,
    pub extra: Vec<String>,
    pub metadata_only: usize,
}

/// Classifies `--out-format=%i|%n` lines (itemized changes `YXcstpoguax|name`).
pub fn classify<S: AsRef<str>>(lines: &[S]) -> Differences {
    let mut d = Differences::default();
    for line in lines {
        let Some((item, name)) = line.as_ref().split_once('|') else {
            continue;
        };
        let item = item.trim();
        let name = name.to_string();
        if item.starts_with("*deleting") {
            if !name.ends_with('/') {
                d.extra.push(name);
            }
            continue;
        }
        let flags: Vec<char> = item.chars().collect();
        if flags.len() < 5 || flags[1] == 'd' {
            continue;
        }
        if flags[2..].starts_with(&['+', '+', '+']) {
            d.missing.push(name);
        } else if flags[0] == '>' || flags[0] == 'c' {
            if flags[2] == 'c' && flags[3] == '.' && flags[4] == '.' {
                d.silent.push(name);
            } else {
                d.changed.push(name);
            }
        } else {
            d.metadata_only += 1;
        }
    }
    d
}

fn capped(mut v: Vec<String>) -> (Vec<String>, usize) {
    let n = v.len();
    v.truncate(LIST_CAP);
    (v, n)
}

pub fn to_record(d: Differences, snapshot: String) -> IntegrityRecord {
    let outcome = if !d.silent.is_empty() {
        Outcome::Failed
    } else if d.changed.is_empty() && d.missing.is_empty() && d.extra.is_empty() {
        Outcome::Success
    } else {
        Outcome::Warning
    };
    let message = match outcome {
        Outcome::Success => "Latest snapshot matches the source byte for byte".to_string(),
        Outcome::Failed => format!(
            "{} file(s) differ with the same size and date: possible corruption or tampering",
            d.silent.len()
        ),
        Outcome::Warning => format!(
            "Source changed since the snapshot: {} edited, {} new, {} deleted",
            d.changed.len(),
            d.missing.len(),
            d.extra.len()
        ),
    };
    let (silent, silent_count) = capped(d.silent);
    let (changed, changed_count) = capped(d.changed);
    let (missing, missing_count) = capped(d.missing);
    let (extra, extra_count) = capped(d.extra);
    IntegrityRecord {
        outcome: Some(outcome),
        message,
        snapshot,
        silent,
        silent_count,
        changed,
        changed_count,
        missing,
        missing_count,
        extra,
        extra_count,
        ..IntegrityRecord::default()
    }
}

/// Compares the current source with the `latest` snapshot by checksum, without writing anything.
pub fn run_integrity(
    pair: &Pair,
    rsync_info: &RsyncInfo,
    log: &mut File,
    cancel: &CancelHandle,
) -> Result<IntegrityRecord, String> {
    let started = Local::now();
    let (source, dest) = paths::check_pair_paths(&pair.source, &pair.destination).map_err(|e| e.to_string())?;
    let snapshot = backup::latest_name(&dest).ok_or("No snapshot to check yet")?;
    let args = rsync::integrity_args(&source, &dest.join(&snapshot), &pair.excludes, pair.read_only);
    let _ = writeln!(log, "Integrity check of {snapshot} against {}", source.display());
    let _ = writeln!(log, "{} {}", rsync_info.path.display(), args_display(&args));

    let output = rsync::run(&rsync_info.path, &args, log, cancel, true, |_| {})?;
    if cancel.is_cancelled() {
        return Err("Cancelled".into());
    }
    let mut record = to_record(classify(&output.stdout_lines), snapshot);
    match output.exit_code {
        0 | 24 => {}
        code => {
            let detail = output.stderr_tail.last().cloned().unwrap_or_default();
            record.outcome = Some(Outcome::Failed);
            record.message = format!(
                "rsync exit {code}: {} {detail}. {}",
                rsync::exit_code_meaning(code),
                record.message
            );
        }
    }
    record.started = Some(started);
    record.finished = Some(Local::now());
    let _ = writeln!(log, "Integrity: {}", record.message);
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_itemized_output() {
        let lines = [
            ">fc........|photos/bitrot.jpg",
            ">fcst......|notes/edited.md",
            ">f.st......|notes/grown.md",
            ">f+++++++++|new.txt",
            "cd+++++++++|newdir/",
            "*deleting  |gone.txt",
            "*deleting  |gonedir/",
            ".f..t......|touched.txt",
            ".d..t......|notes/",
            "cL+++++++++|link",
            "not itemized output",
        ];
        let d = classify(&lines);
        assert_eq!(d.silent, ["photos/bitrot.jpg"]);
        assert_eq!(d.changed, ["notes/edited.md", "notes/grown.md"]);
        assert_eq!(d.missing, ["new.txt", "link"]);
        assert_eq!(d.extra, ["gone.txt"]);
        assert_eq!(d.metadata_only, 1);
    }

    #[test]
    fn clean_vs_differing() {
        let clean = to_record(classify::<&str>(&[]), "s".into());
        assert_eq!(clean.outcome, Some(Outcome::Success));
        let drift = to_record(classify(&[">fcst......|a"]), "s".into());
        assert_eq!(drift.outcome, Some(Outcome::Warning));
        let silent = to_record(classify(&[">fc........|a", ">f+++++++++|b"]), "s".into());
        assert_eq!(silent.outcome, Some(Outcome::Failed));
        assert_eq!((silent.silent_count, silent.missing_count), (1, 1));
    }
}
