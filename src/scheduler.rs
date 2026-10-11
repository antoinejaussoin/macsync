use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};

use crate::backup::{self, BackupError};
use crate::config::{Config, Pair};
use crate::integrity;
use crate::paths;
use crate::rsync::{self, CancelHandle, Progress, RsyncInfo};
use crate::schedule;
use crate::state::{IntegrityRecord, Outcome, RunRecord, State};
use crate::stats;

const TICK: Duration = Duration::from_secs(15);
const STATS_MAX_AGE_HOURS: i64 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobKind {
    Backup,
    Integrity,
}

impl JobKind {
    pub fn verb(self) -> &'static str {
        match self {
            JobKind::Backup => "Backing up",
            JobKind::Integrity => "Checking",
        }
    }

    pub fn schedule(self, pair: &Pair) -> &str {
        match self {
            JobKind::Backup => &pair.backup_schedule,
            JobKind::Integrity => &pair.integrity_schedule,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub pair_id: String,
    pub kind: JobKind,
}

#[derive(Debug, Clone)]
pub struct Running {
    pub job: Job,
    pub pair_name: String,
    pub started: DateTime<Local>,
    pub progress: Option<Progress>,
}

pub struct Shared {
    pub config: Config,
    pub state: State,
    pub queue: VecDeque<Job>,
    pub running: Option<Running>,
    pub rsync: Result<RsyncInfo, String>,
    pub stats_busy: HashSet<String>,
    pub waiting_for_destination: HashSet<String>,
    /// Shown once in the UI (for example a config file that could not be read).
    pub notice: Option<String>,
    anchors: HashMap<(String, JobKind), DateTime<Local>>,
}

impl Shared {
    fn last_attempt(&self, pair_id: &str, kind: JobKind) -> Option<DateTime<Local>> {
        let ps = self.state.pairs.get(pair_id)?;
        match kind {
            JobKind::Backup => ps.last_backup.as_ref().map(|r| r.started),
            JobKind::Integrity => ps.last_integrity.as_ref().and_then(|r| r.started),
        }
    }

    fn anchor(&mut self, pair_id: &str, kind: JobKind, now: DateTime<Local>) -> DateTime<Local> {
        match self.last_attempt(pair_id, kind) {
            Some(t) => t,
            None => *self.anchors.entry((pair_id.to_string(), kind)).or_insert(now),
        }
    }

    /// Next scheduled run; a time in the past means the job is due now.
    pub fn next_run(&self, pair: &Pair, kind: JobKind) -> Option<DateTime<Local>> {
        let anchor = self
            .last_attempt(&pair.id, kind)
            .or_else(|| self.anchors.get(&(pair.id.clone(), kind)).copied())
            .unwrap_or_else(Local::now);
        schedule::next_after(kind.schedule(pair), anchor)
    }

    pub fn is_pending(&self, pair_id: &str, kind: JobKind) -> bool {
        let job = Job {
            pair_id: pair_id.to_string(),
            kind,
        };
        self.queue.contains(&job) || self.running.as_ref().is_some_and(|r| r.job == job)
    }

    pub fn is_busy(&self, pair_id: &str) -> bool {
        self.is_pending(pair_id, JobKind::Backup) || self.is_pending(pair_id, JobKind::Integrity)
    }

    fn push(&mut self, pair_id: &str, kind: JobKind) -> bool {
        if self.is_pending(pair_id, kind) {
            return false;
        }
        self.queue.push_back(Job {
            pair_id: pair_id.to_string(),
            kind,
        });
        true
    }

    /// Queues every scheduled job whose fire time has passed. Returns whether anything changed.
    pub fn tick(&mut self, now: DateTime<Local>) -> bool {
        let mut changed = false;
        let pairs: Vec<Pair> = self.config.pairs.iter().filter(|p| p.enabled).cloned().collect();
        for pair in &pairs {
            for kind in [JobKind::Backup, JobKind::Integrity] {
                let expr = kind.schedule(pair).to_string();
                if expr.trim().is_empty() {
                    continue;
                }
                let anchor = self.anchor(&pair.id, kind, now);
                if !schedule::is_due(&expr, anchor, now) || self.is_pending(&pair.id, kind) {
                    continue;
                }
                if !pair.destination.is_dir() {
                    changed |= self.waiting_for_destination.insert(pair.id.clone());
                    continue;
                }
                changed |= self.waiting_for_destination.remove(&pair.id);
                changed |= self.push(&pair.id, kind);
            }
        }
        changed
    }
}

pub struct Engine {
    shared: Mutex<Shared>,
    wake: Condvar,
    cancel: Mutex<CancelHandle>,
    on_change: OnceLock<Box<dyn Fn() + Send + Sync>>,
}

impl Engine {
    pub fn new(config: Config, state: State, notice: Option<String>) -> Arc<Engine> {
        let rsync = rsync::locate(config.rsync_path.as_deref());
        Arc::new(Engine {
            shared: Mutex::new(Shared {
                config,
                state,
                queue: VecDeque::new(),
                running: None,
                rsync,
                stats_busy: HashSet::new(),
                waiting_for_destination: HashSet::new(),
                notice,
                anchors: HashMap::new(),
            }),
            wake: Condvar::new(),
            cancel: Mutex::new(CancelHandle::default()),
            on_change: OnceLock::new(),
        })
    }

    pub fn start(self: &Arc<Self>) {
        let worker = Arc::clone(self);
        std::thread::Builder::new()
            .name("macsync-worker".into())
            .spawn(move || worker.worker_loop())
            .expect("spawn worker");
        let ticker = Arc::clone(self);
        std::thread::Builder::new()
            .name("macsync-scheduler".into())
            .spawn(move || {
                loop {
                    let changed = ticker.lock().tick(Local::now());
                    if changed {
                        ticker.wake.notify_all();
                        ticker.changed();
                    }
                    std::thread::sleep(TICK);
                }
            })
            .expect("spawn scheduler");

        let stale: Vec<String> = {
            let s = self.lock();
            s.config
                .pairs
                .iter()
                .filter(|p| {
                    s.state
                        .pairs
                        .get(&p.id)
                        .and_then(|ps| ps.stats.as_ref())
                        .and_then(|st| st.measured)
                        .is_none_or(|t| (Local::now() - t).num_hours() >= STATS_MAX_AGE_HOURS)
                })
                .map(|p| p.id.clone())
                .collect()
        };
        for id in stale {
            self.refresh_stats(&id);
        }
    }

    pub fn set_on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        let _ = self.on_change.set(Box::new(f));
    }

    fn changed(&self) {
        if let Some(f) = self.on_change.get() {
            f();
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn enqueue(&self, pair_id: &str, kind: JobKind) {
        if self.lock().push(pair_id, kind) {
            self.wake.notify_all();
            self.changed();
        }
    }

    pub fn enqueue_all_backups(&self) {
        let mut any = false;
        {
            let mut s = self.lock();
            let ids: Vec<String> = s
                .config
                .pairs
                .iter()
                .filter(|p| p.enabled)
                .map(|p| p.id.clone())
                .collect();
            for id in ids {
                any |= s.push(&id, JobKind::Backup);
            }
        }
        if any {
            self.wake.notify_all();
            self.changed();
        }
    }

    /// Stops the running rsync (its incomplete snapshot is removed next time) or drops a queued job.
    pub fn cancel(&self, pair_id: &str) {
        let running = {
            let mut s = self.lock();
            s.queue.retain(|j| j.pair_id != pair_id);
            s.running.as_ref().is_some_and(|r| r.job.pair_id == pair_id)
        };
        if running {
            self.cancel.lock().unwrap().cancel();
        }
        self.changed();
    }

    pub fn save_config(&self, config: Config) -> Result<(), String> {
        config.save()?;
        {
            let mut s = self.lock();
            let ids: HashSet<&String> = config.pairs.iter().map(|p| &p.id).collect();
            s.queue.retain(|j| ids.contains(&j.pair_id));
            s.waiting_for_destination.retain(|id| ids.contains(id));
            s.rsync = rsync::locate(config.rsync_path.as_deref());
            s.config = config;
        }
        self.changed();
        Ok(())
    }

    pub fn refresh_rsync(&self) {
        {
            let mut s = self.lock();
            s.rsync = rsync::locate(s.config.rsync_path.as_deref());
        }
        self.changed();
    }

    pub fn refresh_stats(self: &Arc<Self>, pair_id: &str) {
        let pair = {
            let mut s = self.lock();
            let Some(pair) = s.config.pair(pair_id).cloned() else {
                return;
            };
            if !s.stats_busy.insert(pair_id.to_string()) {
                return;
            }
            pair
        };
        self.changed();
        let engine = Arc::clone(self);
        std::thread::spawn(move || {
            let measured = stats::measure(&pair);
            {
                let mut s = engine.lock();
                s.stats_busy.remove(&pair.id);
                s.state.pair_mut(&pair.id).stats = Some(measured);
                let _ = s.state.save();
            }
            engine.changed();
        });
    }

    /// Stops any rsync and saves state before the process exits.
    pub fn shutdown(&self) {
        self.cancel.lock().unwrap().cancel();
        let _ = self.lock().state.save();
    }

    fn worker_loop(self: Arc<Self>) {
        loop {
            let (job, pair, rsync_path) = {
                let mut s = self.lock();
                loop {
                    if let Some(job) = s.queue.pop_front() {
                        if let Some(pair) = s.config.pair(&job.pair_id).cloned() {
                            s.rsync = rsync::locate(s.config.rsync_path.as_deref());
                            s.running = Some(Running {
                                job: job.clone(),
                                pair_name: pair.display_name().to_string(),
                                started: Local::now(),
                                progress: None,
                            });
                            break (job, pair, s.rsync.clone());
                        }
                        continue;
                    }
                    s = self.wake.wait(s).unwrap_or_else(|e| e.into_inner());
                }
            };
            self.changed();
            let started = Local::now();
            let cancel = CancelHandle::default();
            *self.cancel.lock().unwrap() = cancel.clone();
            match job.kind {
                JobKind::Backup => self.run_backup_job(&pair, rsync_path, &cancel, started),
                JobKind::Integrity => self.run_integrity_job(&pair, rsync_path, &cancel, started),
            }
            self.lock().running = None;
            self.changed();
        }
    }

    fn run_backup_job(
        self: &Arc<Self>,
        pair: &Pair,
        rsync_info: Result<RsyncInfo, String>,
        cancel: &CancelHandle,
        started: DateTime<Local>,
    ) {
        let mut last_update = Instant::now() - Duration::from_secs(5);
        let result = match (rsync_info, open_log(&pair.id)) {
            (Err(e), _) | (_, Err(e)) => Err(BackupError::Failed(e)),
            (Ok(info), Ok(mut log)) => backup::run_backup(pair, &info, &mut log, cancel, |p| {
                if let Some(r) = self.lock().running.as_mut() {
                    r.progress = Some(p);
                }
                if last_update.elapsed() >= Duration::from_millis(500) {
                    last_update = Instant::now();
                    self.changed();
                }
            }),
        };
        let finished = Local::now();
        let record = match &result {
            Ok(r) => RunRecord {
                started,
                finished,
                outcome: if r.warning.is_some() {
                    Outcome::Warning
                } else {
                    Outcome::Success
                },
                message: match &r.warning {
                    Some(w) => format!("{}; {w}", r.summary),
                    None => r.summary.clone(),
                },
                snapshot: Some(r.snapshot.clone()),
                pruned: r.pruned.len(),
            },
            Err(e) => RunRecord {
                started,
                finished,
                outcome: Outcome::Failed,
                message: e.to_string(),
                snapshot: None,
                pruned: 0,
            },
        };
        let notify_failure = matches!(result, Err(BackupError::Failed(_) | BackupError::DestinationMissing(_)));
        {
            let mut s = self.lock();
            let ps = s.state.pair_mut(&pair.id);
            if result.is_ok() {
                ps.last_success = Some(finished);
            }
            ps.last_backup = Some(record.clone());
            let _ = s.state.save();
        }
        if notify_failure {
            notify(&format!("Backup failed: {}", pair.display_name()), &record.message);
        }
        if result.is_ok() {
            self.refresh_stats(&pair.id);
        }
    }

    fn run_integrity_job(
        &self,
        pair: &Pair,
        rsync_info: Result<RsyncInfo, String>,
        cancel: &CancelHandle,
        started: DateTime<Local>,
    ) {
        let result = match (rsync_info, open_log(&pair.id)) {
            (Err(e), _) | (_, Err(e)) => Err(e),
            (Ok(info), Ok(mut log)) => integrity::run_integrity(pair, &info, &mut log, cancel),
        };
        let record = result.unwrap_or_else(|e| IntegrityRecord {
            outcome: Some(Outcome::Failed),
            message: e,
            ..IntegrityRecord::default()
        });
        let record = IntegrityRecord {
            started: Some(started),
            finished: Some(Local::now()),
            ..record
        };
        if record.outcome == Some(Outcome::Failed) && !cancel.is_cancelled() {
            notify(&format!("Integrity check: {}", pair.display_name()), &record.message);
        }
        let mut s = self.lock();
        s.state.pair_mut(&pair.id).last_integrity = Some(record);
        let _ = s.state.save();
    }
}

fn open_log(pair_id: &str) -> Result<File, String> {
    let path = paths::log_file(pair_id);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("Could not open log {}: {e}", path.display()))?;
    let _ = writeln!(file, "\n=== {} ===", Local::now().format("%Y-%m-%d %H:%M:%S"));
    Ok(file)
}

fn notify(summary: &str, body: &str) {
    let (summary, body) = (summary.to_string(), body.to_string());
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new().summary(&summary).body(&body).show();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(d: u32, h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, d, h, m, 0).unwrap()
    }

    fn shared_with(pair: Pair) -> Shared {
        let mut config = Config::default();
        config.pairs.push(pair);
        Shared {
            config,
            state: State::default(),
            queue: VecDeque::new(),
            running: None,
            rsync: Err("not needed".into()),
            stats_busy: HashSet::new(),
            waiting_for_destination: HashSet::new(),
            notice: None,
            anchors: HashMap::new(),
        }
    }

    #[test]
    fn queues_once_when_due_and_waits_for_missing_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let pair = Pair {
            id: "p".into(),
            source: tmp.path().join("src"),
            destination: tmp.path().to_path_buf(),
            backup_schedule: "0 2 * * *".into(),
            integrity_schedule: "0 3 * * 0".into(),
            ..Pair::default()
        };
        let mut s = shared_with(pair);

        // First sighting sets the anchor; nothing is due yet.
        assert!(!s.tick(at(11, 8, 0)));
        assert!(s.queue.is_empty());
        assert_eq!(
            s.next_run(&s.config.pairs[0].clone(), JobKind::Backup),
            Some(at(12, 2, 0))
        );

        assert!(s.tick(at(12, 2, 0)));
        assert_eq!(s.queue.len(), 1);
        assert!(!s.tick(at(12, 2, 0)), "already queued");

        s.queue.clear();
        s.config.pairs[0].destination = tmp.path().join("unplugged");
        assert!(s.tick(at(12, 2, 1)));
        assert!(s.queue.is_empty());
        assert!(s.waiting_for_destination.contains("p"));

        s.config.pairs[0].destination = tmp.path().to_path_buf();
        assert!(s.tick(at(12, 9, 0)));
        assert_eq!(s.queue.front().unwrap().kind, JobKind::Backup);
        assert!(s.waiting_for_destination.is_empty());
    }

    #[test]
    fn last_attempt_drives_catch_up() {
        let tmp = tempfile::tempdir().unwrap();
        let pair = Pair {
            id: "p".into(),
            destination: tmp.path().to_path_buf(),
            ..Pair::default()
        };
        let mut s = shared_with(pair);
        s.state.pair_mut("p").last_backup = Some(RunRecord {
            started: at(8, 2, 0),
            finished: at(8, 2, 5),
            outcome: Outcome::Success,
            message: String::new(),
            snapshot: None,
            pruned: 0,
        });
        // Asleep for three nights: one catch-up run on wake.
        assert!(s.tick(at(11, 9, 0)));
        assert_eq!(s.queue.len(), 1);
    }

    #[test]
    fn disabled_and_manual_pairs_never_queue() {
        let tmp = tempfile::tempdir().unwrap();
        let manual = Pair {
            id: "m".into(),
            destination: tmp.path().into(),
            backup_schedule: String::new(),
            ..Pair::default()
        };
        let mut s = shared_with(manual);
        s.config.pairs.push(Pair {
            id: "d".into(),
            destination: tmp.path().into(),
            enabled: false,
            ..Pair::default()
        });
        s.tick(at(1, 0, 0));
        assert!(!s.tick(at(20, 0, 0)));
        assert!(s.queue.is_empty());
    }
}
