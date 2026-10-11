use std::cell::RefCell;
use std::sync::{Arc, Weak};

use chrono::Local;
use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::macos;
use crate::scheduler::{Engine, JobKind, Shared};
use crate::state::Outcome;
use crate::ui::relative_time;

const OPEN: &str = "open";
const BACKUP_ALL: &str = "backup-all";
const QUIT: &str = "quit";
const PAIR_PREFIX: &str = "pair:";

struct Tray {
    icon: TrayIcon,
    status: MenuItem,
    pair_items: Vec<(String, MenuItem)>,
    /// Pair ids and names the current menu was built for.
    layout: Vec<(String, String)>,
    alert: bool,
}

thread_local! {
    // TrayIcon is main-thread only on macOS.
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

/// Must run on the main thread after the event loop has started.
pub fn install(engine: &Arc<Engine>, ctx: &egui::Context) -> Result<(), String> {
    let icon = TrayIconBuilder::new()
        .with_icon_templated(icon(false))
        .with_tooltip("MacSync")
        .build()
        .map_err(|e| format!("Could not create the menu-bar icon: {e}"))?;
    TRAY.with(|t| {
        *t.borrow_mut() = Some(Tray {
            icon,
            status: MenuItem::new("", false, None),
            pair_items: Vec::new(),
            layout: Vec::new(),
            alert: false,
        })
    });

    let weak = Arc::downgrade(engine);
    let ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let Some(engine) = weak.upgrade() else { return };
        match event.id.0.as_str() {
            OPEN => {
                macos::request_show(&ctx);
                macos::activate_app();
            }
            BACKUP_ALL => engine.enqueue_all_backups(),
            QUIT => {
                engine.shutdown();
                std::process::exit(0);
            }
            id => {
                if let Some(pair_id) = id.strip_prefix(PAIR_PREFIX) {
                    engine.enqueue(pair_id, JobKind::Backup);
                }
            }
        }
    }));
    refresh(engine);
    Ok(())
}

/// Callable from any thread; the update runs on the main queue.
pub fn request_refresh(engine: Weak<Engine>) {
    macos::on_main(move || {
        if let Some(engine) = engine.upgrade() {
            refresh(&engine);
        }
    });
}

fn icon(alert: bool) -> Icon {
    Icon::from_rgba(crate::icon::rgba(alert), crate::icon::SIZE, crate::icon::SIZE).expect("valid icon")
}

fn refresh(engine: &Engine) {
    let s = engine.lock();
    TRAY.with(|t| {
        let mut t = t.borrow_mut();
        let Some(tray) = t.as_mut() else { return };

        let layout: Vec<(String, String)> = s
            .config
            .pairs
            .iter()
            .map(|p| (p.id.clone(), p.display_name().to_string()))
            .collect();
        if layout != tray.layout {
            rebuild_menu(tray, &layout);
        }

        let (status, title) = status_line(&s);
        tray.status.set_text(&status);
        tray.icon.set_title(title.as_deref());
        let _ = tray.icon.set_tooltip(Some(format!("MacSync: {status}")));
        for (id, item) in &tray.pair_items {
            if let Some(pair) = s.config.pair(id) {
                item.set_text(format!("{}  —  {}", pair.display_name(), pair_summary(&s, id)));
            }
        }

        let alert = s.config.pairs.iter().any(|p| {
            s.state.pairs.get(&p.id).is_some_and(|ps| {
                ps.last_backup.as_ref().is_some_and(|r| r.outcome == Outcome::Failed)
                    || ps
                        .last_integrity
                        .as_ref()
                        .is_some_and(|r| r.outcome == Some(Outcome::Failed))
            })
        }) || s.rsync.is_err();
        if alert != tray.alert {
            tray.alert = alert;
            let _ = tray.icon.set_icon_templated(Some(icon(alert)));
        }
    });
}

fn rebuild_menu(tray: &mut Tray, layout: &[(String, String)]) {
    let menu = Menu::new();
    let status = MenuItem::new("", false, None);
    let _ = menu.append(&status);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let mut pair_items = Vec::new();
    for (id, name) in layout {
        let item = MenuItem::with_id(format!("{PAIR_PREFIX}{id}"), name, true, None);
        let _ = menu.append(&item);
        pair_items.push((id.clone(), item));
    }
    if !layout.is_empty() {
        let _ = menu.append(&PredefinedMenuItem::separator());
    }
    let _ = menu.append(&MenuItem::with_id(
        BACKUP_ALL,
        "Back Up All Now",
        !layout.is_empty(),
        None,
    ));
    let _ = menu.append(&MenuItem::with_id(OPEN, "Open MacSync…", true, None));
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id(QUIT, "Quit MacSync", true, None));
    tray.icon.set_menu(Some(Box::new(menu)));
    tray.status = status;
    tray.pair_items = pair_items;
    tray.layout = layout.to_vec();
}

/// Menu status text, plus a short title shown next to the icon while a backup runs.
fn status_line(s: &Shared) -> (String, Option<String>) {
    if let Err(e) = &s.rsync {
        return (e.lines().next().unwrap_or("rsync missing").to_string(), None);
    }
    if let Some(r) = &s.running {
        let pct = r.progress.as_ref().map(|p| p.percent);
        let text = match (r.job.kind, pct) {
            (JobKind::Backup, Some(p)) => format!("Backing up {} ({p}%)", r.pair_name),
            (kind, _) => format!("{} {}…", kind.verb(), r.pair_name),
        };
        let title = pct.filter(|_| r.job.kind == JobKind::Backup).map(|p| format!("{p}%"));
        return (text, title);
    }
    if !s.queue.is_empty() {
        return (format!("{} job(s) queued", s.queue.len()), None);
    }
    let newest = s.state.pairs.values().filter_map(|ps| ps.last_success).max();
    let text = match newest {
        Some(t) => format!("Idle · last backup {}", relative_time(t, Local::now())),
        None if s.config.pairs.is_empty() => "No folders set up yet".to_string(),
        None => "Idle · no backups yet".to_string(),
    };
    (text, None)
}

fn pair_summary(s: &Shared, id: &str) -> String {
    if s.running.as_ref().is_some_and(|r| r.job.pair_id == id) {
        return "running".into();
    }
    if s.is_busy(id) {
        return "queued".into();
    }
    if s.waiting_for_destination.contains(id) {
        return "drive not connected".into();
    }
    let now = Local::now();
    match s.state.pairs.get(id).and_then(|ps| ps.last_backup.as_ref()) {
        Some(r) if r.outcome == Outcome::Failed => format!("failed {}", relative_time(r.finished, now)),
        Some(r) => relative_time(r.finished, now),
        None => "never backed up".into(),
    }
}
