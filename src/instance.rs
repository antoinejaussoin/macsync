use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::OnceLock;

use crate::paths;

static ON_SHOW: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Held for the life of the process; a second copy of the app would run a second scheduler.
pub struct Instance {
    _lock: File,
}

/// Returns `None` when another copy is already running. A copy started by the user
/// (not at login) asks the running one to open its window before exiting.
pub fn acquire(background: bool) -> Option<Instance> {
    let dir = paths::app_support_dir();
    let _ = fs::create_dir_all(&dir);
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("macsync.lock"))
        .ok()?;
    let socket_path = dir.join("macsync.sock");
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if !background && let Ok(mut stream) = UnixStream::connect(&socket_path) {
            let _ = stream.write_all(b"show");
        }
        return None;
    }

    let _ = fs::remove_file(&socket_path);
    if let Ok(listener) = UnixListener::bind(&socket_path) {
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut msg = String::new();
                let _ = stream.read_to_string(&mut msg);
                if msg == "show"
                    && let Some(f) = ON_SHOW.get()
                {
                    f();
                }
            }
        });
    }
    Some(Instance { _lock: lock })
}

pub fn set_on_show(f: impl Fn() + Send + Sync + 'static) {
    let _ = ON_SHOW.set(Box::new(f));
}
