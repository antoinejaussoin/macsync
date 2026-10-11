use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, NaiveDateTime};
use eframe::egui::{self, Color32, RichText};

use crate::backup;
use crate::config::{Config, Pair};
use crate::login;
use crate::macos;
use crate::paths;
use crate::retention::{self, KeepYearly};
use crate::rsync::RsyncInfo;
use crate::schedule::{self, Preset, WEEKDAYS};
use crate::scheduler::{Engine, Job, JobKind, Running};
use crate::state::{Outcome, PairState, State};
use crate::stats::{format_bytes, short_path};

const GREEN: Color32 = Color32::from_rgb(52, 168, 83);
const AMBER: Color32 = Color32::from_rgb(214, 140, 0);
const RED: Color32 = Color32::from_rgb(214, 54, 54);
const BLUE: Color32 = Color32::from_rgb(40, 120, 220);
const GRAY: Color32 = Color32::from_gray(140);

pub fn relative_time(t: DateTime<Local>, now: DateTime<Local>) -> String {
    let secs = (now - t).num_seconds();
    match secs {
        s if s < 0 => relative_future(t, now),
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        86_400..=172_799 => "yesterday".into(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

pub fn relative_future(t: DateTime<Local>, now: DateTime<Local>) -> String {
    let secs = (t - now).num_seconds();
    let when = if t.date_naive() == now.date_naive() {
        t.format("%H:%M").to_string()
    } else {
        t.format("%a %-d %b %H:%M").to_string()
    };
    match secs {
        s if s <= 0 => "now".into(),
        1..=3599 => format!("in {} min ({when})", (secs + 59) / 60),
        3600..=86_399 => format!("in {} h ({when})", secs / 3600),
        _ => when,
    }
}

fn friendly_stamp(name: &str) -> String {
    retention::parse_stamp(name)
        .map(|t| t.format("%a %-d %b %Y, %H:%M").to_string())
        .unwrap_or_else(|| name.to_string())
}

fn outcome_color(outcome: Outcome) -> Color32 {
    match outcome {
        Outcome::Success => GREEN,
        Outcome::Warning => AMBER,
        Outcome::Failed => RED,
    }
}

/// Copy of the engine state for one frame, so no lock is held while the UI
/// runs (folder pickers block the main thread).
struct View {
    config: Config,
    state: State,
    running: Option<Running>,
    queue: VecDeque<Job>,
    rsync: Result<RsyncInfo, String>,
    waiting: HashSet<String>,
    stats_busy: HashSet<String>,
    notice: Option<String>,
    next_runs: HashMap<(String, JobKind), Option<DateTime<Local>>>,
}

impl View {
    fn running_for(&self, pair_id: &str) -> Option<&Running> {
        self.running.as_ref().filter(|r| r.job.pair_id == pair_id)
    }

    fn queued(&self, pair_id: &str, kind: JobKind) -> bool {
        self.queue.iter().any(|j| j.pair_id == pair_id && j.kind == kind)
    }

    fn pair_state(&self, pair_id: &str) -> PairState {
        self.state.pairs.get(pair_id).cloned().unwrap_or_default()
    }
}

struct LogTail {
    pair_id: String,
    read_at: Option<Instant>,
    text: String,
}

struct SnapshotCache {
    destination: PathBuf,
    read_at: Option<Instant>,
    list: Vec<(String, NaiveDateTime)>,
    latest: Option<String>,
}

pub struct MacSyncApp {
    engine: Arc<Engine>,
    /// eframe forces the window visible after its first painted frame, whatever the
    /// viewport builder said, so a login start has to hide it again once.
    start_hidden: bool,
    selected: Option<String>,
    draft: Option<Pair>,
    draft_is_new: bool,
    confirm_delete: bool,
    flash: Option<(String, bool, Instant)>,
    custom_schedule: HashSet<(String, &'static str)>,
    rsync_override: String,
    login_enabled: bool,
    log: LogTail,
    snapshots: SnapshotCache,
}

impl MacSyncApp {
    pub fn new(engine: Arc<Engine>, start_hidden: bool) -> Self {
        let (first, rsync_override) = {
            let s = engine.lock();
            (
                s.config.pairs.first().cloned(),
                s.config
                    .rsync_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            )
        };
        MacSyncApp {
            engine,
            start_hidden,
            selected: first.as_ref().map(|p| p.id.clone()),
            draft: first,
            draft_is_new: false,
            confirm_delete: false,
            flash: None,
            custom_schedule: HashSet::new(),
            rsync_override,
            login_enabled: login::is_enabled(),
            log: LogTail {
                pair_id: String::new(),
                read_at: None,
                text: String::new(),
            },
            snapshots: SnapshotCache {
                destination: PathBuf::new(),
                read_at: None,
                list: Vec::new(),
                latest: None,
            },
        }
    }

    fn view(&self) -> View {
        let s = self.engine.lock();
        let mut next_runs = HashMap::new();
        for pair in &s.config.pairs {
            for kind in [JobKind::Backup, JobKind::Integrity] {
                next_runs.insert((pair.id.clone(), kind), s.next_run(pair, kind));
            }
        }
        View {
            config: s.config.clone(),
            state: s.state.clone(),
            running: s.running.clone(),
            queue: s.queue.clone(),
            rsync: s.rsync.clone(),
            waiting: s.waiting_for_destination.clone(),
            stats_busy: s.stats_busy.clone(),
            notice: s.notice.clone(),
            next_runs,
        }
    }

    fn flash(&mut self, msg: impl Into<String>, error: bool) {
        self.flash = Some((msg.into(), error, Instant::now()));
    }

    fn is_dirty(&self, view: &View) -> bool {
        match &self.draft {
            Some(d) => self.draft_is_new || view.config.pair(&d.id) != Some(d),
            None => false,
        }
    }

    fn select(&mut self, view: &View, id: &str) {
        if self.selected.as_deref() == Some(id) {
            return;
        }
        if self.is_dirty(view) {
            self.flash("Save or revert your changes first", true);
            return;
        }
        self.selected = Some(id.to_string());
        self.draft = view.config.pair(id).cloned();
        self.draft_is_new = false;
        self.confirm_delete = false;
    }

    fn add_pair(&mut self, view: &View) {
        if self.is_dirty(view) {
            self.flash("Save or revert your changes first", true);
            return;
        }
        let pair = Pair::default();
        self.selected = Some(pair.id.clone());
        self.draft = Some(pair);
        self.draft_is_new = true;
        self.confirm_delete = false;
    }

    fn save_draft(&mut self) {
        let Some(draft) = self.draft.clone() else { return };
        let errors = draft.validate();
        if !errors.is_empty() {
            self.flash(errors.join(". "), true);
            return;
        }
        if !draft.destination.exists()
            && draft.destination.parent().is_some_and(Path::is_dir)
            && let Err(e) = fs::create_dir(&draft.destination)
        {
            self.flash(format!("Could not create {}: {e}", draft.destination.display()), true);
            return;
        }
        let mut config = self.engine.lock().config.clone();
        match config.pairs.iter_mut().find(|p| p.id == draft.id) {
            Some(p) => *p = draft.clone(),
            None => config.pairs.push(draft.clone()),
        }
        match self.engine.save_config(config) {
            Ok(()) => {
                self.draft_is_new = false;
                self.flash(format!("Saved \"{}\"", draft.display_name()), false);
                self.engine.refresh_stats(&draft.id);
                self.snapshots.read_at = None;
            }
            Err(e) => self.flash(e, true),
        }
    }

    fn delete_selected(&mut self) {
        let Some(id) = self.selected.clone() else { return };
        self.engine.cancel(&id);
        let mut config = self.engine.lock().config.clone();
        config.pairs.retain(|p| p.id != id);
        match self.engine.save_config(config) {
            Ok(()) => {
                self.engine.lock().state.pairs.remove(&id);
                let next = self.engine.lock().config.pairs.first().cloned();
                self.selected = next.as_ref().map(|p| p.id.clone());
                self.draft = next;
                self.draft_is_new = false;
                self.confirm_delete = false;
                self.flash("Folder pair removed. Its snapshots were left on disk.", false);
            }
            Err(e) => self.flash(e, true),
        }
    }

    fn refresh_snapshot_cache(&mut self, destination: &Path, force: bool) {
        let stale = self
            .snapshots
            .read_at
            .is_none_or(|t| t.elapsed() > Duration::from_secs(5));
        if !force && !stale && self.snapshots.destination == destination {
            return;
        }
        self.snapshots.destination = destination.to_path_buf();
        self.snapshots.read_at = Some(Instant::now());
        self.snapshots.list = backup::list_snapshots(destination).unwrap_or_default();
        self.snapshots.latest = backup::latest_name(destination);
    }

    fn refresh_log(&mut self, pair_id: &str) {
        let fresh =
            self.log.pair_id == pair_id && self.log.read_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2));
        if fresh {
            return;
        }
        self.log.pair_id = pair_id.to_string();
        self.log.read_at = Some(Instant::now());
        self.log.text = read_tail(&paths::log_file(pair_id), 48 * 1024, 300);
    }
}

fn read_tail(path: &Path, max_bytes: u64, max_lines: usize) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().skip(usize::from(start > 0)).collect();
    lines[lines.len().saturating_sub(max_lines)..].join("\n")
}

impl eframe::App for MacSyncApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if std::mem::take(&mut self.start_hidden) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if macos::take_show_request() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            macos::activate_app();
        }
        let hide = ctx.input(|i| i.viewport().close_requested())
            || ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::W));
        if hide {
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let view = self.view();
        if let Some(id) = &self.selected
            && !self.draft_is_new
            && view.config.pair(id).is_none()
        {
            self.selected = None;
            self.draft = None;
        }

        self.banners(ui, &view);

        egui::Panel::left("pairs")
            .resizable(false)
            .default_size(250.0)
            .show(ui, |ui| self.sidebar(ui, &view));

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(mut draft) = self.draft.take() {
                self.detail(ui, &view, &mut draft);
                if self.draft.is_none() && self.selected.as_deref() == Some(draft.id.as_str()) {
                    self.draft = Some(draft);
                }
            } else {
                self.welcome(ui, &view);
            }
        });

        let busy = view.running.is_some() || !view.stats_busy.is_empty();
        ui.ctx()
            .request_repaint_after(Duration::from_millis(if busy { 300 } else { 5000 }));
    }
}

