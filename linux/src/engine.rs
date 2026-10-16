use std::collections::HashSet;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use crate::backup::run_backup;
use crate::config::Config;
use crate::integrity::{run_integrity, IntegrityOutcome};
use crate::logutil::{ensure_data_dirs, PairLogger};
use crate::paths::{config_path, log_file, state_path};
use crate::rsync::check_rsync;
use crate::schedule::{now_local, truncate_minute, CronExpr, ScheduleTracker};
use crate::state::AppState;

static RELOAD: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);

pub fn install_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGHUP, handle_sighup as *const () as usize);
        libc::signal(libc::SIGTERM, handle_sigterm as *const () as usize);
        libc::signal(libc::SIGINT, handle_sigterm as *const () as usize);
    }
}

extern "C" fn handle_sighup(_: libc::c_int) {
    RELOAD.store(true, Ordering::SeqCst);
}

extern "C" fn handle_sigterm(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

struct PairSched {
    id: String,
    backup: Option<CronExpr>,
    integrity: Option<CronExpr>,
}

fn build_schedules(cfg: &Config) -> Result<Vec<PairSched>, String> {
    let mut out = Vec::new();
    for pair in &cfg.pair {
        if !pair.enabled {
            continue;
        }
        let backup = CronExpr::parse(&pair.backup_schedule)
            .map_err(|e| format!("pair '{}': backup_schedule: {e}", pair.id))?;
        let integrity = CronExpr::parse(&pair.integrity_schedule)
            .map_err(|e| format!("pair '{}': integrity_schedule: {e}", pair.id))?;
        out.push(PairSched {
            id: pair.id.clone(),
            backup,
            integrity,
        });
    }
    Ok(out)
}

#[derive(Clone, Copy)]
enum JobKind {
    Backup,
    Integrity,
}

fn jobs_due(schedules: &[PairSched], tracker: &mut ScheduleTracker, now: chrono::NaiveDateTime) -> Vec<(String, JobKind)> {
    let start = truncate_minute(tracker.last_tick);
    let end = truncate_minute(now);
    tracker.last_tick = now;

    let mut due = Vec::new();
    if end <= start {
        return due;
    }

    let mut backup_fired = HashSet::new();
    let mut integrity_fired = HashSet::new();
    let mut t = start + chrono::Duration::minutes(1);
    while t <= end {
        for sched in schedules {
            if let Some(ref cron) = sched.backup {
                if cron.matches(t) && backup_fired.insert(sched.id.clone()) {
                    due.push((sched.id.clone(), JobKind::Backup));
                }
            }
            if let Some(ref cron) = sched.integrity {
                if cron.matches(t) && integrity_fired.insert(sched.id.clone()) {
                    due.push((sched.id.clone(), JobKind::Integrity));
                }
            }
        }
        t += chrono::Duration::minutes(1);
    }
    due
}

fn run_job(
    cfg: &Config,
    pair_id: &str,
    kind: JobKind,
    state: &Mutex<AppState>,
    state_file: &std::path::Path,
    cancel: &Arc<AtomicBool>,
) {
    let Some(pair) = cfg.pair_by_id(pair_id).cloned() else {
        return;
    };
    let log = match PairLogger::open(&log_file(pair_id)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("macsync: cannot open log for {pair_id}: {e}");
            return;
        }
    };

    match kind {
        JobKind::Backup => match run_backup(cfg, &pair, &log, cancel) {
            Ok(_) => {
                let mut st = state.lock().unwrap();
                st.mark_backup(pair_id, true);
                let _ = st.save(state_file);
            }
            Err(e) => {
                log.error(&e.0);
                let mut st = state.lock().unwrap();
                st.mark_backup(pair_id, false);
                let _ = st.save(state_file);
            }
        },
        JobKind::Integrity => match run_integrity(cfg, &pair, &log, cancel) {
            Ok(r) => {
                let ok = r.outcome != IntegrityOutcome::Failed;
                let mut st = state.lock().unwrap();
                st.mark_integrity(pair_id, ok);
                let _ = st.save(state_file);
            }
            Err(e) => {
                log.error(&e.0);
                let mut st = state.lock().unwrap();
                st.mark_integrity(pair_id, false);
                let _ = st.save(state_file);
            }
        },
    }
}

