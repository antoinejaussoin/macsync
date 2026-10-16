use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use chrono::Local;

pub struct PairLogger {
    path: std::path::PathBuf,
    file: Mutex<std::fs::File>,
}

impl PairLogger {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Mutex::new(file),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn info(&self, msg: &str) {
        self.write("INFO", msg);
    }

    pub fn warn(&self, msg: &str) {
        self.write("WARN", msg);
    }

    pub fn error(&self, msg: &str) {
        self.write("ERROR", msg);
    }

    fn write(&self, level: &str, msg: &str) {
        let line = format!(
            "{} [{level}] {msg}\n",
            Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
        // Also mirror to stdout for journald.
        print!("{line}");
        let _ = std::io::stdout().flush();
    }
}

pub fn ensure_data_dirs() -> std::io::Result<()> {
    fs::create_dir_all(crate::paths::data_dir())?;
    fs::create_dir_all(crate::paths::logs_dir())?;
    Ok(())
}

pub fn append_line(path: &Path, msg: &str) {
    let line = format!(
        "{} [INFO] {msg}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}
