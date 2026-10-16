use std::path::Path;
use std::process::Command;

use crate::config::SshConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsyncInfo {
    pub path: String,
    pub version: String,
}

#[derive(Debug)]
pub struct RsyncError(pub String);

impl std::fmt::Display for RsyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RsyncError {}

/// Require GNU rsync 3.1 or newer at `path`. Checked once at startup.
pub fn check_rsync(path: &str) -> Result<RsyncInfo, RsyncError> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(RsyncError(format!("rsync not found at {path}")));
    }
    let output = Command::new(path)
        .arg("--version")
        .output()
        .map_err(|e| RsyncError(format!("could not run {path}: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = parse_rsync_version(&text)
        .map_err(|e| RsyncError(format!("{path}: {e}")))?;
    let (major, minor) = parse_version_tuple(&version)
        .ok_or_else(|| RsyncError(format!("{path}: could not parse version '{version}'")))?;
    if major < 3 || (major == 3 && minor < 1) {
        return Err(RsyncError(format!(
            "{path}: need GNU rsync 3.1 or newer (found {version})"
        )));
    }
    Ok(RsyncInfo {
        path: path.to_string(),
        version,
    })
}

pub fn parse_rsync_version(text: &str) -> Result<String, String> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("openrsync") {
        return Err("openrsync is not supported; install GNU rsync".into());
    }
    // "rsync  version 3.2.7  protocol version 31"
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("rsync") {
            let rest = rest.trim_start();
            let rest = rest.strip_prefix("version").unwrap_or(rest).trim_start();
            let ver: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if !ver.is_empty() {
                return Ok(ver);
            }
        }
    }
    Err("could not parse rsync --version output".into())
}

fn parse_version_tuple(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().unwrap_or(0);
    Some((major, minor))
}

pub fn with_trailing_slash(path: &str) -> String {
    if path.ends_with('/') {
        path.to_string()
    } else {
        format!("{path}/")
    }
}

pub fn build_ssh_command(ssh: &SshConfig) -> String {
    // Prefer ~/.ssh/config for Host aliases (HostName, Port, IdentityFile, known_hosts).
    // Only pass -i / -p when the config file actually overrides the OpenSSH defaults.
    let mut parts = vec!["ssh".to_string()];
    if !ssh.identity_file.is_empty() {
        parts.push("-i".into());
        parts.push(ssh.identity_file.clone());
    }
    if ssh.port != 22 {
        parts.push("-p".into());
        parts.push(ssh.port.to_string());
    }
    // No prompts under systemd / BatchMode; host must already be in known_hosts
    // (same names as in `source`, e.g. "mini").
    parts.push("-o".into());
    parts.push("BatchMode=yes".into());
    for arg in &ssh.extra_args {
        parts.push(arg.clone());
    }
    shell_join(&parts)
}

fn shell_join(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| {
            if p.chars().any(|c| c.is_whitespace() || "\"'\\$`".contains(c)) {
                format!("'{}'", p.replace('\'', "'\\''"))
            } else {
                p.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone)]
pub struct BackupArgParams<'a> {
    pub source: &'a str,
    pub target: &'a str,
    pub excludes: &'a [String],
    pub read_only: bool,
    pub link_dest: Option<&'a str>,
    pub ssh: &'a SshConfig,
    pub remote_rsync_path: &'a str,
}

pub fn backup_arguments(p: &BackupArgParams<'_>) -> Vec<String> {
    let mut args = vec![
        "-aHAX".into(),
        "--delete".into(),
        "--delete-excluded".into(),
        "--info=stats2,progress2".into(),
        "-e".into(),
        build_ssh_command(p.ssh),
    ];
    append_remote_rsync_path(&mut args, p.remote_rsync_path);
    if p.read_only {
        args.push("--chmod=Fa-w".into());
    }
    append_excludes(&mut args, p.excludes);
    if let Some(link) = p.link_dest {
        args.push(format!("--link-dest={link}"));
    }
    args.push(with_trailing_slash(p.source));
    args.push(with_trailing_slash(p.target));
    args
}

#[derive(Debug, Clone)]
pub struct IntegrityArgParams<'a> {
    pub source: &'a str,
    pub snapshot: &'a str,
    pub excludes: &'a [String],
    pub read_only: bool,
    pub ssh: &'a SshConfig,
    pub remote_rsync_path: &'a str,
}

