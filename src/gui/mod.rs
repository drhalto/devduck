//! The window. State and behaviour live here; drawing is in `views`.
//!
//! A worker thread owns the `Scanner` and rescans on a timer (live mode) or on demand;
//! HTTP probes, stops, restarts and log reads all run on their own threads and report
//! back over one channel, so the UI thread never blocks.

mod icons;
mod theme;
mod views;
mod widgets;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::probe::{self, Probe};
use crate::scan::{self, Kind, PortApp, Scanner};
use crate::store::{self, Favorite, FavoriteKind, Recent, State, ViewMode};
use crate::{icon, launch, win};

pub fn run(demo: bool) -> eframe::Result {
    let icon = egui::IconData { rgba: icon::rgba(64), width: 64, height: 64 };
    let shot = std::env::var_os("DEVPORTS_SHOT").is_some();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Dev Ports")
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([760.0, 480.0])
        .with_icon(Arc::new(icon));
    if shot {
        // Screenshot mode: never appear on (or take focus from) the desktop.
        viewport = viewport.with_position([-6000.0, 120.0]).with_active(false).with_taskbar(false);
    }
    let options = eframe::NativeOptions { viewport, centered: !shot, ..Default::default() };
    eframe::run_native("Dev Ports", options, Box::new(move |cc| Ok(Box::new(DevPorts::new(cc, demo)))))
}

const HISTORY: usize = 60;
const UNAVAILABLE: &str = "(command line unavailable)";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Chip {
    All,
    Dev,
    Docker,
    Apps,
    System,
}

struct Live {
    cpu: VecDeque<f32>,
    mem: VecDeque<f32>,
    first_seen: Instant,
}

struct ProbeEntry {
    /// None while the probe is in flight.
    result: Option<Probe>,
    at: Instant,
}

struct Toast {
    text: String,
    ok: bool,
    at: Instant,
}

enum Dialog {
    ConfirmStop(String),
    Error(String, String),
    Settings,
}

struct Launched {
    folder: String,
    pid: u32,
    log: String,
}

enum Msg {
    Scanned(Result<Vec<PortApp>, String>, f32),
    Probed(String, u16, Probe),
    Done { key: String, toast: Option<String>, error: Option<(String, String)>, launched: Option<Launched> },
    DockerLogs(String, String),
}

enum WorkerCmd {
    ScanNow,
    Interval(u32),
}

/// Everything the views can ask for. Collected while drawing, applied afterwards.
#[derive(Clone, Debug)]
pub(crate) enum Action {
    Select(String),
    CloseDrawer,
    OpenPort(u16),
    OpenMain(String),
    Copy(String, String),
    Folder(String),
    Editor(String),
    Terminal(String),
    AskStop(String),
    Stop(String),
    Restart(String),
    Start(String),
    Forget(String),
    Refresh,
    ToggleLive,
    SetChip(Chip),
    SetView(ViewMode),
    OpenSettings,
    Reprobe(String),
    /// Star or unstar a running row (dev project or container) by its key.
    ToggleFavorite(String),
    /// Star or unstar a recently stopped project by folder.
    ToggleFavoriteRecent(String),
    RemoveFavorite(String),
    StartFavorite(String),
    StartAllFavorites,
    ToggleSidebar,
}

/// What the status light on a row means.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Health {
    Probing,
    Ok(u16, u32),
    ClientError(u16),
    ServerError(u16),
    Silent,
    NotHttp,
    /// Only dynamic-range loopback ports: build workers, IPC, debuggers. Never probed.
    Internal,
    Docker,
    Plain,
}

pub(crate) struct LogView {
    key: String,
    text: String,
    fetched: Instant,
    source: &'static str,
    path: Option<String>,
}

/// Developer hook for README screenshots and visual checks that never touch the real
/// desktop: with DEVPORTS_SHOT=<file.bmp> the window opens off-screen and unfocused,
/// applies DEVPORTS_UI ("select=<app>;view=list;chip=docker;search=x;dialog=settings|stop;
/// act=restart|stop|start|favorite|startfav:<app>;delay=<ms>"), saves one frame as a BMP and exits.
struct ShotHook {
    path: String,
    started: Instant,
    delay: Duration,
    requested: bool,
    select: Option<String>,
    dialog: Option<String>,
    act: Option<(String, String)>,
    drawer_scroll: Option<f32>,
}

pub(crate) struct DevPorts {
    demo: bool,
    shot: Option<ShotHook>,
    ctx: egui::Context,
    state: State,
    state_dirty_since: Option<Instant>,
    rows: Vec<PortApp>,
    live: HashMap<String, Live>,
    probes: HashMap<String, BTreeMap<u16, ProbeEntry>>,
    scanned: Option<Instant>,
    scan_clock: String,
    scan_ms: f32,
    search: String,
    chip: Chip,
    selected: Option<String>,
    reselect_folder: Option<String>,
    drawer: bool,
    dialog: Option<Dialog>,
    toasts: Vec<Toast>,
    busy: HashMap<String, &'static str>,
    logs: Option<LogView>,
    logs_inflight: bool,
    lan_ip: Option<IpAddr>,
    titlebar_styled: bool,
    last_interval: u32,
    focus_search: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    worker: Sender<WorkerCmd>,
}