impl MacSyncApp {
    fn banners(&mut self, ui: &mut egui::Ui, view: &View) {
        let flash = self
            .flash
            .clone()
            .filter(|(_, _, t)| t.elapsed() < Duration::from_secs(8));
        if view.rsync.is_err() || view.notice.is_some() || flash.is_some() {
            egui::Panel::top("banners").show(ui, |ui| {
                ui.add_space(4.0);
                if let Err(e) = &view.rsync {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(RED, RichText::new("rsync unavailable").strong());
                        ui.label(e);
                        if ui.button("Check again").clicked() {
                            self.engine.refresh_rsync();
                        }
                    });
                }
                if let Some(notice) = &view.notice {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(AMBER, notice);
                        if ui.small_button("Dismiss").clicked() {
                            self.engine.lock().notice = None;
                        }
                    });
                }
                if let Some((msg, error, _)) = flash {
                    ui.colored_label(if error { RED } else { GREEN }, msg);
                }
                ui.add_space(4.0);
            });
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, view: &View) {
        ui.add_space(8.0);
        ui.heading("MacSync");
        ui.add_space(6.0);

        egui::Panel::bottom("settings").show(ui, |ui| self.settings(ui, view));

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut clicked = None;
            for pair in &view.config.pairs {
                if self.pair_row(ui, view, pair).clicked() {
                    clicked = Some(pair.id.clone());
                }
            }
            if self.draft_is_new
                && let Some(d) = &self.draft
            {
                self.row_frame(ui, true, GRAY, d.display_name(), "Not saved yet");
            }
            if let Some(id) = clicked {
                self.select(view, &id);
            }
            ui.add_space(8.0);
            if ui.button("+ Add folder pair").clicked() {
                self.add_pair(view);
            }
        });
    }

    fn pair_row(&self, ui: &mut egui::Ui, view: &View, pair: &Pair) -> egui::Response {
        let ps = view.pair_state(&pair.id);
        let now = Local::now();
        let (color, subtitle) = if let Some(r) = view.running_for(&pair.id) {
            let text = match (r.job.kind, &r.progress) {
                (JobKind::Backup, Some(p)) => format!("Backing up · {}%", p.percent),
                (kind, _) => format!("{}…", kind.verb()),
            };
            (BLUE, text)
        } else if view.queued(&pair.id, JobKind::Backup) || view.queued(&pair.id, JobKind::Integrity) {
            (BLUE, "Queued".into())
        } else if view.waiting.contains(&pair.id) {
            (AMBER, "Waiting for destination drive".into())
        } else if !pair.enabled {
            (GRAY, "Paused".into())
        } else {
            match &ps.last_backup {
                Some(r) if r.outcome == Outcome::Failed => (RED, format!("Failed {}", relative_time(r.finished, now))),
                Some(r) => (
                    outcome_color(r.outcome),
                    format!("Backed up {}", relative_time(r.finished, now)),
                ),
                None => (GRAY, "Never backed up".into()),
            }
        };
        let selected = self.selected.as_deref() == Some(pair.id.as_str());
        self.row_frame(ui, selected, color, pair.display_name(), &subtitle)
    }

    fn row_frame(
        &self,
        ui: &mut egui::Ui,
        selected: bool,
        color: Color32,
        title: &str,
        subtitle: &str,
    ) -> egui::Response {
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 44.0), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                true,
                selected,
                format!("{title}, {subtitle}"),
            )
        });
        let visuals = ui.visuals();
        if selected {
            ui.painter()
                .rect_filled(rect, 6.0, visuals.selection.bg_fill.gamma_multiply(0.6));
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect, 6.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let text_color = visuals.strong_text_color();
        ui.painter()
            .circle_filled(rect.left_center() + egui::vec2(12.0, 0.0), 5.0, color);
        ui.painter().text(
            rect.left_top() + egui::vec2(26.0, 7.0),
            egui::Align2::LEFT_TOP,
            title,
            egui::FontId::proportional(14.0),
            text_color,
        );
        ui.painter().text(
            rect.left_top() + egui::vec2(26.0, 25.0),
            egui::Align2::LEFT_TOP,
            subtitle,
            egui::FontId::proportional(11.5),
            visuals.weak_text_color(),
        );
        response
    }

    fn settings(&mut self, ui: &mut egui::Ui, view: &View) {
        ui.add_space(6.0);
        let mut login_enabled = self.login_enabled;
        if ui.checkbox(&mut login_enabled, "Launch at login").changed() {
            let result = if login_enabled {
                login::enable()
            } else {
                login::disable()
            };
            match result {
                Ok(()) => self.login_enabled = login_enabled,
                Err(e) => self.flash(e, true),
            }
        }
        if self.login_enabled && !login::running_from_bundle() {
            ui.label(
                RichText::new("Running outside MacSync.app: the login item points at this build.")
                    .small()
                    .color(AMBER),
            );
        }
        ui.add_space(4.0);
        match &view.rsync {
            Ok(info) => ui.label(
                RichText::new(format!("rsync {} · {}", info.version, info.path.display()))
                    .small()
                    .weak(),
            ),
            Err(_) => ui.label(RichText::new("rsync not found: brew install rsync").small().color(RED)),
        };
        egui::CollapsingHeader::new(RichText::new("rsync location").small())
            .id_salt("rsync-override")
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.rsync_override)
                        .hint_text("/opt/homebrew/bin/rsync")
                        .desired_width(f32::INFINITY),
                );
                if ui.small_button("Apply").clicked() {
                    let mut config = self.engine.lock().config.clone();
                    let value = self.rsync_override.trim();
                    config.rsync_path = (!value.is_empty()).then(|| PathBuf::from(value));
                    if let Err(e) = self.engine.save_config(config) {
                        self.flash(e, true);
                    }
                }
            });
        ui.add_space(6.0);
    }

    fn welcome(&mut self, ui: &mut egui::Ui, view: &View) {
        ui.add_space(24.0);
        ui.heading("Dated snapshots of your folders");
        ui.add_space(8.0);
        ui.label(
            "MacSync copies each source folder into a new dated snapshot on a schedule. \
             Unchanged files are hard-linked to the previous snapshot, so every snapshot is a \
             complete, browsable copy that only costs the space of what changed.",
        );
        ui.add_space(16.0);
        ui.label(RichText::new("1. Homebrew rsync").strong());
        match &view.rsync {
            Ok(info) => ui.colored_label(
                GREEN,
                format!("Found rsync {} at {}", info.version, info.path.display()),
            ),
            Err(_) => ui.colored_label(RED, "Not found. In Terminal run:  brew install rsync"),
        };
        ui.add_space(10.0);
        ui.label(RichText::new("2. Full Disk Access").strong());
        ui.label("Desktop, Documents and other protected folders can only be read once MacSync is allowed in System Settings > Privacy & Security > Full Disk Access.");
        if ui.button("Open Full Disk Access settings").clicked() {
            macos::open(macos::FULL_DISK_ACCESS_URL);
        }
        ui.add_space(10.0);
        ui.label(RichText::new("3. Add a folder pair").strong());
        ui.label("Pick a folder to protect and a folder on another drive to hold its snapshots. APFS or Mac OS Extended drives work; exFAT has no hard links.");
        if ui.button("+ Add folder pair").clicked() {
            self.add_pair(view);
        }
    }

    fn detail(&mut self, ui: &mut egui::Ui, view: &View, draft: &mut Pair) {
        let dirty = self.draft_is_new || view.config.pair(&draft.id) != Some(&*draft);
        let ps = view.pair_state(&draft.id);
        let errors = draft.validate();

        egui::Panel::bottom("save-bar").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let save = ui.add_enabled(dirty && errors.is_empty(), egui::Button::new("Save changes"));
                if save.clicked() {
                    self.draft = Some(draft.clone());
                    self.save_draft();
                    if let Some(saved) = self.draft.take() {
                        *draft = saved;
                    }
                }
                if self.draft_is_new {
                    if ui.button("Discard").clicked() {
                        let first = view.config.pairs.first().cloned();
                        self.selected = first.as_ref().map(|p| p.id.clone());
                        self.draft_is_new = false;
                        self.draft = first;
                        return;
                    }
                } else if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked()
                    && let Some(saved) = view.config.pair(&draft.id)
                {
                    *draft = saved.clone();
                }
                if dirty {
                    ui.label(RichText::new("Unsaved changes").color(AMBER));
                }
            });
            for e in &errors {
                ui.colored_label(RED, e);
            }
            ui.add_space(6.0);
        });
        if self.draft.is_some() {
            return;
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut draft.name)
                        .font(egui::TextStyle::Heading)
                        .desired_width(300.0),
                );
                ui.checkbox(&mut draft.enabled, "Run on schedule");
            });
            ui.add_space(8.0);

            self.actions(ui, view, draft, dirty, &ps);
            ui.add_space(6.0);
            self.status(ui, view, draft, &ps);

            section(ui, "Folders");
            self.folders(ui, draft);

            section(ui, "Sizes");
            self.stats_grid(ui, view, draft, &ps);

            section(ui, "Schedule");
            egui::Grid::new("schedules").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Back up");
                self.schedule_editor(ui, &draft.id.clone(), "backup", &mut draft.backup_schedule);
                ui.end_row();
                ui.label("Check integrity");
                self.schedule_editor(ui, &draft.id.clone(), "integrity", &mut draft.integrity_schedule);
                ui.end_row();
            });
            ui.label(
                RichText::new("If the Mac is asleep or the drive is unplugged at the scheduled time, the job runs once when it can.")
                    .small()
                    .weak(),
            );

            section(ui, "Retention");
            self.retention(ui, draft);

            section(ui, "Options");
            self.options(ui, draft);

            if let Some(record) = &ps.last_integrity {
                section(ui, "Last integrity check");
                integrity_details(ui, record);
            }

            if !self.draft_is_new {
                section(ui, "Log");
                self.log_view(ui, &draft.id);
            }

            section(ui, "What this protects against");
            ui.label(
                "Each snapshot is a separate dated folder, so files that ransomware encrypts or deletes in the source \
                 can be restored from an earlier snapshot. Hard links mean snapshots share the same disk blocks for \
                 unchanged files: this is not a second copy of the data. Read-only snapshots stop casual rewrites, \
                 but anything running as you can undo that. Keep the destination on a separate drive and disconnect \
                 it (or use a second, offline drive) for real isolation.",
            );

            if !self.draft_is_new {
                ui.add_space(16.0);
                if self.confirm_delete {
                    ui.horizontal(|ui| {
                        ui.label("Remove this folder pair? Snapshots already on disk are kept.");
                        if ui.button(RichText::new("Remove").color(RED)).clicked() {
                            self.delete_selected();
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_delete = false;
                        }
                    });
                } else if ui.button(RichText::new("Remove folder pair…").color(RED)).clicked() {
                    self.confirm_delete = true;
                }
            }
            ui.add_space(16.0);
        });
    }

    fn actions(&mut self, ui: &mut egui::Ui, view: &View, draft: &Pair, dirty: bool, ps: &PairState) {
        if self.draft_is_new {
            ui.label(RichText::new("Choose the folders below, then save to start backing up.").weak());
            return;
        }
        if let Some(r) = view.running_for(&draft.id) {
            ui.horizontal(|ui| {
                match (&r.job.kind, &r.progress) {
                    (JobKind::Backup, Some(p)) => {
                        ui.add(
                            egui::ProgressBar::new(p.percent as f32 / 100.0)
                                .desired_width(320.0)
                                .text(format!(
                                    "{}%  ·  {} copied  ·  {}  ·  {} left",
                                    p.percent,
                                    format_bytes(p.bytes),
                                    p.rate,
                                    p.eta
                                )),
                        );
                    }
                    (kind, _) => {
                        ui.spinner();
                        ui.label(format!("{} since {}", kind.verb(), r.started.format("%H:%M:%S")));
                    }
                }
                if ui.button("Stop").clicked() {
                    self.engine.cancel(&draft.id);
                }
            });
            return;
        }
        let rsync_ok = view.rsync.is_ok();
        let has_snapshot = ps.last_success.is_some() || backup::latest_name(&draft.destination).is_some();
        ui.horizontal(|ui| {
            let queued = view.queued(&draft.id, JobKind::Backup);
            let label = if queued { "Queued…" } else { "Back up now" };
            if ui
                .add_enabled(rsync_ok && !dirty && !queued, egui::Button::new(label))
                .clicked()
            {
                self.engine.enqueue(&draft.id, JobKind::Backup);
            }
            let queued = view.queued(&draft.id, JobKind::Integrity);
            let label = if queued { "Queued…" } else { "Check integrity" };
            if ui
                .add_enabled(rsync_ok && !dirty && !queued && has_snapshot, egui::Button::new(label))
                .on_hover_text(
                    "Compares every file in the source with the latest snapshot by checksum. Nothing is written.",
                )
                .clicked()
            {
                self.engine.enqueue(&draft.id, JobKind::Integrity);
            }
            if ui
                .add_enabled(draft.destination.is_dir(), egui::Button::new("Show snapshots"))
                .clicked()
            {
                let latest = draft.destination.join(backup::LATEST);
                if latest.exists() {
                    macos::reveal_in_finder(&latest);
                } else {
                    macos::open(&draft.destination);
                }
            }
            if dirty {
                ui.label(RichText::new("Save changes to run").small().weak());
            }
        });
    }

    fn status(&self, ui: &mut egui::Ui, view: &View, draft: &Pair, ps: &PairState) {
        let now = Local::now();
        egui::Grid::new("status")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label("Last backup");
                match &ps.last_backup {
                    Some(r) => {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(
                                outcome_color(r.outcome),
                                format!(
                                    "{} ({})",
                                    relative_time(r.finished, now),
                                    r.finished.format("%a %-d %b %H:%M")
                                ),
                            );
                            ui.label(&r.message);
                        });
                    }
                    None => {
                        ui.label(RichText::new("Never").weak());
                    }
                }
                ui.end_row();
                if let (Some(success), Some(last)) = (ps.last_success, &ps.last_backup)
                    && last.outcome == Outcome::Failed
                {
                    ui.label("Last success");
                    ui.label(relative_time(success, now));
                    ui.end_row();
                }
                for (kind, label) in [(JobKind::Backup, "Next backup"), (JobKind::Integrity, "Next check")] {
                    if self.draft_is_new {
                        break;
                    }
                    ui.label(label);
                    let expr = kind.schedule(draft);
                    let text = if expr.trim().is_empty() {
                        "Manual only".to_string()
                    } else if !draft.enabled {
                        "Paused".to_string()
                    } else if view.waiting.contains(&draft.id) {
                        "Waiting for the destination drive".to_string()
                    } else {
                        match view.next_runs.get(&(draft.id.clone(), kind)).copied().flatten() {
                            Some(t) if t <= now => "Due now".to_string(),
                            Some(t) => relative_future(t, now),
                            None => "—".to_string(),
                        }
                    };
                    ui.label(text);
                    ui.end_row();
                }
            });
    }

    fn folders(&mut self, ui: &mut egui::Ui, draft: &mut Pair) {
        egui::Grid::new("folders")
            .num_columns(3)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Source");
                path_label(ui, &draft.source, "Folder to protect");
                if ui.button("Choose…").clicked()
                    && let Some(dir) = rfd::FileDialog::new().set_title("Folder to back up").pick_folder()
                {
                    if draft.name == Pair::default().name || draft.name.trim().is_empty() {
                        draft.name = dir
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                    }
                    draft.source = dir;
                }
                ui.end_row();

                ui.label("Destination");
                path_label(ui, &draft.destination, "Where snapshots are stored");
                if ui.button("Choose…").clicked()
                    && let Some(dir) = rfd::FileDialog::new()
                        .set_title("Where to store snapshots")
                        .pick_folder()
                {
                    draft.destination = suggested_destination(&dir, &draft.source);
                }
                ui.end_row();
            });
        if !draft.destination.as_os_str().is_empty() {
            match paths::check_pair_paths(&draft.source, &draft.destination) {
                Err(paths::PathError::DestinationMissing(_))
                    if draft.destination.parent().is_some_and(Path::is_dir) =>
                {
                    ui.label(
                        RichText::new("This folder will be created when you save.")
                            .small()
                            .weak(),
                    );
                }
                Err(e @ (paths::PathError::DestinationMissing(_) | paths::PathError::SourceMissing(_))) => {
                    ui.colored_label(AMBER, e.to_string());
                }
                _ => {}
            }
            if let Some(info) = backup::fs_info(&draft.destination)
                && !backup::filesystem_supports_hard_links(&info.fs_type)
            {
                ui.colored_label(
                    RED,
                    format!(
                        "This drive is formatted as {}, which has no hard links. Use APFS or Mac OS Extended.",
                        info.fs_type
                    ),
                );
            }
        }
    }

    fn stats_grid(&mut self, ui: &mut egui::Ui, view: &View, draft: &Pair, ps: &PairState) {
        let measuring = view.stats_busy.contains(&draft.id);
        let st = ps.stats.clone().unwrap_or_default();
        let bytes = |b: Option<u64>| b.map(format_bytes).unwrap_or_else(|| "—".into());
        egui::Grid::new("stats")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label("Source");
                ui.label(bytes(st.source_bytes));
                ui.end_row();
                ui.label("All snapshots on disk");
                ui.label(format!(
                    "{}  (unchanged files stored once)",
                    bytes(st.destination_bytes)
                ));
                ui.end_row();
                ui.label("Snapshots");
                match (&st.oldest, &st.newest) {
                    (Some(oldest), Some(newest)) => ui.label(format!(
                        "{} · {} to {}",
                        st.snapshot_count,
                        friendly_stamp(oldest),
                        friendly_stamp(newest)
                    )),
                    _ => ui.label(st.snapshot_count.to_string()),
                };
                ui.end_row();
                ui.label("Free on destination");
                ui.label(match (&st.free_bytes, &st.filesystem) {
                    (Some(free), Some(fs)) => format!("{} ({fs})", format_bytes(*free)),
                    (free, _) => bytes(*free),
                });
                ui.end_row();
            });
        ui.horizontal(|ui| {
            if measuring {
                ui.spinner();
                ui.label(RichText::new("Measuring…").small().weak());
            } else {
                if let Some(t) = st.measured {
                    ui.label(
                        RichText::new(format!("Measured {}", relative_time(t, Local::now())))
                            .small()
                            .weak(),
                    );
                }
                if !self.draft_is_new && ui.small_button("Refresh").clicked() {
                    self.engine.refresh_stats(&draft.id);
                }
            }
        });
        if let Some(e) = &st.error {
            ui.label(RichText::new(e).small().color(AMBER));
        }
    }

    fn schedule_editor(&mut self, ui: &mut egui::Ui, pair_id: &str, which: &'static str, expr: &mut String) {
        let key = (pair_id.to_string(), which);
        let before = if self.custom_schedule.contains(&key) {
            Preset::Custom
        } else {
            Preset::from_cron(expr)
        };
        let mut preset = before;
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("preset", which))
                    .selected_text(preset.label())
                    .width(130.0)
                    .show_ui(ui, |ui| {
                        for option in [
                            Preset::Manual,
                            Preset::Hourly { minute: 0 },
                            Preset::Daily { hour: 2, minute: 0 },
                            Preset::Weekly { weekday: 0, hour: 3, minute: 0 },
                            Preset::Custom,
                        ] {
                            let same = std::mem::discriminant(&preset) == std::mem::discriminant(&option);
                            if ui.selectable_label(same, option.label()).clicked() && !same {
                                preset = option;
                            }
                        }
                    });
                match &mut preset {
                    Preset::Manual => {}
                    Preset::Hourly { minute } => {
                        ui.label("at minute");
                        ui.add(egui::DragValue::new(minute).range(0..=59));
                    }
                    Preset::Daily { hour, minute } => {
                        ui.label("at");
                        time_edit(ui, hour, minute);
                    }
                    Preset::Weekly { weekday, hour, minute } => {
                        ui.label("on");
                        egui::ComboBox::from_id_salt(("weekday", which))
                            .selected_text(WEEKDAYS[*weekday as usize % 7])
                            .width(100.0)
                            .show_ui(ui, |ui| {
                                for (i, name) in WEEKDAYS.iter().enumerate() {
                                    ui.selectable_value(weekday, i as u32, *name);
                                }
                            });
                        ui.label("at");
                        time_edit(ui, hour, minute);
                    }
                    Preset::Custom => {
                        ui.add(
                            egui::TextEdit::singleline(expr)
                                .hint_text("min hour day month weekday")
                                .desired_width(160.0)
                                .font(egui::TextStyle::Monospace),
                        )
                        .on_hover_text("Standard cron: minute hour day-of-month month day-of-week (0 = Sunday). Example: 30 */4 * * 1-5");
                    }
                }
            });
            if preset == Preset::Custom {
                if before != Preset::Custom && expr.trim().is_empty() {
                    *expr = "0 2 * * *".into();
                }
                self.custom_schedule.insert(key);
                if let Err(e) = schedule::validate(expr) {
                    ui.colored_label(RED, e);
                } else if let Some(next) = schedule::next_after(expr, Local::now()) {
                    ui.label(RichText::new(format!("Next match: {}", next.format("%a %-d %b %H:%M"))).small().weak());
                }
            } else {
                self.custom_schedule.remove(&key);
                if let Some(cron) = preset.to_cron() {
                    *expr = cron;
                }
            }
        });
    }

    fn retention(&mut self, ui: &mut egui::Ui, draft: &mut Pair) {
        let r = &mut draft.retention;
        egui::Grid::new("retention")
            .num_columns(3)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Keep the last");
                ui.add(egui::DragValue::new(&mut r.keep_last).range(0..=10_000));
                ui.label("snapshots");
                ui.end_row();
                ui.label("One per day for");
                ui.add(egui::DragValue::new(&mut r.keep_daily).range(0..=3650));
                ui.label("days");
                ui.end_row();
                ui.label("One per week for");
                ui.add(egui::DragValue::new(&mut r.keep_weekly).range(0..=520));
                ui.label("weeks");
                ui.end_row();
                ui.label("One per month for");
                ui.add(egui::DragValue::new(&mut r.keep_monthly).range(0..=1200));
                ui.label("months");
                ui.end_row();
                ui.label("One per year");
                let mut forever = r.keep_yearly == KeepYearly::Forever;
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut forever, "forever").changed() {
                        r.keep_yearly = if forever {
                            KeepYearly::Forever
                        } else {
                            KeepYearly::Count(5)
                        };
                    }
                    if let KeepYearly::Count(n) = &mut r.keep_yearly {
                        ui.add(egui::DragValue::new(n).range(0..=100));
                    }
                });
                ui.label(if forever { "" } else { "years" });
                ui.end_row();
            });
        ui.label(
            RichText::new("0 turns a tier off. Each tier keeps the newest snapshot of each day, week, month or year; a period already covered by another tier is not stored twice. The snapshot \"latest\" points at is never removed.")
                .small()
                .weak(),
        );

        if draft.destination.as_os_str().is_empty() {
            return;
        }
        self.refresh_snapshot_cache(&draft.destination.clone(), false);
        let snapshots = &self.snapshots.list;
        if snapshots.is_empty() {
            ui.label(RichText::new("No snapshots yet.").weak());
            return;
        }
        let decisions = retention::plan(snapshots, &draft.retention, self.snapshots.latest.as_deref());
        let kept = decisions.iter().filter(|d| d.keep()).count();
        let removed = decisions.len() - kept;
        let color = if removed > 0 { AMBER } else { GREEN };
        ui.colored_label(
            color,
            format!(
                "{} snapshots on disk: this policy keeps {kept} and removes {removed} after the next backup.",
                decisions.len()
            ),
        );
        egui::CollapsingHeader::new("Preview")
            .id_salt("retention-preview")
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .id_salt("preview-scroll")
                    .show(ui, |ui| {
                        egui::Grid::new("preview").num_columns(2).striped(true).show(ui, |ui| {
                            for d in &decisions {
                                ui.label(RichText::new(friendly_stamp(&d.name)).monospace());
                                if d.keep() {
                                    let tiers: Vec<&str> = d.kept_by.iter().map(|t| t.label()).collect();
                                    ui.colored_label(GREEN, format!("keep · {}", tiers.join(", ")));
                                } else {
                                    ui.colored_label(RED, "remove");
                                }
                                ui.end_row();
                            }
                        });
                    });
            });
    }

    fn options(&mut self, ui: &mut egui::Ui, draft: &mut Pair) {
        ui.checkbox(&mut draft.read_only, "Make snapshots read-only")
            .on_hover_text("Files are copied without write permission and snapshot folders are locked after each backup. Restored files come back read-only. Changing this setting makes the next backup copy every file once.");
        ui.add_space(6.0);
        ui.label("Exclude (one rsync pattern per line)");
        let mut text = draft.excludes.join("\n");
        if ui
            .add(
                egui::TextEdit::multiline(&mut text)
                    .desired_rows(3)
                    .desired_width(360.0)
                    .font(egui::TextStyle::Monospace)
                    .hint_text(".DS_Store\nnode_modules/\n*.tmp"),
            )
            .changed()
        {
            draft.excludes = text.lines().map(str::to_owned).collect();
        }
    }

    fn log_view(&mut self, ui: &mut egui::Ui, pair_id: &str) {
        self.refresh_log(pair_id);
        ui.horizontal(|ui| {
            if ui.small_button("Open log file").clicked() {
                macos::open(paths::log_file(pair_id));
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("log")
            .max_height(220.0)
            .stick_to_bottom(true)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let text = if self.log.text.is_empty() {
                    "No activity yet."
                } else {
                    self.log.text.as_str()
                };
                ui.add(egui::Label::new(RichText::new(text).monospace().small()).wrap());
            });
    }
}

