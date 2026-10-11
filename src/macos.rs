use std::sync::atomic::{AtomicBool, Ordering};

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;

static SHOW_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Asks the (possibly hidden) window to come forward. Safe from any thread:
/// eframe keeps calling `App::logic` while hidden once a repaint is requested.
pub fn request_show(ctx: &eframe::egui::Context) {
    SHOW_REQUESTED.store(true, Ordering::SeqCst);
    ctx.request_repaint();
}

pub fn take_show_request() -> bool {
    SHOW_REQUESTED.swap(false, Ordering::SeqCst)
}

/// Accessory apps are not activated by clicking a menu-bar item, so the window
/// would otherwise open behind the frontmost app.
pub fn activate_app() {
    match MainThreadMarker::new() {
        // `activate` only exists on macOS 14+; this works on 13 and still behaves on newer releases.
        #[allow(deprecated)]
        Some(mtm) => NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true),
        None => on_main(activate_app),
    }
}

pub fn on_main(f: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(f);
}

pub fn open(target: impl AsRef<std::ffi::OsStr>) {
    let _ = std::process::Command::new("/usr/bin/open").arg(target).spawn();
}

pub fn reveal_in_finder(path: &std::path::Path) {
    let _ = std::process::Command::new("/usr/bin/open").arg("-R").arg(path).spawn();
}

pub const FULL_DISK_ACCESS_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";