impl DevPorts {
    fn new(cc: &eframe::CreationContext<'_>, demo: bool) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::install_style(&cc.egui_ctx);
        let mut state = if demo { demo_state() } else { State::load() };
        state.prune();
        let (tx, rx) = channel();
        let worker = spawn_worker(demo, tx.clone(), cc.egui_ctx.clone(), state.settings.interval_secs);
        let last_interval = if state.settings.interval_secs == 0 { 2 } else { state.settings.interval_secs };
        let shot = std::env::var("DEVPORTS_SHOT").ok().map(|path| ShotHook {
            path,
            started: Instant::now(),
            delay: Duration::from_millis(2500),
            requested: false,
            select: None,
            dialog: None,
            act: None,
            drawer_scroll: None,
        });
        let mut app = DevPorts {
            demo,
            shot,
            ctx: cc.egui_ctx.clone(),
            state,
            state_dirty_since: None,
            rows: Vec::new(),
            live: HashMap::new(),
            probes: HashMap::new(),
            scanned: None,
            scan_clock: String::new(),
            scan_ms: 0.0,
            search: String::new(),
            chip: Chip::All,
            selected: None,
            reselect_folder: None,
            drawer: false,
            dialog: None,
            toasts: Vec::new(),
            busy: HashMap::new(),
            logs: None,
            logs_inflight: false,
            lan_ip: if demo { Some(IpAddr::from([192, 168, 1, 24])) } else { launch::lan_ip() },
            titlebar_styled: false,
            last_interval,
            focus_search: false,
            tx,
            rx,
            worker,
        };
        if app.shot.is_some() {
            app.apply_demo_ui(&std::env::var("DEVPORTS_UI").unwrap_or_default());
        }
        app
    }

    fn apply_demo_ui(&mut self, spec: &str) {
        for part in spec.split(';').filter(|p| !p.is_empty()) {
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            match k {
                "view" => self.state.settings.view = if v == "list" { ViewMode::List } else { ViewMode::Cards },
                "chip" => {
                    self.chip = match v {
                        "dev" => Chip::Dev,
                        "docker" => Chip::Docker,
                        "apps" => Chip::Apps,
                        "system" => Chip::System,
                        _ => Chip::All,
                    }
                }
                "search" => self.search = v.to_string(),
                "sidebar" => self.state.settings.sidebar = v != "0",
                "select" => self.shot.as_mut().unwrap().select = Some(v.to_string()),
                "dialog" => self.shot.as_mut().unwrap().dialog = Some(v.to_string()),
                "act" => {
                    if let Some((what, app)) = v.split_once(':') {
                        self.shot.as_mut().unwrap().act = Some((what.to_string(), app.to_string()));
                    }
                }
                "scroll" => self.shot.as_mut().unwrap().drawer_scroll = v.parse().ok(),
                "delay" => {
                    if let Ok(ms) = v.parse() {
                        self.shot.as_mut().unwrap().delay = Duration::from_millis(ms);
                    }
                }
                _ => {}
            }
        }
    }

