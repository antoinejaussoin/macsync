use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

pub const HOMEBREW_CANDIDATES: [&str; 2] = ["/opt/homebrew/bin/rsync", "/usr/local/bin/rsync"];
const SYSTEM_RSYNC: &str = "/usr/bin/rsync";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsyncInfo {
    pub path: PathBuf,
    pub version: String,
}

pub fn locate(override_path: Option<&Path>) -> Result<RsyncInfo, String> {
    let candidates: Vec<PathBuf> = match override_path {
        Some(p) if !p.as_os_str().is_empty() => vec![p.to_path_buf()],
        _ => HOMEBREW_CANDIDATES.iter().map(PathBuf::from).collect(),
    };
    let mut problems = Vec::new();
    for path in candidates {
        if path == Path::new(SYSTEM_RSYNC) {
            problems.push(format!("{SYSTEM_RSYNC} is Apple's openrsync, which is not supported"));
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let output = Command::new(&path)
            .arg("--version")
            .output()
            .map_err(|e| format!("Could not run {}: {e}", path.display()))?;
        let text = String::from_utf8_lossy(&output.stdout);
        match parse_version(&text) {
            Ok(version) => return Ok(RsyncInfo { path, version }),
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
    if problems.is_empty() {
        Err("Homebrew rsync not found. Install it with: brew install rsync".into())
    } else {
        Err(format!(
            "{}. Install Homebrew rsync with: brew install rsync",
            problems.join("; ")
        ))
    }
}

/// Accepts `rsync  version 3.x.y  protocol version NN` with x >= 1 (needed for `--info`).
pub fn parse_version(output: &str) -> Result<String, String> {
    let first = output.lines().next().unwrap_or_default();
    let tokens: Vec<&str> = first.split_whitespace().collect();
    if tokens.len() < 3 || tokens[0] != "rsync" || tokens[1] != "version" {
        return Err(format!("not GNU rsync (\"{}\")", first.trim()));
    }
    let version = tokens[2].trim_start_matches('v');
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    if major != 3 || minor < 1 {
        return Err(format!("rsync {version} is too old, 3.1 or newer is required"));
    }
    Ok(version.to_string())
}

pub struct BackupArgs<'a> {
    pub source: &'a Path,
    pub target: &'a Path,
    pub link_dest: Option<&'a Path>,
    pub excludes: &'a [String],
    pub read_only: bool,
}

/// Copying with `--chmod=Fa-w` (instead of chmod-ing afterwards) keeps file modes
/// equal between runs, which `--link-dest` requires before it will hard-link.
pub fn backup_args(a: &BackupArgs) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-aHAX", "--delete", "--delete-excluded", "--info=stats2,progress2"]
        .into_iter()
        .map(OsString::from)
        .collect();
    if a.read_only {
        args.push("--chmod=Fa-w".into());
    }
    push_excludes(&mut args, a.excludes);
    if let Some(link) = a.link_dest {
        let mut opt = OsString::from("--link-dest=");
        opt.push(link);
        args.push(opt);
    }
    args.push(dir_arg(a.source));
    args.push(dir_arg(a.target));
    args
}

/// Checksum dry run of the source against a snapshot; prints one `%i|%n` line per difference.
pub fn integrity_args(source: &Path, snapshot: &Path, excludes: &[String], read_only: bool) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-aHAX", "--dry-run", "--checksum", "--delete", "--out-format=%i|%n"]
        .into_iter()
        .map(OsString::from)
        .collect();
    if read_only {
        args.push("--chmod=Fa-w".into());
    }
    push_excludes(&mut args, excludes);
    args.push(dir_arg(source));
    args.push(dir_arg(snapshot));
    args
}

fn push_excludes(args: &mut Vec<OsString>, excludes: &[String]) {
    for pattern in excludes.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        args.push(format!("--exclude={pattern}").into());
    }
}

/// Trailing slash: copy the folder's contents, not the folder itself.
fn dir_arg(path: &Path) -> OsString {
    let mut s = path.as_os_str().to_os_string();
    if !s.to_string_lossy().ends_with('/') {
        s.push("/");
    }
    s
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    pub bytes: u64,
    pub percent: u8,
    pub rate: String,
    pub eta: String,
    pub files_transferred: u64,
}

/// Parses a `--info=progress2` line: `  1,234,567  45%   12.34MB/s    0:00:10 (xfr#5, to-chk=10/100)`.
pub fn parse_progress(line: &str) -> Option<Progress> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 4 || !tokens[1].ends_with('%') {
        return None;
    }
    let bytes = tokens[0].replace(',', "").parse().ok()?;
    let percent = tokens[1].trim_end_matches('%').parse().ok()?;
    let files_transferred = tokens
        .iter()
        .find_map(|t| t.strip_prefix("(xfr#"))
        .and_then(|t| t.trim_end_matches(',').parse().ok())
        .unwrap_or(0);
    Some(Progress {
        bytes,
        percent,
        rate: tokens[2].to_string(),
        eta: tokens[3].to_string(),
        files_transferred,
    })
}