fn integrity_details(ui: &mut egui::Ui, record: &crate::state::IntegrityRecord) {
    let color = record.outcome.map(outcome_color).unwrap_or(GRAY);
    ui.horizontal_wrapped(|ui| {
        if let Some(t) = record.finished {
            ui.label(format!("{}:", relative_time(t, Local::now())));
        }
        ui.colored_label(color, &record.message);
    });
    if !record.snapshot.is_empty() {
        ui.label(
            RichText::new(format!("Compared with snapshot {}", friendly_stamp(&record.snapshot)))
                .small()
                .weak(),
        );
    }
    for (title, list, count, color) in [
        (
            "Same size and date, different content",
            &record.silent,
            record.silent_count,
            RED,
        ),
        (
            "Edited since the snapshot",
            &record.changed,
            record.changed_count,
            AMBER,
        ),
        ("New in the source", &record.missing, record.missing_count, GRAY),
        ("Deleted from the source", &record.extra, record.extra_count, GRAY),
    ] {
        if count == 0 {
            continue;
        }
        egui::CollapsingHeader::new(RichText::new(format!("{title} ({count})")).color(color))
            .id_salt(title)
            .show(ui, |ui| {
                for name in list {
                    ui.label(RichText::new(name).monospace().small());
                }
                if count > list.len() {
                    ui.label(
                        RichText::new(format!("…and {} more (see log)", count - list.len()))
                            .small()
                            .weak(),
                    );
                }
            });
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title).heading().size(16.0));
    ui.separator();
}