    fn run_shot_hook(&mut self) {
        let Some(hook) = self.shot.as_mut() else { return };
        self.ctx.request_repaint();
        if let Some(app) = hook.select.clone()
            && let Some(r) = self.rows.iter().find(|r| r.app == app)
        {
            let key = r.key.clone();
            hook.select = None;
            self.selected = Some(key.clone());
            self.drawer = true;
            if hook.dialog.as_deref() == Some("stop") {
                self.dialog = Some(Dialog::ConfirmStop(key));
                hook.dialog = None;
            }
        }
        let hook = self.shot.as_mut().unwrap();
        if let Some((what, app)) = hook.act.clone() {
            let row_key = self.rows.iter().find(|r| r.app == app).map(|r| r.key.clone());
            let folder = self.state.recent.iter().find(|e| e.app == app).map(|e| e.folder.clone());
            let fav_id = self.state.favorites.iter().find(|f| f.name == app).map(Favorite::id);
            let action = match (what.as_str(), row_key, folder) {
                ("restart", Some(k), _) => Some(Action::Restart(k)),
                ("stop", Some(k), _) => Some(Action::Stop(k)),
                ("start", None, Some(f)) => Some(Action::Start(f)),
                ("favorite", Some(k), _) => Some(Action::ToggleFavorite(k)),
                ("startfav", _, _) => fav_id.map(Action::StartFavorite),
                _ => None,
            };
            if let Some(a) = action {
                self.shot.as_mut().unwrap().act = None;
                self.act(a);
            }
        }
        let hook = self.shot.as_mut().unwrap();
        if hook.dialog.as_deref() == Some("settings") {
            self.dialog = Some(Dialog::Settings);
            hook.dialog = None;
        }
        let shot = self.ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let hook = self.shot.as_mut().unwrap();
        if let Some(image) = shot {
            let _ = write_bmp(&hook.path, &image);
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if !hook.requested && self.scanned.is_some() && hook.started.elapsed() > hook.delay {
            hook.requested = true;
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
    }

    // ------------------------------------------------------------------ data --

    fn row(&self, key: &str) -> Option<&PortApp> {
        self.rows.iter().find(|r| r.key == key)
    }

    /// The favorite a running row belongs to, if it's starred.
    pub(crate) fn favorite_of(&self, r: &PortApp) -> Option<&Favorite> {
        self.state.favorites.iter().find(|f| match (f.kind, r.kind) {
            (FavoriteKind::Project, Kind::Dev) => !r.folder.is_empty() && f.target.eq_ignore_ascii_case(&r.folder),
            (FavoriteKind::Container, Kind::Docker) => f.target == r.docker_name,
            _ => false,
        })
    }

    /// The running row for a favorite, if it's up.
    pub(crate) fn running_row(&self, f: &Favorite) -> Option<&PortApp> {
        self.rows.iter().find(|r| match (f.kind, r.kind) {
            (FavoriteKind::Project, Kind::Dev) => f.target.eq_ignore_ascii_case(&r.folder),
            (FavoriteKind::Container, Kind::Docker) => f.target == r.docker_name,
            _ => false,
        })
    }

    pub(crate) fn can_favorite(r: &PortApp) -> bool {
        (r.kind == Kind::Dev && !r.folder.is_empty()) || r.kind == Kind::Docker
    }

    /// Whether a favorite is being started (from the sidebar or Recently stopped).
    pub(crate) fn favorite_busy(&self, f: &Favorite) -> bool {
        self.busy.contains_key(&format!("fav:{}", f.id())) || (f.kind == FavoriteKind::Project && self.busy.contains_key(&format!("start:{}", f.target)))
    }

    fn probe_of(&self, key: &str, port: u16) -> Option<&ProbeEntry> {
        self.probes.get(key).and_then(|m| m.get(&port))
    }

    /// The port to open: the first one serving HTML, else any HTTP, else the first
    /// non-ephemeral one.
    pub(crate) fn main_port(&self, r: &PortApp) -> u16 {
        let result = |p: u16| self.probe_of(&r.key, p).and_then(|e| e.result.as_ref());
        r.ports
            .iter()
            .find(|p| matches!(result(p.port), Some(Probe::Http { html: true, .. })))
            .or_else(|| r.ports.iter().find(|p| result(p.port).is_some_and(Probe::is_http)))
            .or_else(|| r.ports.iter().find(|p| !p.is_ephemeral()))
            .unwrap_or(&r.ports[0])
            .port
    }

    pub(crate) fn health(&self, r: &PortApp) -> Health {
        match r.kind {
            Kind::Docker => return Health::Docker,
            Kind::App | Kind::System => return Health::Plain,
            Kind::Dev => {}
        }
        if r.ports.iter().all(|p| p.is_ephemeral()) {
            return Health::Internal;
        }
        if !self.state.settings.probe_http {
            return Health::Plain;
        }
        let Some(entry) = self.probe_of(&r.key, self.main_port(r)) else { return Health::Probing };
        match &entry.result {
            None => Health::Probing,
            Some(Probe::Http { status, millis, .. }) => match status {
                500.. => Health::ServerError(*status),
                400..500 => Health::ClientError(*status),
                _ => Health::Ok(*status, *millis),
            },
            Some(Probe::Timeout) => Health::Silent,
            Some(Probe::NotHttp) => Health::NotHttp,
        }
    }

    pub(crate) fn page_title(&self, r: &PortApp) -> Option<&str> {
        match self.probe_of(&r.key, self.main_port(r))?.result.as_ref()? {
            Probe::Http { title: Some(t), .. } => Some(t.as_str()),
            _ => None,
        }
    }

    fn launched_log(&self, r: &PortApp) -> Option<String> {
        if r.folder.is_empty() {
            return None;
        }
        let entry = self.state.recent.iter().find(|e| e.folder.eq_ignore_ascii_case(&r.folder))?;
        let pid = entry.launched_pid?;
        let ours = r.kill_pid == pid || r.tree.iter().any(|t| t.pid == pid);
        if ours { entry.log.clone() } else { None }
    }

    fn mark_dirty(&mut self) {
        self.state_dirty_since.get_or_insert_with(Instant::now);
    }

    fn save_now(&mut self) {
        if !self.demo {
            self.state.save();
        }
        self.state_dirty_since = None;
    }

    fn toast(&mut self, text: impl Into<String>, ok: bool) {
        self.toasts.push(Toast { text: text.into(), ok, at: Instant::now() });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    fn scan_now(&self) {
        let _ = self.worker.send(WorkerCmd::ScanNow);
    }

    /// Ask for scans a little later too: a (re)started server takes a moment to bind.
    fn scan_soon(&self) {
        self.scan_now();
        let worker = self.worker.clone();
        std::thread::spawn(move || {
            for wait in [1500, 2500, 4000] {
                std::thread::sleep(Duration::from_millis(wait));
                if worker.send(WorkerCmd::ScanNow).is_err() {
                    return;
                }
            }
        });
    }

    fn spawn_probe(&mut self, key: &str, port: u16, addrs: Vec<IpAddr>) {
        self.probes.entry(key.to_string()).or_default().insert(port, ProbeEntry { result: None, at: Instant::now() });
        let (tx, ctx, key) = (self.tx.clone(), self.ctx.clone(), key.to_string());
        std::thread::spawn(move || {
            let result = probe::probe(port, &addrs);
            let _ = tx.send(Msg::Probed(key, port, result));
            ctx.request_repaint();
        });
    }

    fn on_scan(&mut self, rows: Vec<PortApp>, millis: f32) {
        let now = Instant::now();
        let first = self.scanned.is_none();
        for r in &rows {
            let live = self.live.entry(r.key.clone()).or_insert_with(|| Live {
                cpu: VecDeque::new(),
                mem: VecDeque::new(),
                first_seen: if first { now - Duration::from_secs(60) } else { now },
            });
            if self.demo && live.mem.is_empty() {
                // Demo rows get a believable two minutes of history straight away.
                let mb = r.mem_bytes as f32 / (1 << 20) as f32;
                let cpu = r.cpu.unwrap_or(0.0);
                for i in 0..HISTORY {
                    let t = i as f32 * 0.37 + mb * 0.01;
                    live.mem.push_back(mb * (0.94 + 0.05 * (t * 0.6).sin() + 0.02 * (t * 2.3).cos()));
                    live.cpu.push_back((cpu * (0.6 + 0.5 * (t * 1.3).sin().abs())).max(0.0));
                }
            }
            if let Some(cpu) = r.cpu {
                live.cpu.push_back(cpu);
            }
            live.mem.push_back(r.mem_bytes as f32 / (1 << 20) as f32);
            while live.cpu.len() > HISTORY {
                live.cpu.pop_front();
            }
            while live.mem.len() > HISTORY {
                live.mem.pop_front();
            }
        }
        self.live.retain(|k, _| rows.iter().any(|r| &r.key == k));
        self.probes.retain(|k, _| rows.iter().any(|r| &r.key == k));

        // HTTP probes: once per (server, port), retried while a dev server is still compiling.
        if self.state.settings.probe_http {
            for r in rows.iter().filter(|r| r.kind == Kind::Dev) {
                for p in r.ports.iter().filter(|p| !p.is_ephemeral()) {
                    if self.demo {
                        let result = demo_probe(r, p.port);
                        self.probes.entry(r.key.clone()).or_default().insert(p.port, ProbeEntry { result: Some(result), at: now });
                        continue;
                    }
                    let stale = match self.probe_of(&r.key, p.port) {
                        None => true,
                        Some(ProbeEntry { result: Some(Probe::Timeout), at }) => at.elapsed() > Duration::from_secs(8),
                        Some(_) => false,
                    };
                    if stale {
                        self.spawn_probe(&r.key.clone(), p.port, p.addrs.clone());
                    }
                }
            }
        }

        // Remember dev projects so they can be started again after they stop.
        if !self.demo {
            let stamp = store::now_unix();
            for r in rows.iter().filter(|r| r.kind == Kind::Dev && !r.folder.is_empty()) {
                if self.state.recent_mut(&r.folder).is_none() {
                    self.state.recent.push(Recent { folder: r.folder.clone(), ..Default::default() });
                }
                let e = self.state.recent_mut(&r.folder).unwrap();
                e.app = r.app.clone();
                e.framework = r.framework.clone();
                e.ports = r.port_numbers();
                if !r.root_cmd.is_empty() && r.root_cmd != UNAVAILABLE {
                    e.command = r.root_cmd.clone();
                    e.cwd = r.cwd.clone().or_else(|| Some(r.folder.clone()));
                }
                e.last_seen = stamp;
            }
            if rows.iter().any(|r| r.kind == Kind::Dev) {
                self.mark_dirty();
            }
        }

        // Favorites remember how to start themselves; refresh that while they run.
        for f in &mut self.state.favorites {
            let running = rows.iter().find(|r| match (f.kind, r.kind) {
                (FavoriteKind::Project, Kind::Dev) => f.target.eq_ignore_ascii_case(&r.folder),
                (FavoriteKind::Container, Kind::Docker) => f.target == r.docker_name,
                _ => false,
            });
            if let Some(r) = running {
                f.name = r.app.clone();
                f.framework = r.framework.clone();
                f.ports = r.port_numbers();
                if f.kind == FavoriteKind::Project && !r.root_cmd.is_empty() && r.root_cmd != UNAVAILABLE {
                    f.command = r.root_cmd.clone();
                    f.cwd = r.cwd.clone().or_else(|| Some(r.folder.clone()));
                }
            }
        }

        if let Some(folder) = &self.reselect_folder
            && let Some(r) = rows.iter().find(|r| r.folder.eq_ignore_ascii_case(folder))
        {
            self.selected = Some(r.key.clone());
            self.reselect_folder = None;
        }
        if let Some(sel) = &self.selected
            && !rows.iter().any(|r| &r.key == sel)
            && self.reselect_folder.is_none()
        {
            self.selected = None;
            self.drawer = false;
        }
        self.busy.retain(|k, _| k.starts_with("fav:") || k.starts_with("start:") || rows.iter().any(|r| &r.key == k));
        self.rows = rows;
        self.scanned = Some(now);
        self.scan_clock = win::local_clock();
        self.scan_ms = millis;
    }

    fn poll(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Scanned(Ok(rows), ms) => self.on_scan(rows, ms),
                Msg::Scanned(Err(e), _) => self.toast(format!("Scan failed: {e}"), false),
                Msg::Probed(key, port, result) => {
                    if let Some(entry) = self.probes.get_mut(&key).and_then(|m| m.get_mut(&port)) {
                        entry.result = Some(result);
                        entry.at = Instant::now();
                    }
                }
                Msg::Done { key, toast, error, launched } => {
                    self.busy.remove(&key);
                    if let Some(t) = toast {
                        self.toast(t, true);
                    }
                    if let Some((title, body)) = error {
                        self.dialog = Some(Dialog::Error(title, body));
                    }
                    if let Some(l) = launched {
                        if self.state.recent_mut(&l.folder).is_none() {
                            let app = std::path::Path::new(&l.folder).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            self.state.recent.push(Recent { folder: l.folder.clone(), app, last_seen: store::now_unix(), ..Default::default() });
                        }
                        if let Some(e) = self.state.recent_mut(&l.folder) {
                            e.launched_pid = Some(l.pid);
                            e.log = Some(l.log);
                        }
                        self.reselect_folder = Some(l.folder);
                        self.save_now();
                    }
                    self.scan_soon();
                }
                Msg::DockerLogs(key, text) => {
                    self.logs_inflight = false;
                    self.logs = Some(LogView { key, text: launch::clean_log(&text), fetched: Instant::now(), source: "docker logs", path: None });
                }
            }
        }
    }

    /// Keep the drawer's log view fresh while it's open.
    fn refresh_logs(&mut self) {
        let Some(key) = self.selected.clone().filter(|_| self.drawer) else {
            self.logs = None;
            return;
        };
        let Some(r) = self.row(&key).cloned() else { return };
        let due = self.logs.as_ref().is_none_or(|l| l.key != key || l.fetched.elapsed() > Duration::from_millis(1500));
        if !due {
            return;
        }
        if r.kind == Kind::Docker {
            if self.logs_inflight || self.demo {
                if self.demo && self.logs.as_ref().is_none_or(|l| l.key != key) {
                    self.logs = Some(LogView { key, text: DEMO_DOCKER_LOG.into(), fetched: Instant::now(), source: "docker logs", path: None });
                }
                return;
            }
            self.logs_inflight = true;
            let (tx, ctx, name) = (self.tx.clone(), self.ctx.clone(), r.docker_name.clone());
            std::thread::spawn(move || {
                let text = launch::docker(&["logs", "--tail", "200", &name]).unwrap_or_else(|e| e);
                let _ = tx.send(Msg::DockerLogs(key, text));
                ctx.request_repaint();
            });
        } else if let Some(path) = self.launched_log(&r) {
            let text = launch::tail(std::path::Path::new(&path), 48 * 1024).unwrap_or_default();
            self.logs = Some(LogView { key, text, fetched: Instant::now(), source: "started by Dev Ports", path: Some(path) });
        } else if self.demo && r.kind == Kind::Dev {
            self.logs = Some(LogView { key, text: DEMO_DEV_LOG.into(), fetched: Instant::now(), source: "started by Dev Ports", path: None });
        } else {
            self.logs = None;
        }
    }

    // --------------------------------------------------------------- actions --

    fn act(&mut self, action: Action) {
        match action {
            Action::Select(key) => {
                if self.selected.as_deref() == Some(key.as_str()) && self.drawer {
                    return;
                }
                self.selected = Some(key.clone());
                self.drawer = true;
                self.logs = None;
                // Re-check a server's HTTP status when you look at it.
                if let Some(r) = self.row(&key).cloned()
                    && r.kind == Kind::Dev
                    && self.state.settings.probe_http
                    && !self.demo
                {
                    for p in r.ports.iter().filter(|p| !p.is_ephemeral()) {
                        let fresh = self.probe_of(&key, p.port).is_some_and(|e| e.result.is_none() || e.at.elapsed() < Duration::from_secs(15));
                        if !fresh {
                            self.spawn_probe(&key, p.port, p.addrs.clone());
                        }
                    }
                }
            }
            Action::CloseDrawer => self.drawer = false,
            Action::OpenPort(port) => {
                win::shell_open(&format!("http://localhost:{port}"));
            }
            Action::OpenMain(key) => {
                if let Some(r) = self.row(&key) {
                    let port = self.main_port(r);
                    win::shell_open(&format!("http://localhost:{port}"));
                }
            }
            Action::Copy(text, what) => {
                self.ctx.copy_text(text);
                self.toast(format!("Copied {what}"), true);
            }
            Action::Folder(path) => {
                if std::path::Path::new(&path).is_dir() {
                    win::shell_open(&path);
                } else {
                    self.toast("That folder no longer exists", false);
                }
            }
            Action::Editor(folder) => {
                let editor = self.state.settings.editor;
                match launch::open_in_editor(editor, &folder) {
                    Ok(()) => self.toast(format!("Opening in {}", launch::editor_name(editor)), true),
                    Err(e) => self.toast(e, false),
                }
            }
            Action::Terminal(folder) => {
                if let Err(e) = launch::open_terminal(&folder) {
                    self.toast(e, false);
                }
            }
            Action::AskStop(key) => {
                if self.state.settings.confirm_stop {
                    self.dialog = Some(Dialog::ConfirmStop(key));
                } else {
                    self.act(Action::Stop(key));
                }
            }
            Action::Stop(key) => self.stop(&key),
            Action::Restart(key) => self.restart(&key),
            Action::Start(folder) => self.start(&folder),
            Action::Forget(folder) => {
                self.state.recent.retain(|e| !e.folder.eq_ignore_ascii_case(&folder));
                self.save_now();
            }
            Action::Refresh => {
                self.scan_now();
                if let Some(key) = self.selected.clone() {
                    self.act(Action::Reprobe(key));
                }
            }
            Action::ToggleLive => {
                let s = &mut self.state.settings;
                if s.interval_secs == 0 {
                    s.interval_secs = self.last_interval.max(1);
                } else {
                    self.last_interval = s.interval_secs;
                    s.interval_secs = 0;
                }
                let _ = self.worker.send(WorkerCmd::Interval(self.state.settings.interval_secs));
                self.save_now();
            }
            Action::SetChip(chip) => self.chip = chip,
            Action::SetView(view) => {
                self.state.settings.view = view;
                self.save_now();
            }
            Action::OpenSettings => self.dialog = Some(Dialog::Settings),
            Action::ToggleFavorite(key) => {
                let Some(r) = self.row(&key).cloned() else { return };
                if let Some(id) = self.favorite_of(&r).map(Favorite::id) {
                    self.state.favorites.retain(|f| f.id() != id);
                    self.toast(format!("Removed {} from favorites", r.app), true);
                } else if Self::can_favorite(&r) {
                    let recent = self.state.recent.iter().find(|e| e.folder.eq_ignore_ascii_case(&r.folder));
                    let (command, cwd) = match self.relaunch_spec(&r) {
                        Some((c, d)) => (c, Some(d)),
                        None => (recent.map(|e| e.command.clone()).unwrap_or_default(), recent.and_then(|e| e.cwd.clone())),
                    };
                    let fav = if r.kind == Kind::Docker {
                        Favorite { kind: FavoriteKind::Container, target: r.docker_name.clone(), name: r.app.clone(), framework: r.framework.clone(), ports: r.port_numbers(), ..Default::default() }
                    } else {
                        Favorite { kind: FavoriteKind::Project, target: r.folder.clone(), name: r.app.clone(), framework: r.framework.clone(), ports: r.port_numbers(), command, cwd }
                    };
                    self.state.favorites.push(fav);
                    self.toast(format!("Added {} to favorites", r.app), true);
                }
                self.save_now();
            }
            Action::ToggleFavoriteRecent(folder) => {
                let id = format!("project:{}", folder.to_lowercase());
                if self.state.favorites.iter().any(|f| f.id() == id) {
                    self.state.favorites.retain(|f| f.id() != id);
                } else if let Some(e) = self.state.recent.iter().find(|e| e.folder.eq_ignore_ascii_case(&folder)).cloned() {
                    self.state.favorites.push(Favorite {
                        kind: FavoriteKind::Project,
                        target: e.folder.clone(),
                        name: e.app.clone(),
                        framework: e.framework.clone(),
                        ports: e.ports.clone(),
                        command: e.command.clone(),
                        cwd: e.cwd.clone(),
                    });
                    self.toast(format!("Added {} to favorites", e.app), true);
                }
                self.save_now();
            }
            Action::RemoveFavorite(id) => {
                self.state.favorites.retain(|f| f.id() != id);
                self.save_now();
            }
            Action::StartFavorite(id) => self.start_favorite(&id),
            Action::StartAllFavorites => {
                let stopped: Vec<String> = self
                    .state
                    .favorites
                    .iter()
                    .filter(|f| self.running_row(f).is_none() && !self.favorite_busy(f))
                    .map(Favorite::id)
                    .collect();
                for id in stopped {
                    self.start_favorite(&id);
                }
            }
            Action::ToggleSidebar => {
                self.state.settings.sidebar = !self.state.settings.sidebar;
                self.save_now();
            }
            Action::Reprobe(key) => {
                if let Some(r) = self.row(&key).cloned()
                    && r.kind == Kind::Dev
                    && !self.demo
                {
                    for p in r.ports.iter().filter(|p| !p.is_ephemeral()) {
                        self.spawn_probe(&key, p.port, p.addrs.clone());
                    }
                }
            }
        }
    }

    fn stop(&mut self, key: &str) {
        let Some(r) = self.row(key).cloned() else { return };
        if self.demo {
            self.toast(format!("Demo mode: {} was not stopped", r.app), false);
            return;
        }
        self.busy.insert(key.to_string(), "Stopping");
        let (tx, ctx, key) = (self.tx.clone(), self.ctx.clone(), key.to_string());
        std::thread::spawn(move || {
            let result = if r.kind == Kind::Docker {
                launch::docker(&["stop", "-t", "2", &r.docker_name]).map(drop)
            } else {
                launch::kill_tree(r.kill_pid)
            };
            let msg = match result {
                Ok(()) => Msg::Done { key, toast: Some(format!("Stopped {}", r.app)), error: None, launched: None },
                Err(e) => Msg::Done { key, toast: None, error: Some((format!("Couldn't stop {}", r.app), e)), launched: None },
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// The command and directory that would bring this server back.
    fn relaunch_spec(&self, r: &PortApp) -> Option<(String, String)> {
        let recent = self.state.recent.iter().find(|e| !r.folder.is_empty() && e.folder.eq_ignore_ascii_case(&r.folder));
        let command = if !r.root_cmd.is_empty() && r.root_cmd != UNAVAILABLE {
            r.root_cmd.clone()
        } else {
            recent.map(|e| e.command.clone()).filter(|c| !c.is_empty())?
        };
        let cwd = r.cwd.clone().or_else(|| recent.and_then(|e| e.cwd.clone())).unwrap_or_else(|| r.folder.clone());
        (!cwd.is_empty()).then_some((command, cwd))
    }

    pub(crate) fn can_restart(&self, r: &PortApp) -> bool {
        match r.kind {
            Kind::Docker => true,
            Kind::Dev => self.relaunch_spec(r).is_some(),
            _ => false,
        }
    }

    fn restart(&mut self, key: &str) {
        let Some(r) = self.row(key).cloned() else { return };
        if self.demo {
            self.toast(format!("Demo mode: {} was not restarted", r.app), false);
            return;
        }
        let (tx, ctx, key_s) = (self.tx.clone(), self.ctx.clone(), key.to_string());
        if r.kind == Kind::Docker {
            self.busy.insert(key_s.clone(), "Restarting");
            std::thread::spawn(move || {
                let msg = match launch::docker(&["restart", "-t", "2", &r.docker_name]) {
                    Ok(_) => Msg::Done { key: key_s, toast: Some(format!("Restarted {}", r.app)), error: None, launched: None },
                    Err(e) => Msg::Done { key: key_s, toast: None, error: Some((format!("Couldn't restart {}", r.app), e)), launched: None },
                };
                let _ = tx.send(msg);
                ctx.request_repaint();
            });
            return;
        }
        let Some((command, cwd)) = self.relaunch_spec(&r) else {
            self.dialog = Some(Dialog::Error(
                format!("Can't restart {}", r.app),
                "Dev Ports couldn't read this server's command line (it may be running as another user), so it doesn't know how to start it again.".into(),
            ));
            return;
        };
        self.busy.insert(key_s.clone(), "Restarting");
        let ports = r.port_numbers();
        let folder = r.folder.clone();
        std::thread::spawn(move || {
            let msg = match launch::kill_tree(r.kill_pid) {
                Err(e) => Msg::Done { key: key_s, toast: None, error: Some((format!("Couldn't stop {}", r.app), e)), launched: None },
                Ok(()) => {
                    launch::wait_ports_free(&ports, Duration::from_secs(6));
                    match launch::start(&r.app, &command, &cwd) {
                        Ok((pid, log)) => Msg::Done {
                            key: key_s,
                            toast: Some(format!("Restarted {}", r.app)),
                            error: None,
                            launched: Some(Launched { folder, pid, log: log.to_string_lossy().into_owned() }),
                        },
                        Err(e) => Msg::Done { key: key_s, toast: None, error: Some((format!("Stopped {}, but couldn't start it again", r.app), e)), launched: None },
                    }
                }
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn start(&mut self, folder: &str) {
        let Some(e) = self.state.recent.iter().find(|e| e.folder.eq_ignore_ascii_case(folder)).cloned() else { return };
        let cwd = e.cwd.clone().unwrap_or_else(|| e.folder.clone());
        self.launch_project(format!("start:{}", e.folder), &e.app, &e.command, &cwd, &e.folder);
    }

    fn start_favorite(&mut self, id: &str) {
        let Some(f) = self.state.favorites.iter().find(|f| f.id() == id).cloned() else { return };
        if self.running_row(&f).is_some() {
            return;
        }
        let busy_key = format!("fav:{id}");
        match f.kind {
            FavoriteKind::Project => {
                // Fall back to what Recently stopped knows if the favorite never saw it run.
                let recent = self.state.recent.iter().find(|e| e.folder.eq_ignore_ascii_case(&f.target));
                let command = if f.command.is_empty() { recent.map(|e| e.command.clone()).unwrap_or_default() } else { f.command.clone() };
                let cwd = f.cwd.clone().or_else(|| recent.and_then(|e| e.cwd.clone())).unwrap_or_else(|| f.target.clone());
                self.launch_project(busy_key, &f.name, &command, &cwd, &f.target);
            }
            FavoriteKind::Container => {
                if self.demo {
                    self.toast(format!("Demo mode: {} was not started", f.name), false);
                    return;
                }
                self.busy.insert(busy_key.clone(), "Starting");
                let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
                std::thread::spawn(move || {
                    let msg = match launch::docker(&["start", &f.target]) {
                        Ok(_) => Msg::Done { key: busy_key, toast: Some(format!("Started {}", f.name)), error: None, launched: None },
                        Err(e) => {
                            let body = if e.contains("error during connect") || e.contains("Couldn't run docker") {
                                "Docker isn't running. Start Docker Desktop, then try again.".to_string()
                            } else {
                                e
                            };
                            Msg::Done { key: busy_key, toast: None, error: Some((format!("Couldn't start {}", f.name), body)), launched: None }
                        }
                    };
                    let _ = tx.send(msg);
                    ctx.request_repaint();
                });
            }
        }
    }

    /// Run a remembered dev-server command in its folder, with output captured to a log.
    fn launch_project(&mut self, busy_key: String, app: &str, command: &str, cwd: &str, folder: &str) {
        if self.demo {
            self.toast(format!("Demo mode: {app} was not started"), false);
            return;
        }
        if command.is_empty() {
            self.dialog = Some(Dialog::Error(format!("Can't start {app}"), "Dev Ports never saw this project's command line, so it doesn't know how to start it. Run it once as usual and it will remember.".into()));
            return;
        }
        self.busy.insert(busy_key.clone(), "Starting");
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let (app, command, cwd, folder) = (app.to_string(), command.to_string(), cwd.to_string(), folder.to_string());
        std::thread::spawn(move || {
            let msg = match launch::start(&app, &command, &cwd) {
                Ok((pid, log)) => Msg::Done {
                    key: busy_key,
                    toast: Some(format!("Starting {app}")),
                    error: None,
                    launched: Some(Launched { folder, pid, log: log.to_string_lossy().into_owned() }),
                },
                Err(err) => Msg::Done { key: busy_key, toast: None, error: Some((format!("Couldn't start {app}"), err)), launched: None },
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }
}

impl eframe::App for DevPorts {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if !self.titlebar_styled {
            self.titlebar_styled = win::style_title_bar(theme::rgb(theme::BG), theme::rgb(theme::TEXT_2), theme::rgb(theme::BORDER));
        }
        self.poll();
        self.refresh_logs();
        self.run_shot_hook();

        let mut actions = Vec::new();
        if self.dialog.is_none() {
            self.keyboard(&mut actions);
        }
        self.draw(ui, &mut actions);
        for a in actions {
            self.act(a);
        }

        self.toasts.retain(|t| t.at.elapsed() < Duration::from_millis(3600));
        if let Some(since) = self.state_dirty_since
            && since.elapsed() > Duration::from_secs(15)
        {
            self.save_now();
        }
        // Keep animating while something is fading (new rows, toasts, the live pulse).
        let animating = !self.toasts.is_empty()
            || self.scanned.is_some_and(|t| t.elapsed() < Duration::from_millis(900))
            || self.live.values().any(|l| l.first_seen.elapsed() < Duration::from_millis(2600))
            || !self.busy.is_empty();
        if animating {
            self.ctx.request_repaint_after(Duration::from_millis(33));
        }
        if self.drawer {
            self.ctx.request_repaint_after(Duration::from_millis(1600));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_now();
    }
}

fn spawn_worker(demo: bool, tx: Sender<Msg>, ctx: egui::Context, interval: u32) -> Sender<WorkerCmd> {
    let (cmd_tx, cmd_rx) = channel();
    std::thread::spawn(move || {
        let mut scanner = Scanner::new();
        let mut interval = interval;
        let mut tick = 0u64;
        let mut fresh_docker = true;
        loop {
            let started = Instant::now();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if demo { scan::demo_rows(tick) } else { scanner.scan(fresh_docker) }
            }))
            .map_err(|e| e.downcast_ref::<&str>().map(|s| s.to_string()).unwrap_or_else(|| "internal error".into()));
            tick += 1;
            fresh_docker = false;
            if tx.send(Msg::Scanned(result, started.elapsed().as_secs_f32() * 1000.0)).is_err() {
                return;
            }
            ctx.request_repaint();
            let deadline = (interval > 0).then(|| Instant::now() + Duration::from_secs(interval as u64));
            loop {
                let got = match deadline {
                    Some(t) => cmd_rx.recv_timeout(t.saturating_duration_since(Instant::now())),
                    None => cmd_rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                match got {
                    Ok(WorkerCmd::ScanNow) => {
                        fresh_docker = true;
                        break;
                    }
                    Ok(WorkerCmd::Interval(n)) => {
                        interval = n;
                        if n > 0 {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    });
    cmd_tx
}

fn write_bmp(path: &str, image: &egui::ColorImage) -> std::io::Result<()> {
    let [w, h] = image.size;
    let row = (w * 3).div_ceil(4) * 4;
    let mut out = Vec::with_capacity(54 + row * h);
    out.extend(b"BM");
    out.extend(((54 + row * h) as u32).to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(54u32.to_le_bytes());
    out.extend(40u32.to_le_bytes());
    out.extend((w as i32).to_le_bytes());
    out.extend((h as i32).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(24u16.to_le_bytes());
    out.extend([0u8; 24]);
    for y in (0..h).rev() {
        let start = out.len();
        for x in 0..w {
            let c = image.pixels[y * w + x];
            out.extend([c.b(), c.g(), c.r()]);
        }
        out.resize(start + row, 0);
    }
    std::fs::write(path, out)
}

// ------------------------------------------------------------------ demo ------

fn demo_state() -> State {
    let now = store::now_unix();
    let recent = vec![
        Recent { folder: r"C:\Users\dev\docs-site".into(), app: "docs-site".into(), framework: Some("Astro".into()), ports: vec![4321], command: "npm run dev".into(), cwd: Some(r"C:\Users\dev\docs-site".into()), last_seen: now - 3 * 3600, ..Default::default() },
        Recent { folder: r"C:\Users\dev\ml-notebooks".into(), app: "ml-notebooks".into(), framework: Some("Jupyter".into()), ports: vec![8888], command: "jupyter lab".into(), cwd: None, last_seen: now - 86_400 - 3600, ..Default::default() },
    ];
    let fav = |kind, target: &str, name: &str, fw: &str, ports: Vec<u16>| Favorite {
        kind,
        target: target.into(),
        name: name.into(),
        framework: Some(fw.into()),
        ports,
        command: "npm run dev".into(),
        cwd: None,
    };
    let favorites = vec![
        fav(FavoriteKind::Project, r"C:\Users\dev\acme-storefront", "acme-storefront", "Next.js", vec![3000]),
        fav(FavoriteKind::Container, "postgres-16", "postgres-16", "postgres", vec![5432]),
        fav(FavoriteKind::Project, r"C:\Users\dev\docs-site", "docs-site", "Astro", vec![4321]),
        fav(FavoriteKind::Container, "billing-db", "billing-db", "postgres", vec![5433]),
    ];
    State { recent, favorites, ..Default::default() }
}

fn demo_probe(r: &PortApp, port: u16) -> Probe {
    let title = match r.app.as_str() {
        "acme-storefront" => Some("Acme \u{2014} Spring collection"),
        "marketing-site" => Some("Acme \u{00B7} Launch week"),
        "launch-film" => Some("Remotion Studio"),
        _ => None,
    };
    match (r.app.as_str(), port) {
        (_, p) if p >= 49152 => Probe::NotHttp,
        ("internal-api", _) => Probe::Http { status: 404, title: None, millis: 4, html: false },
        _ => Probe::Http { status: 200, title: title.map(str::to_string), millis: 18 + (port % 40) as u32, html: true },
    }
}

const DEMO_DEV_LOG: &str = "[dev ports] 14:02:11 in C:\\Users\\dev\\acme-storefront\n[dev ports] npm run dev\n\n> acme-storefront@0.4.0 dev\n> next dev --turbopack\n\n   \u{25B2} Next.js 16.0.1 (Turbopack)\n   - Local:        http://localhost:3000\n   - Network:      http://192.168.1.24:3000\n\n \u{2713} Starting...\n \u{2713} Ready in 1180ms\n \u{25CB} Compiling / ...\n GET / 200 in 2.1s\n GET /products/linen-shirt 200 in 412ms\n GET /api/cart 200 in 38ms\n";
const DEMO_DOCKER_LOG: &str = "PostgreSQL init process complete; ready for start up.\n\n2026-10-04 09:12:01.233 UTC [1] LOG:  starting PostgreSQL 16.4 on x86_64-pc-linux-gnu\n2026-10-04 09:12:01.234 UTC [1] LOG:  listening on IPv4 address \"0.0.0.0\", port 5432\n2026-10-04 09:12:01.240 UTC [1] LOG:  database system is ready to accept connections\n";