/// Process group of the running rsync, so a cancel reaches its forked helpers too.
#[derive(Clone, Default)]
pub struct CancelHandle {
    pgid: Arc<AtomicI32>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl CancelHandle {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let pgid = self.pgid.load(Ordering::SeqCst);
        if pgid > 0 {
            unsafe {
                libc::kill(-pgid, libc::SIGTERM);
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

pub struct RunOutput {
    pub exit_code: i32,
    /// stdout lines other than progress updates (stats, itemized changes).
    pub stdout_lines: Vec<String>,
    pub stderr_tail: Vec<String>,
}

/// Runs rsync without a shell, appending its output to `log`. Progress lines are
/// reported through `on_progress` and kept out of the log.
pub fn run(
    rsync: &Path,
    args: &[OsString],
    log: &mut File,
    cancel: &CancelHandle,
    keep_stdout: bool,
    mut on_progress: impl FnMut(Progress),
) -> Result<RunOutput, String> {
    let mut child = Command::new(rsync)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("Could not start rsync: {e}"))?;
    cancel.pgid.store(child.id() as i32, Ordering::SeqCst);
    if cancel.is_cancelled() {
        cancel.cancel();
    }

    let stderr_log = Arc::new(Mutex::new(log.try_clone().map_err(|e| e.to_string())?));
    let mut stderr = child.stderr.take().expect("stderr piped");
    let stderr_thread = std::thread::spawn(move || {
        let mut tail: Vec<String> = Vec::new();
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        let mut log = stderr_log.lock().unwrap();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let _ = writeln!(log, "{line}");
            tail.push(line.to_string());
            if tail.len() > 20 {
                tail.remove(0);
            }
        }
        tail
    });

    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stdout_lines = Vec::new();
    let mut pending = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    let mut handle_line = |raw: &[u8], log: &mut File, lines: &mut Vec<String>| {
        let line = String::from_utf8_lossy(raw);
        if line.trim().is_empty() {
            return;
        }
        if let Some(p) = parse_progress(&line) {
            on_progress(p);
        } else {
            let _ = writeln!(log, "{line}");
            if keep_stdout {
                lines.push(line.into_owned());
            }
        }
    };
    loop {
        let n = match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                handle_line(&pending, log, &mut stdout_lines);
                pending.clear();
            } else {
                pending.push(b);
            }
        }
    }
    handle_line(&pending, log, &mut stdout_lines);

    let status = child.wait().map_err(|e| format!("rsync did not finish: {e}"))?;
    cancel.pgid.store(0, Ordering::SeqCst);
    let stderr_tail = stderr_thread.join().unwrap_or_default();
    Ok(RunOutput {
        exit_code: status.code().unwrap_or(-1),
        stdout_lines,
        stderr_tail,
    })
}

pub fn exit_code_meaning(code: i32) -> &'static str {
    match code {
        0 => "success",
        1 => "syntax or usage error",
        2 => "protocol incompatibility",
        3 => "errors selecting input/output files or folders",
        11 => "error in file I/O (disk full?)",
        12 => "error in rsync data stream",
        20 => "stopped",
        23 => "some files could not be copied (permissions? Full Disk Access?)",
        24 => "some source files vanished during the copy",
        30 => "timeout",
        _ => "rsync error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_gate() {
        assert_eq!(
            parse_version("rsync  version 3.4.1  protocol version 32\nCopyright").unwrap(),
            "3.4.1"
        );
        assert!(parse_version("openrsync: protocol version 29\nrsync version 2.6.9 compatible").is_err());
        assert!(parse_version("rsync  version 2.6.9  protocol version 29").is_err());
        assert!(parse_version("rsync  version 3.0.9  protocol version 30").is_err());
        assert!(parse_version("").is_err());
    }

    #[test]
    fn system_rsync_is_rejected_even_as_override() {
        let err = locate(Some(Path::new("/usr/bin/rsync"))).unwrap_err();
        assert!(err.contains("openrsync"), "{err}");
    }

    #[test]
    fn backup_argv_first_snapshot() {
        let excludes = vec![".DS_Store".to_string(), "  ".to_string(), "-weird name".to_string()];
        let args = backup_args(&BackupArgs {
            source: Path::new("/Users/a/My Docs"),
            target: Path::new("/Volumes/B/docs/.inprogress-2026-10-11T081300"),
            link_dest: None,
            excludes: &excludes,
            read_only: false,
        });
        let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            [
                "-aHAX",
                "--delete",
                "--delete-excluded",
                "--info=stats2,progress2",
                "--exclude=.DS_Store",
                "--exclude=-weird name",
                "/Users/a/My Docs/",
                "/Volumes/B/docs/.inprogress-2026-10-11T081300/",
            ]
        );
    }

    #[test]
    fn backup_argv_with_link_dest_and_read_only() {
        let args = backup_args(&BackupArgs {
            source: Path::new("/src/"),
            target: Path::new("/dst/.inprogress-x"),
            link_dest: Some(Path::new("/dst/2026-10-10T020000")),
            excludes: &[],
            read_only: true,
        });
        let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.contains(&"--chmod=Fa-w".to_string()));
        assert!(args.contains(&"--link-dest=/dst/2026-10-10T020000".to_string()));
        assert_eq!(args[args.len() - 2], "/src/");
    }

    #[test]
    fn integrity_argv_is_a_dry_run() {
        let args = integrity_args(Path::new("/src"), Path::new("/dst/2026"), &[], true);
        let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.contains(&"--dry-run".to_string()));
        assert!(args.contains(&"--checksum".to_string()));
        assert!(args.contains(&"--chmod=Fa-w".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--link-dest")));
    }

    #[test]
    fn progress_lines() {
        let p = parse_progress("      1,234,567  45%   12.34MB/s    0:00:10 (xfr#5, to-chk=10/100)").unwrap();
        assert_eq!(p.bytes, 1_234_567);
        assert_eq!(p.percent, 45);
        assert_eq!(p.rate, "12.34MB/s");
        assert_eq!(p.files_transferred, 5);
        let p = parse_progress("              0   0%    0.00kB/s    0:00:00 (xfr#0, ir-chk=1000/1031)").unwrap();
        assert_eq!(p.percent, 0);
        assert!(parse_progress("sending incremental file list").is_none());
        assert!(parse_progress("Number of files: 1,031 (reg: 1,000, dir: 31)").is_none());
    }
}