fn path_label(ui: &mut egui::Ui, path: &Path, placeholder: &str) {
    if path.as_os_str().is_empty() {
        ui.label(RichText::new(placeholder).weak().italics());
    } else {
        ui.allocate_ui_with_layout(
            egui::vec2(480.0, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.add(egui::Label::new(RichText::new(short_path(path)).monospace()).truncate())
                    .on_hover_text(path.display().to_string());
            },
        );
    }
}

fn time_edit(ui: &mut egui::Ui, hour: &mut u32, minute: &mut u32) {
    ui.add(
        egui::DragValue::new(hour)
            .range(0..=23)
            .custom_formatter(|v, _| format!("{v:02}")),
    );
    ui.label(":");
    ui.add(
        egui::DragValue::new(minute)
            .range(0..=59)
            .custom_formatter(|v, _| format!("{v:02}")),
    );
}

/// A folder that already holds snapshots, or is empty, is used as is. Otherwise
/// snapshots go in a subfolder named after the source so they do not mix with other files.
fn suggested_destination(picked: &Path, source: &Path) -> PathBuf {
    let has_snapshots =
        picked.join(backup::LATEST).exists() || backup::list_snapshots(picked).is_ok_and(|s| !s.is_empty());
    let is_empty = fs::read_dir(picked).is_ok_and(|mut d| d.next().is_none());
    if has_snapshots || is_empty {
        return picked.to_path_buf();
    }
    let name = source
        .file_name()
        .map(|n| format!("{} snapshots", n.to_string_lossy()))
        .unwrap_or_else(|| "MacSync snapshots".into());
    picked.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn relative_times() {
        let now = Local.with_ymd_and_hms(2026, 10, 11, 8, 0, 0).unwrap();
        assert_eq!(relative_time(now - chrono::Duration::seconds(30), now), "just now");
        assert_eq!(relative_time(now - chrono::Duration::minutes(5), now), "5 min ago");
        assert_eq!(relative_time(now - chrono::Duration::hours(3), now), "3 h ago");
        assert_eq!(relative_time(now - chrono::Duration::days(3), now), "3 days ago");
        assert_eq!(
            relative_future(now + chrono::Duration::minutes(30), now),
            "in 30 min (08:30)"
        );
    }

    use crate::retention::Retention;
    use crate::rsync::Progress;
    use crate::state::{IntegrityRecord, RunRecord, Stats};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    fn harness(engine: &Arc<Engine>) -> Harness<'static, MacSyncApp> {
        let engine = Arc::clone(engine);
        Harness::builder()
            .with_size(egui::vec2(980.0, 900.0))
            .build_eframe(move |_cc| MacSyncApp::new(engine, false))
    }

    /// Set `MACSYNC_UI_SHOTS=/some/dir` to write PNG renders of each screen.
    fn shot(h: &mut Harness<'_, MacSyncApp>, name: &str) {
        if let Some(dir) = std::env::var_os("MACSYNC_UI_SHOTS") {
            let image = h.render().expect("render");
            image
                .save(Path::new(&dir).join(format!("{name}.png")))
                .expect("save png");
        }
    }

    fn stamp_dirs(dest: &Path, names: &[&str]) {
        for name in names {
            fs::create_dir_all(dest.join(name)).unwrap();
        }
        backup::set_latest(dest, names[0]).unwrap();
    }

    /// One test, because `MACSYNC_HOME` is process-wide.
    #[test]
    fn ui_flows() {
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: no other test reads or writes MACSYNC_HOME.
        unsafe { std::env::set_var("MACSYNC_HOME", tmp.path().join("home")) };

        // Welcome screen, then add a pair.
        let engine = Engine::new(Config::default(), State::default(), None);
        let mut h = harness(&engine);
        h.run_steps(6);
        h.get_by_label("Dated snapshots of your folders");
        shot(&mut h, "1-welcome");
        h.get_all_by_label("+ Add folder pair").next().unwrap().click();
        h.run_steps(6);
        assert!(h.state().draft_is_new);
        h.get_by_label("Source folder is not set");
        assert!(h.get_by_label("Save changes").accesskit_node().is_disabled());

        let src = tmp.path().join("Documents");
        let drive = tmp.path().join("Drive");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&drive).unwrap();
        {
            let draft = h.state_mut().draft.as_mut().unwrap();
            draft.name = "Documents".into();
            draft.source = src.clone();
            draft.destination = drive.join("Documents snapshots");
        }
        h.run_steps(6);
        h.get_by_label("This folder will be created when you save.");

        // Pick "Every week" for the backup schedule.
        h.get_by_value("Every day").click();
        h.run_steps(6);
        h.get_by_label("Every week").click();
        h.run_steps(6);
        assert_eq!(h.state().draft.as_ref().unwrap().backup_schedule, "0 3 * * 0");
        shot(&mut h, "2-new-pair");

        h.get_by_label("Save changes").click();
        h.run_steps(6);
        assert_eq!(engine.lock().config.pairs.len(), 1);
        assert!(drive.join("Documents snapshots").is_dir());
        assert!(tmp.path().join("home/config.toml").is_file());
        h.get_by_label("Saved \"Documents\"");
        h.get_by_label_contains("Documents, Never backed up");
        assert!(!h.state().draft_is_new);

        // A pair with history: snapshots on disk, a failed run and a failed integrity check.
        let dest = tmp.path().join("Backups");
        stamp_dirs(
            &dest,
            &[
                "2026-10-11T020000",
                "2026-10-10T020000",
                "2026-10-09T020000",
                "2026-09-30T020000",
                "2026-09-15T020000",
                "2026-08-31T020000",
                "2025-06-01T020000",
                "2025-05-01T020000",
            ],
        );
        let pair = Pair {
            id: "hist".into(),
            name: "Photos".into(),
            source: src.clone(),
            destination: dest.clone(),
            retention: Retention {
                keep_last: 3,
                ..Retention::default()
            },
            integrity_schedule: "0 4 * * 0".into(),
            ..Pair::default()
        };
        let mut state = State::default();
        let now = Local::now();
        let ps = state.pair_mut("hist");
        ps.last_success = Some(now - chrono::Duration::hours(26));
        ps.last_backup = Some(RunRecord {
            started: now - chrono::Duration::hours(2),
            finished: now - chrono::Duration::hours(2),
            outcome: Outcome::Failed,
            message: "rsync exit 23: some files could not be copied".into(),
            snapshot: None,
            pruned: 0,
        });
        ps.stats = Some(Stats {
            measured: Some(now),
            source_bytes: Some(52_000_000_000),
            destination_bytes: Some(61_500_000_000),
            free_bytes: Some(900_000_000_000),
            filesystem: Some("apfs".into()),
            snapshot_count: 8,
            oldest: Some("2025-05-01T020000".into()),
            newest: Some("2026-10-11T020000".into()),
            error: None,
        });
        ps.last_integrity = Some(IntegrityRecord {
            finished: Some(now - chrono::Duration::days(1)),
            outcome: Some(Outcome::Failed),
            message: "1 file(s) differ with the same size and date: possible corruption or tampering".into(),
            snapshot: "2026-10-11T020000".into(),
            silent: vec!["2024/IMG_0042.HEIC".into()],
            silent_count: 1,
            changed: vec!["notes.md".into()],
            changed_count: 1,
            ..IntegrityRecord::default()
        });
        let config = Config {
            rsync_path: None,
            pairs: vec![pair],
        };
        let engine = Engine::new(config, state, None);
        let mut h = harness(&engine);
        h.run_steps(6);
        h.get_by_label_contains("Photos, Failed 2 h ago");
        h.get_by_label_contains("rsync exit 23");
        h.get_by_label("Last success");
        h.get_by_label("61.5 GB  (unchanged files stored once)");
        h.get_by_label("8 snapshots on disk: this policy keeps 6 and removes 2 after the next backup.");
        h.get_by_label("Same size and date, different content (1)");
        h.get_by_label("Preview").click();
        h.run_steps(6);
        assert_eq!(h.get_all_by_label("remove").count(), 2);
        assert_eq!(h.get_all_by_label("keep · monthly").count(), 2, "Sep 30 and Aug 31");
        h.get_by_label("keep · yearly");
        h.set_size(egui::vec2(980.0, 1900.0));
        h.run_steps(6);
        shot(&mut h, "3-history");
        h.set_size(egui::vec2(980.0, 900.0));

        // Editing the ladder updates the preview and marks the pair dirty; Revert undoes it.
        h.state_mut().draft.as_mut().unwrap().retention.keep_monthly = 0;
        h.run_steps(6);
        h.get_by_label("Unsaved changes");
        h.get_by_label("8 snapshots on disk: this policy keeps 4 and removes 4 after the next backup.");
        assert!(h.get_by_label("Back up now").accesskit_node().is_disabled());
        h.get_by_label("Revert").click();
        h.run_steps(6);
        assert!(h.query_by_label("Unsaved changes").is_none());
        assert!(!h.get_by_label("Back up now").accesskit_node().is_disabled());

        // Running backup shows progress and a Stop button.
        engine.lock().running = Some(Running {
            job: Job {
                pair_id: "hist".into(),
                kind: JobKind::Backup,
            },
            pair_name: "Photos".into(),
            started: now,
            progress: Some(Progress {
                bytes: 1_200_000_000,
                percent: 45,
                rate: "85.2MB/s".into(),
                eta: "0:02:10".into(),
                files_transferred: 310,
            }),
        });
        h.run_steps(6);
        h.get_by_label_contains("Photos, Backing up · 45%");
        h.get_by_label("Stop");
        shot(&mut h, "4-running");
    }

    #[test]
    fn destination_suggestion() {
        let tmp = tempfile::tempdir().unwrap();
        let drive = tmp.path().join("Drive");
        fs::create_dir_all(drive.join("Other stuff")).unwrap();
        let source = Path::new("/Users/a/Documents");
        assert_eq!(suggested_destination(&drive, source), drive.join("Documents snapshots"));
        let empty = tmp.path().join("Empty");
        fs::create_dir_all(&empty).unwrap();
        assert_eq!(suggested_destination(&empty, source), empty);
    }
}
