use std::fs;
use std::path::{Path, PathBuf};

use crate::paths;

pub const LABEL: &str = "com.macsync.agent";
pub const BACKGROUND_ARG: &str = "--background";

pub fn plist_path() -> PathBuf {
    paths::home()
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

pub fn is_enabled() -> bool {
    plist_path().exists()
}

/// launchd reads `~/Library/LaunchAgents` at login, so writing the plist is enough.
/// Loading it now with `launchctl bootstrap` would start a second copy, and
/// `bootout` would kill this one if launchd started it.
pub fn enable() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Could not find the app binary: {e}"))?;
    let path = plist_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    fs::write(&path, plist_xml(&exe)).map_err(|e| format!("Could not write {}: {e}", path.display()))
}

pub fn disable() -> Result<(), String> {
    match fs::remove_file(plist_path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Could not remove the login item: {e}")),
    }
}

/// The login item points at whatever binary is running now; that should be the
/// copy inside MacSync.app, not a `target/` build that `cargo clean` deletes.
pub fn running_from_bundle() -> bool {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
        .unwrap_or(false)
}

pub fn plist_xml(exe: &Path) -> String {
    let exe = xml_escape(&exe.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>{BACKGROUND_ARG}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
</dict>
</plist>
"#
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_escapes_path_and_starts_in_background() {
        let xml = plist_xml(Path::new("/Applications/Mac & Sync.app/Contents/MacOS/macsync"));
        assert!(xml.contains("<string>/Applications/Mac &amp; Sync.app/Contents/MacOS/macsync</string>"));
        assert!(xml.contains("<string>--background</string>"));
        assert!(xml.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(!xml.contains("KeepAlive"));
    }

    #[test]
    fn plist_is_valid_for_launchd() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent.plist");
        fs::write(
            &path,
            plist_xml(Path::new("/Applications/MacSync.app/Contents/MacOS/MacSync")),
        )
        .unwrap();
        let status = std::process::Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&path)
            .output()
            .unwrap();
        assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stdout));
    }
}