pub fn integrity_arguments(p: &IntegrityArgParams<'_>) -> Vec<String> {
    let mut args = vec![
        "-aHAX".into(),
        "--dry-run".into(),
        "--checksum".into(),
        "--delete".into(),
        "--out-format=%i|%n".into(),
        "-e".into(),
        build_ssh_command(p.ssh),
    ];
    append_remote_rsync_path(&mut args, p.remote_rsync_path);
    if p.read_only {
        args.push("--chmod=Fa-w".into());
    }
    append_excludes(&mut args, p.excludes);
    args.push(with_trailing_slash(p.source));
    args.push(with_trailing_slash(p.snapshot));
    args
}

fn append_remote_rsync_path(args: &mut Vec<String>, remote: &str) {
    let remote = remote.trim();
    if !remote.is_empty() {
        args.push(format!("--rsync-path={remote}"));
    }
}

fn append_excludes(args: &mut Vec<String>, excludes: &[String]) {
    for pattern in excludes {
        let pattern = pattern.trim();
        if !pattern.is_empty() {
            args.push(format!("--exclude={pattern}"));
        }
    }
}

pub fn display_arguments(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(' ') {
                format!("'{a}'")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn rsync_exit_meaning(code: i32) -> &'static str {
    match code {
        0 => "success",
        1 => "syntax or usage error",
        2 => "protocol incompatibility",
        3 => "errors selecting input/output files or folders",
        11 => "error in file I/O (disk full?)",
        12 => "error in rsync data stream",
        20 => "stopped",
        23 => "some files could not be copied",
        24 => "some source files vanished during the copy",
        30 => "timeout",
        _ => "rsync error",
    }
}

/// Parse a progress2 line for logging (bytes, rate, ETA).
pub fn parse_progress_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // Typical: "  1,234,567  45%  12.34MB/s    0:01:23  (xfr#12, ir-chk=1000/2000)"
    if !line.contains('%') && !line.contains("/s") {
        return None;
    }
    Some(line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parse() {
        let v = parse_rsync_version("rsync  version 3.2.7  protocol version 31\n").unwrap();
        assert_eq!(v, "3.2.7");
        assert!(parse_rsync_version("openrsync: protocol version 33").is_err());
    }

    #[test]
    fn backup_argv_has_ssh_and_link_dest() {
        let ssh = SshConfig {
            port: 2222,
            identity_file: "/home/me/.ssh/id_ed25519".into(),
            extra_args: vec!["-o".into(), "StrictHostKeyChecking=yes".into()],
        };
        let excludes = vec![".DS_Store".into()];
        let args = backup_arguments(&BackupArgParams {
            source: "backup@nas:/volume1/photos",
            target: "/home/me/backups/photos/.inprogress-2024-01-01T000000",
            excludes: &excludes,
            read_only: true,
            link_dest: Some("/home/me/backups/photos/2023-12-31T000000"),
            ssh: &ssh,
            remote_rsync_path: "/opt/homebrew/bin/rsync",
        });
        assert_eq!(args[0], "-aHAX");
        assert!(args.contains(&"--delete".to_string()));
        assert!(args.contains(&"--chmod=Fa-w".to_string()));
        assert!(args.contains(&"--exclude=.DS_Store".to_string()));
        assert!(args.contains(&"--rsync-path=/opt/homebrew/bin/rsync".to_string()));
        let e_idx = args.iter().position(|a| a == "-e").unwrap();
        assert!(args[e_idx + 1].contains("ssh"));
        assert!(args[e_idx + 1].contains("-p 2222"));
        assert!(args[e_idx + 1].contains("-i"));
        assert!(args.iter().any(|a| a.starts_with("--link-dest=")));
        assert!(args
            .iter()
            .any(|a| a == "backup@nas:/volume1/photos/"));
        assert!(args.iter().any(|a| a.ends_with("/.inprogress-2024-01-01T000000/")));
    }

    #[test]
    fn integrity_argv_dry_run_checksum() {
        let ssh = SshConfig::default();
        let args = integrity_arguments(&IntegrityArgParams {
            source: "u@h:/src",
            snapshot: "/dest/2024-01-01T000000",
            excludes: &[],
            read_only: false,
            ssh: &ssh,
            remote_rsync_path: "",
        });
        assert!(args.contains(&"--dry-run".to_string()));
        assert!(args.contains(&"--checksum".to_string()));
        assert!(args.contains(&"--out-format=%i|%n".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--link-dest")));
        assert!(!args.iter().any(|a| a.starts_with("--rsync-path=")));
    }

    #[test]
    fn default_ssh_defers_to_ssh_config() {
        let cmd = build_ssh_command(&SshConfig::default());
        assert_eq!(cmd, "ssh -o BatchMode=yes");
        assert!(!cmd.contains("-i"));
        assert!(!cmd.contains("-p"));
    }
}