pub fn run_daemon() -> Result<(), String> {
    ensure_data_dirs().map_err(|e| e.to_string())?;
    install_signal_handlers();

    let cfg_path = config_path();
    let cfg = Config::load(&cfg_path).map_err(|e| e.0)?;
    let rsync = check_rsync(&cfg.rsync_path).map_err(|e| e.0)?;
    eprintln!(
        "macsync: rsync {} at {}, config {}",
        rsync.version,
        rsync.path,
        cfg_path.display()
    );

    let state_file = state_path();
    let state = Arc::new(Mutex::new(AppState::load(&state_file)));
    let cfg_lock = Arc::new(RwLock::new(cfg));

    let tick_start = {
        let st = state.lock().unwrap();
        st.scheduler_tick().unwrap_or_else(now_local)
    };
    let mut tracker = ScheduleTracker::new(tick_start);
    let mut schedules = build_schedules(&cfg_lock.read().unwrap())?;
    let cancel = Arc::new(AtomicBool::new(false));

    eprintln!(
        "macsync: running ({} enabled pair(s)); SIGHUP reloads config",
        schedules.len()
    );

    while !STOP.load(Ordering::SeqCst) {
        if RELOAD.swap(false, Ordering::SeqCst) {
            match Config::load(&cfg_path) {
                Ok(new_cfg) => {
                    if let Err(e) = check_rsync(&new_cfg.rsync_path) {
                        eprintln!("macsync: reload ignored (rsync): {e}");
                    } else {
                        match build_schedules(&new_cfg) {
                            Ok(s) => {
                                schedules = s;
                                *cfg_lock.write().unwrap() = new_cfg;
                                eprintln!("macsync: config reloaded from {}", cfg_path.display());
                            }
                            Err(e) => eprintln!("macsync: reload ignored: {e}"),
                        }
                    }
                }
                Err(e) => eprintln!("macsync: reload ignored: {e}"),
            }
        }

        let now = now_local();
        let due = jobs_due(&schedules, &mut tracker, now);
        {
            let mut st = state.lock().unwrap();
            st.set_scheduler_tick(now);
            let _ = st.save(&state_file);
        }

        for (pair_id, kind) in due {
            if STOP.load(Ordering::SeqCst) {
                cancel.store(true, Ordering::SeqCst);
                break;
            }
            cancel.store(false, Ordering::SeqCst);

            // Propagate STOP into cancel while the job runs.
            let cancel_job = cancel.clone();
            let stop_watch = Arc::new(AtomicBool::new(false));
            let stop_watch_flag = stop_watch.clone();
            let watcher = thread::spawn(move || {
                while !stop_watch_flag.load(Ordering::SeqCst) {
                    if STOP.load(Ordering::SeqCst) {
                        cancel_job.store(true, Ordering::SeqCst);
                        break;
                    }
                    thread::sleep(Duration::from_millis(200));
                }
            });

            let cfg_snap = cfg_lock.read().unwrap().clone();
            run_job(
                &cfg_snap,
                &pair_id,
                kind,
                &state,
                &state_file,
                &cancel,
            );

            stop_watch.store(true, Ordering::SeqCst);
            let _ = watcher.join();
        }

        for _ in 0..15 {
            if STOP.load(Ordering::SeqCst) || RELOAD.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_secs(1));
        }
    }

    cancel.store(true, Ordering::SeqCst);
    eprintln!("macsync: shutting down");
    Ok(())
}

pub fn run_interactive() -> Result<(), String> {
    ensure_data_dirs().map_err(|e| e.to_string())?;
    let cfg_path = config_path();
    let cfg = Config::load(&cfg_path).map_err(|e| e.0)?;
    let _ = check_rsync(&cfg.rsync_path).map_err(|e| e.0)?;

    let pairs: Vec<_> = cfg.pair.iter().collect();
    if pairs.is_empty() {
        return Err("no pairs in config".into());
    }

    loop {
        println!("Select a folder pair to back up:");
        for (i, p) in pairs.iter().enumerate() {
            println!(
                "  {}. {} ({})\n     {} → {}",
                i + 1,
                p.name,
                p.id,
                p.source,
                p.destination
            );
        }
        print!("Choice [1-{}]: ", pairs.len());
        let _ = io::stdout().flush();

        let mut line = String::new();
        let n = io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("stdin closed".into());
        }
        let choice = line.trim();
        if choice.is_empty() {
            return Err("empty choice".into());
        }
        let Ok(idx) = choice.parse::<usize>() else {
            eprintln!("Invalid choice.");
            continue;
        };
        if idx == 0 || idx > pairs.len() {
            eprintln!("Invalid choice.");
            continue;
        }
        let pair = pairs[idx - 1];
        let log = PairLogger::open(&log_file(&pair.id)).map_err(|e| e.to_string())?;
        let cancel = Arc::new(AtomicBool::new(false));
        match run_backup(&cfg, pair, &log, &cancel) {
            Ok(stamp) => {
                println!("Backup finished: {stamp}");
                let mut st = AppState::load(&state_path());
                st.mark_backup(&pair.id, true);
                let _ = st.save(&state_path());
                return Ok(());
            }
            Err(e) => {
                eprintln!("Backup failed: {}", e.0);
                let mut st = AppState::load(&state_path());
                st.mark_backup(&pair.id, false);
                let _ = st.save(&state_path());
                return Err(e.0);
            }
        }
    }
}
