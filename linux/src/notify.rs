use chrono::NaiveDateTime;
use std::path::Path;
use std::process::Command;

use crate::config::NotifyConfig;
use crate::schedule::now_local;
use crate::state::{AppState, PairState};

const NTFY_BASE: &str = "https://ntfy.sh/";

pub fn topic_url(topic: &str) -> Option<String> {
    let topic = topic.trim();
    if topic.is_empty() {
        return None;
    }
    if topic.starts_with("http://") || topic.starts_with("https://") {
        return Some(topic.trim_end_matches('/').to_string());
    }
    Some(format!("{NTFY_BASE}{}", topic.trim_start_matches('/')))
}

/// POST to ntfy via `curl` (always present on Ubuntu; avoids rustls cross-compile).
pub fn publish(cfg: &NotifyConfig, title: &str, body: &str, tags: &str) -> Result<(), String> {
    let url = topic_url(&cfg.topic).ok_or_else(|| "notify.topic is empty".to_string())?;
    let mut cmd = Command::new("curl");
    cmd.args([
        "-fsS",
        "-X",
        "POST",
        "-H",
        &format!("Title: {title}"),
        "-H",
        &format!("Tags: {tags}"),
        "--max-time",
        "15",
        "-d",
        body,
    ]);
    if !cfg.token.trim().is_empty() {
        cmd.args(["-H", &format!("Authorization: Bearer {}", cfg.token.trim())]);
    }
    cmd.arg(&url);
    let output = cmd
        .output()
        .map_err(|e| format!("could not run curl (install curl): {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "curl failed ({}): {}",
            output.status,
            stderr.trim()
        ))
    }
}

pub fn test_message() -> (String, String) {
    let host = hostname();
    (
        "macsync test".into(),
        format!("Notifications are working (from {host})."),
    )
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut _, buf.len()) };
    if rc != 0 {
        return "unknown".into();
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

/// Destination must exist as a directory. It may be a mount point or a folder
/// on a mounted volume (e.g. `/mnt/backup/sync` when `/mnt/backup` is the mount).
/// When the volume is unplugged the path typically disappears.
pub fn destination_ok(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("{} does not exist (unmounted?)", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok(())
}

/// Whether we should send a mount alert now (first time or past remind window).
pub fn should_mount_alert(pair: &PairState, remind_hours: u32) -> bool {
    let Some(ref last) = pair.last_mount_alert else {
        return true;
    };
    let Some(prev) = NaiveDateTime::parse_from_str(last, "%Y-%m-%dT%H:%M:%S").ok() else {
        return true;
    };
    let now = now_local();
    let hours = chrono::Duration::hours(i64::from(remind_hours.max(1)));
    now - prev >= hours
}

pub fn mark_mount_alert(state: &mut AppState, pair_id: &str) {
    let p = state.pair_mut(pair_id);
    p.last_mount_alert = Some(now_local().format("%Y-%m-%dT%H:%M:%S").to_string());
}

pub fn clear_mount_alert(state: &mut AppState, pair_id: &str) {
    state.pair_mut(pair_id).last_mount_alert = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_url_forms() {
        assert_eq!(
            topic_url("https://ntfy.sh/abc").unwrap(),
            "https://ntfy.sh/abc"
        );
        assert_eq!(topic_url("my-topic").unwrap(), "https://ntfy.sh/my-topic");
        assert!(topic_url("").is_none());
        assert!(topic_url("   ").is_none());
    }
}
