//! The window. State and behaviour live here; drawing is in `views`.
//!
//! A worker thread owns the `Scanner` and rescans on a timer (live mode) or on demand;
//! HTTP probes, stops, restarts and log reads all run on their own threads and report
//! back over one channel, so the UI thread never blocks.

mod control;
mod icons;
mod model;
mod theme;
mod views;
mod widgets;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::probe::{self, Probe};
use crate::projects::{self, Project};
use crate::scan::{self, Kind, PortApp, Scanner};
use crate::store::{self, Favorite, FavoriteKind, Member, Recent, Stack, State, ViewMode};
use crate::{icon, launch, win};

pub(crate) use model::{RunState, container_id, project_id, row_id};
use model::{RunSpec, start_key};

pub fn run(demo: bool) -> eframe::Result {
    let icon = egui::IconData { rgba: icon::ICON_64.to_vec(), width: 64, height: 64 };
    let shot = std::env::var_os("DEVDUCK_SHOT").is_some();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("DevDuck")
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([760.0, 480.0])
        .with_icon(Arc::new(icon));
    if shot {
        // Screenshot mode: never appear on (or take focus from) the desktop.
        viewport = viewport.with_position([-6000.0, 120.0]).with_active(false).with_taskbar(false);
    }
    let options = eframe::NativeOptions { viewport, centered: !shot, ..Default::default() };
    eframe::run_native("DevDuck", options, Box::new(move |cc| Ok(Box::new(DevDuck::new(cc, demo)))))
}

const HISTORY: usize = 60;
const UNAVAILABLE: &str = "(command line unavailable)";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Chip {
    /// Everything you can start or stop: projects and containers.
    Projects,
    /// One stack's members.
    Stack(usize),
    /// The port lens: every listener.
    All,
    Docker,
    Apps,
    System,
}

impl Chip {
    pub(crate) fn is_projects(self) -> bool {
        matches!(self, Chip::Projects | Chip::Stack(_))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Tab {
    #[default]
    Overview,
    Output,
    Details,
}

/// How a project that DevDuck started finished.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Exit {
    pub code: Option<i32>,
    pub at: u64,
}

/// Ctrl K: type to find a command, Enter to run it.
#[derive(Default)]
pub(crate) struct Palette {
    pub query: String,
    pub selected: usize,
    pub focused: bool,
}

/// The stack being created or edited.
#[derive(Clone, Default, Debug)]
pub(crate) struct StackDraft {
    pub index: Option<usize>,
    pub name: String,
    pub members: Vec<Member>,
    pub filter: String,
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
    /// Starting `folder` would collide with `holder` (a row key) on `port`.
    PortConflict { folder: String, script: Option<String>, port: u16, holder: String, free: u16 },
    Stack(StackDraft),
}

struct Launched {
    folder: String,
    pid: u32,
    log: String,
    child: Option<std::process::Child>,
    script: Option<String>,
    focus: bool,
}

enum Msg {
    Scanned(Result<Vec<PortApp>, String>, f32),
    Probed(String, u16, Probe),
    Done { key: String, toast: Option<String>, error: Option<(String, String)>, launched: Option<Launched> },
    DockerLogs(String, String),
    Projects(Vec<Project>),
    /// git status by lowercase folder; `true` when it covered every project.
    Git(Vec<(String, Option<crate::git::GitInfo>)>, bool),
    Icons(Vec<(String, egui::ColorImage)>),
}

enum WorkerCmd {
    ScanNow,
    Interval(u32),
}

/// Everything the views can ask for. Collected while drawing, applied afterwards.
#[derive(Clone, Debug)]
pub(crate) enum Action {
    /// Select an item (project, container, or a listener's row key) and open its page.
    Select(String),
    /// Select and show a tab.
    Show(String, Tab),
    SetTab(Tab),
    /// Start a project folder: a named script, or whatever it ran last.
    Run(String, Option<String>),
    /// Start a project on a port other than its usual one.
    RunOnPort(String, Option<String>, u16),
    /// Start it although a port it wants is taken, stopping that row (key) first if given.
    RunForce(String, Option<String>, Option<String>),
    /// Stop an item by id: a running row, or a project still starting.
    StopItem(String),
    StartContainer(String),
    DismissExit(String),
    OpenPalette,
    StartStack(usize),
    StopStack(usize),
    NewStack,
    EditStack(usize),
    DeleteStack(usize),
    SaveStack(StackDraft),
    /// Add or remove an item from a stack.
    ToggleInStack(usize, Member),
    StopAllDev,
    RescanProjects,
    AddRoot(String),
    RemoveRoot(String),
    CloseDrawer,
    /// A web page (a repo on GitHub).
    OpenWeb(String),
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
    /// False when the server running now was started outside DevDuck (the log, if any,
    /// is from an earlier run).
    ours: bool,
}

/// Developer hook for README screenshots and visual checks that never touch the real
/// desktop: with DEVDUCK_SHOT=<file.bmp> the window opens off-screen and unfocused,
/// applies DEVDUCK_UI ("select=<app>;tab=output|details;view=list;chip=all|docker|stack;search=x;
/// dialog=settings|stop|stack|conflict:<app>;palette=<query>;act=run|restart|stop|start|favorite|startfav:<app>;
/// main_scroll=<px>;delay=<ms>"), saves one frame as a BMP and exits.
struct ShotHook {
    path: String,
    started: Instant,
    delay: Duration,
    requested: bool,
    select: Option<String>,
    dialog: Option<String>,
    act: Option<(String, String)>,
    drawer_scroll: Option<f32>,
    tab: Option<Tab>,
    palette: Option<String>,
    main_scroll: Option<f32>,
}

pub(crate) struct DevDuck {
    demo: bool,
    shot: Option<ShotHook>,
    ctx: egui::Context,
    logo: egui::TextureHandle,
    /// How many recent tiles fit in one line, from the last frame's width.
    recent_slots: std::cell::Cell<usize>,
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
    projects: Vec<Project>,
    /// git status by lowercase folder (None: not in a repo).
    git: HashMap<String, Option<crate::git::GitInfo>>,
    git_at: Option<Instant>,
    git_inflight: bool,
    /// When the open page's project was last re-read.
    git_one: Option<(String, Instant)>,
    /// Each project's own icon, by lowercase folder.
    icons: HashMap<String, egui::TextureHandle>,
    projects_loading: bool,
    /// Processes DevDuck started, by lowercased project folder.
    children: HashMap<String, std::process::Child>,
    /// Started but not listening yet, by lowercased folder.
    launching: HashMap<String, Instant>,
    exits: HashMap<String, Exit>,
    /// Folders DevDuck is stopping itself, so their exit isn't reported as a crash.
    expected_exit: HashSet<String>,
    /// The last line of output of each starting project, for its card.
    last_lines: HashMap<String, (Instant, String)>,
    tab: Tab,
    palette: Option<Palette>,
    root_input: String,
    /// Project folders a stack start is holding until its containers are up.
    stack_queue: Vec<String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    worker: Sender<WorkerCmd>,
}

impl DevDuck {
    fn new(cc: &eframe::CreationContext<'_>, demo: bool) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::install_style(&cc.egui_ctx);
        let mut state = if demo { demo_state() } else { State::load() };
        state.prune();
        let (tx, rx) = channel();
        let worker = spawn_worker(demo, tx.clone(), cc.egui_ctx.clone(), state.settings.interval_secs);
        let last_interval = if state.settings.interval_secs == 0 { 2 } else { state.settings.interval_secs };
        let shot = std::env::var("DEVDUCK_SHOT").ok().map(|path| ShotHook {
            path,
            started: Instant::now(),
            delay: Duration::from_millis(2500),
            requested: false,
            select: None,
            dialog: None,
            act: None,
            drawer_scroll: None,
            tab: None,
            palette: None,
            main_scroll: None,
        });
        let mut app = DevDuck {
            demo,
            shot,
            ctx: cc.egui_ctx.clone(),
            recent_slots: std::cell::Cell::new(4),
            logo: cc.egui_ctx.load_texture(
                "devduck-logo",
                egui::ColorImage::from_rgba_unmultiplied([96, 96], icon::LOGO_96),
                egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR },
            ),
            state,
            state_dirty_since: None,
            rows: Vec::new(),
            live: HashMap::new(),
            probes: HashMap::new(),
            scanned: None,
            scan_clock: String::new(),
            scan_ms: 0.0,
            search: String::new(),
            chip: Chip::Projects,
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
            projects: Vec::new(),
            git: HashMap::new(),
            git_at: None,
            git_inflight: false,
            git_one: None,
            icons: HashMap::new(),
            projects_loading: true,
            children: HashMap::new(),
            launching: HashMap::new(),
            exits: HashMap::new(),
            expected_exit: HashSet::new(),
            last_lines: HashMap::new(),
            tab: Tab::Overview,
            palette: None,
            root_input: String::new(),
            stack_queue: Vec::new(),
            tx,
            rx,
            worker,
        };
        if demo {
            app.exits.insert(r"c:\users\dev\ml-notebooks".into(), Exit { code: Some(1), at: store::now_unix() - 240 });
        }
        app.rescan_projects();
        if app.shot.is_some() {
            app.apply_demo_ui(&std::env::var("DEVDUCK_UI").unwrap_or_default());
        }
        app
    }

    fn rescan_projects(&mut self) {
        self.projects_loading = true;
        let (tx, ctx, demo, roots) = (self.tx.clone(), self.ctx.clone(), self.demo, self.state.roots());
        std::thread::spawn(move || {
            let found = if demo { demo_projects() } else { projects::discover(&roots) };
            let _ = tx.send(Msg::Projects(found));
            ctx.request_repaint();
        });
    }

    /// Re-read git for one folder, or for every project (every few minutes).
    fn refresh_git(&mut self, only: Option<String>) {
        let all = only.is_none();
        if all && self.git_inflight {
            return;
        }
        let folders: Vec<String> = match only {
            Some(f) => vec![f],
            None => {
                let mut v: Vec<String> = self.projects.iter().map(|p| p.folder.clone()).collect();
                for r in self.rows.iter().filter(|r| r.kind == Kind::Dev && !r.folder.is_empty()) {
                    if !v.iter().any(|f| f.eq_ignore_ascii_case(&r.folder)) {
                        v.push(r.folder.clone());
                    }
                }
                v
            }
        };
        if all {
            self.git_inflight = true;
        }
        let (tx, ctx, demo) = (self.tx.clone(), self.ctx.clone(), self.demo);
        std::thread::spawn(move || {
            let found = if demo { demo_git(&folders) } else { git_all(folders) };
            let _ = tx.send(Msg::Git(found, all));
            ctx.request_repaint();
        });
    }

    /// Find and decode each project's icon off the UI thread.
    fn load_icons(&mut self) {
        if self.demo {
            return;
        }
        let folders: Vec<String> = self.projects.iter().map(|p| p.folder.clone()).filter(|f| !self.icons.contains_key(&f.to_lowercase())).collect();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let found = folders
                .into_iter()
                .filter_map(|f| {
                    let path = projects::find_icon(std::path::Path::new(&f))?;
                    let img = image::open(&path).ok()?.thumbnail(64, 64).to_rgba8();
                    let size = [img.width() as usize, img.height() as usize];
                    Some((f.to_lowercase(), egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw())))
                })
                .collect();
            let _ = tx.send(Msg::Icons(found));
            ctx.request_repaint();
        });
    }

    fn apply_demo_ui(&mut self, spec: &str) {
        for part in spec.split(';').filter(|p| !p.is_empty()) {
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            match k {
                "view" => self.state.settings.view = if v == "list" { ViewMode::List } else { ViewMode::Cards },
                "chip" => {
                    self.chip = match v {
                        "all" => Chip::All,
                        "stack" => Chip::Stack(0),
                        "docker" => Chip::Docker,
                        "apps" => Chip::Apps,
                        "system" => Chip::System,
                        _ => Chip::Projects,
                    }
                }
                "tab" => self.shot.as_mut().unwrap().tab = Some(if v == "output" { Tab::Output } else if v == "details" { Tab::Details } else { Tab::Overview }),
                "palette" => self.shot.as_mut().unwrap().palette = Some(v.to_string()),
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
                "main_scroll" => self.shot.as_mut().unwrap().main_scroll = v.parse().ok(),
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
            && !self.projects_loading
            && self.scanned.is_some()
            && let Some(id) = self.items().into_iter().find(|i| i.name == app).map(|i| i.id)
        {
            let hook = self.shot.as_mut().unwrap();
            let tab = hook.tab.take();
            hook.select = None;
            self.selected = Some(id);
            self.drawer = true;
            self.tab = tab.unwrap_or_default();
            if self.shot.as_ref().unwrap().dialog.as_deref() == Some("stop")
                && let Some(key) = self.selected_row().map(|r| r.key.clone())
            {
                self.dialog = Some(Dialog::ConfirmStop(key));
                self.shot.as_mut().unwrap().dialog = None;
            }
        }
        let hook = self.shot.as_mut().unwrap();
        if let Some(q) = hook.palette.take() {
            self.palette = Some(Palette { query: q, selected: 0, focused: false });
        }
        let hook = self.shot.as_mut().unwrap();
        if hook.dialog.as_deref() == Some("stack") {
            hook.dialog = None;
            self.act(Action::EditStack(0));
        } else if let Some(app) = hook.dialog.as_deref().and_then(|d| d.strip_prefix("conflict:")).map(str::to_string)
            && !self.projects_loading
            && self.scanned.is_some()
            && let Some(folder) = self.items().into_iter().find(|i| i.name == app).map(|i| i.folder)
        {
            self.shot.as_mut().unwrap().dialog = None;
            self.act(Action::Run(folder, None));
        }
        let hook = self.shot.as_mut().unwrap();
        if let Some((what, app)) = hook.act.clone() {
            let row_key = self.rows.iter().find(|r| r.app == app).map(|r| r.key.clone());
            let folder = self.state.recent.iter().find(|e| e.app == app).map(|e| e.folder.clone());
            let fav_id = self.state.favorites.iter().find(|f| f.name == app).map(Favorite::id);
            let item_folder = if self.projects_loading || self.scanned.is_none() { None } else { self.items().into_iter().find(|i| i.name == app).map(|i| i.folder) };
            let action = match (what.as_str(), row_key, folder) {
                ("run", _, _) => item_folder.map(|f| Action::Run(f, None)),
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
        } else if !hook.requested && self.scanned.is_some() && !self.projects_loading && hook.started.elapsed() > hook.delay {
            hook.requested = true;
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
    }

    // ------------------------------------------------------------------ data --

    fn row(&self, key: &str) -> Option<&PortApp> {
        self.rows.iter().find(|r| r.key == key)
    }

    /// The running row behind the selection, if it's running.
    pub(crate) fn selected_row(&self) -> Option<&PortApp> {
        let id = self.selected.as_deref()?;
        self.rows.iter().find(|r| row_id(r) == id || r.key == id)
    }

    pub(crate) fn project(&self, folder: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.folder.eq_ignore_ascii_case(folder))
    }

    pub(crate) fn recent(&self, folder: &str) -> Option<&Recent> {
        self.state.recent.iter().find(|e| e.folder.eq_ignore_ascii_case(folder))
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

    /// Whether a favorite is being started.
    pub(crate) fn favorite_busy(&self, f: &Favorite) -> bool {
        self.busy.contains_key(&format!("fav:{}", f.id()))
            || (f.kind == FavoriteKind::Project && (self.busy.contains_key(&start_key(&f.target)) || self.launching.contains_key(&f.target.to_lowercase())))
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

    /// Where `/` redirects to, when it does.
    pub(crate) fn redirect(&self, r: &PortApp) -> Option<&str> {
        match self.probe_of(&r.key, self.main_port(r))?.result.as_ref()? {
            Probe::Http { status: 300..=399, location, .. } => Some(location.as_deref().unwrap_or("another page")),
            _ => None,
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
        let ours = self.children.contains_key(&r.folder.to_lowercase()) || r.kill_pid == pid || r.tree.iter().any(|t| t.pid == pid);
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
        let appeared: HashSet<String> = rows.iter().filter(|r| !self.live.contains_key(&r.key)).map(|r| r.key.clone()).collect();
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

        for r in rows.iter().filter(|r| r.kind == Kind::Dev && !r.folder.is_empty()) {
            let lc = r.folder.to_lowercase();
            self.launching.remove(&lc);
            if appeared.contains(&r.key) {
                self.exits.remove(&lc);
            }
        }
        if let Some(folder) = &self.reselect_folder
            && rows.iter().any(|r| r.folder.eq_ignore_ascii_case(folder))
        {
            self.selected = Some(project_id(folder));
            self.reselect_folder = None;
        }
        // A listener that went away closes its page; a project or container keeps it
        // (now showing Start).
        if let Some(sel) = &self.selected
            && !sel.starts_with("p:")
            && !sel.starts_with("c:")
            && !rows.iter().any(|r| &r.key == sel)
        {
            self.selected = None;
            self.drawer = false;
        }
        self.busy.retain(|k, _| k.starts_with("fav:") || k.starts_with("start:") || k.starts_with("stop:") || rows.iter().any(|r| &r.key == k));
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
                            let framework = self.project(&l.folder).and_then(|p| p.framework.clone());
                            self.state.recent.push(Recent { folder: l.folder.clone(), app, framework, ..Default::default() });
                        }
                        if let Some(e) = self.state.recent_mut(&l.folder) {
                            e.launched_pid = Some(l.pid);
                            e.log = Some(l.log);
                            e.last_seen = store::now_unix();
                            if l.script.is_some() {
                                e.script = l.script.clone();
                            }
                        }
                        let lc = l.folder.to_lowercase();
                        if let Some(child) = l.child {
                            self.children.insert(lc.clone(), child);
                        }
                        self.expected_exit.remove(&lc);
                        self.exits.remove(&lc);
                        self.launching.insert(lc, Instant::now());
                        if l.focus {
                            self.selected = Some(project_id(&l.folder));
                            self.drawer = true;
                            self.tab = Tab::Output;
                        }
                        self.save_now();
                    }
                    self.scan_soon();
                }
                Msg::Projects(found) => {
                    self.projects = found;
                    self.projects_loading = false;
                    self.load_icons();
                    self.refresh_git(None);
                }
                Msg::Git(found, all) => {
                    if all {
                        self.git_inflight = false;
                        self.git_at = Some(Instant::now());
                    }
                    self.git.extend(found);
                }
                Msg::Icons(found) => {
                    for (k, img) in found {
                        let tex = self.ctx.load_texture(format!("icon:{k}"), img, egui::TextureOptions::LINEAR);
                        self.icons.insert(k, tex);
                    }
                }
                Msg::DockerLogs(key, text) => {
                    self.logs_inflight = false;
                    self.logs = Some(LogView { key, text: launch::clean_log(&text), fetched: Instant::now(), source: "docker logs", path: None, ours: true });
                }
            }
        }
    }

    /// Notice when something DevDuck started exits: a crash shows on its card and page.
    fn poll_children(&mut self) {
        let mut done = Vec::new();
        for (lc, child) in &mut self.children {
            if let Ok(Some(status)) = child.try_wait() {
                done.push((lc.clone(), status.code()));
            }
        }
        for (lc, code) in done {
            self.children.remove(&lc);
            self.launching.remove(&lc);
            if self.expected_exit.remove(&lc) {
                continue;
            }
            let name = self.display_name(&lc);
            self.exits.insert(lc, Exit { code, at: store::now_unix() });
            match code {
                Some(0) => self.toast(format!("{name} finished"), true),
                Some(c) => self.toast(format!("{name} exited with code {c}"), false),
                None => self.toast(format!("{name} stopped"), false),
            }
            self.scan_now();
        }
        // A stack start runs its projects once its containers are up (or gave up).
        if !self.stack_queue.is_empty() && !self.busy.keys().any(|k| k.starts_with("fav:container:")) {
            for folder in std::mem::take(&mut self.stack_queue) {
                self.run_project(&folder, None, false, None, false);
            }
        }
    }

    /// The last line each project DevDuck started has printed, for its card: every
    /// ~0.9 s while it starts, every 2 s once it's up.
    fn refresh_last_lines(&mut self) {
        let mut wanted: Vec<(String, Option<String>, bool)> = self.launching.keys().map(|lc| (lc.clone(), self.recent(lc).and_then(|e| e.log.clone()), true)).collect();
        for r in self.rows.iter().filter(|r| r.kind == Kind::Dev && !r.folder.is_empty()) {
            let log = self.launched_log(r);
            if log.is_some() || self.demo {
                wanted.push((r.folder.to_lowercase(), log, false));
            }
        }
        for (lc, log, starting) in &wanted {
            let every = if *starting { 900 } else { 2000 };
            if self.last_lines.get(lc).is_some_and(|(t, _)| t.elapsed() < Duration::from_millis(every)) {
                continue;
            }
            let line = if self.demo {
                if *starting { "\u{25CB} Compiling / ...".to_string() } else { format!("GET / 200 in {}ms", 12 + lc.len() * 3) }
            } else {
                log.as_ref()
                    .and_then(|p| launch::tail(std::path::Path::new(p), 4096))
                    .and_then(|t| t.lines().rev().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("[devduck]")).map(str::to_string))
                    .unwrap_or_default()
            };
            self.last_lines.insert(lc.clone(), (Instant::now(), line));
        }
        self.last_lines.retain(|k, _| wanted.iter().any(|w| &w.0 == k));
    }

    /// Keep the open page's output fresh: the project's log (DevDuck's own runs), or
    /// `docker logs` for a container.
    fn refresh_logs(&mut self) {
        self.refresh_last_lines();
        if !self.git_inflight && self.git_at.is_some_and(|t| t.elapsed() > Duration::from_secs(180)) {
            self.refresh_git(None);
        }
        // The open page's project, more often: it's the one being worked on.
        if let Some(folder) = self.selected.as_deref().filter(|_| self.drawer).and_then(|id| self.item(id)).filter(|it| it.is_project()).map(|it| it.folder)
            && self.git_one.as_ref().is_none_or(|(f, t)| *f != folder || t.elapsed() > Duration::from_secs(5))
            && self.git_at.is_some()
        {
            self.git_one = Some((folder.clone(), Instant::now()));
            self.refresh_git(Some(folder));
        }
        let Some(id) = self.selected.clone().filter(|_| self.drawer) else {
            self.logs = None;
            return;
        };
        let every = if self.tab == Tab::Output { 700 } else { 1500 };
        let due = self.logs.as_ref().is_none_or(|l| l.key != id || l.fetched.elapsed() > Duration::from_millis(every));
        if !due {
            return;
        }
        let row = self.selected_row().cloned();
        let container = id.strip_prefix("c:").map(str::to_string).or_else(|| row.as_ref().filter(|r| r.kind == Kind::Docker).map(|r| r.docker_name.clone()));
        let now = Instant::now();
        if let Some(name) = container {
            if self.demo {
                if self.logs.as_ref().is_none_or(|l| l.key != id) {
                    self.logs = Some(LogView { key: id, text: DEMO_DOCKER_LOG.into(), fetched: now, source: "docker logs", path: None, ours: true });
                }
                return;
            }
            if self.logs_inflight {
                return;
            }
            self.logs_inflight = true;
            let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
            std::thread::spawn(move || {
                let text = launch::docker(&["logs", "--tail", "300", &name]).unwrap_or_else(|e| e);
                let _ = tx.send(Msg::DockerLogs(id, text));
                ctx.request_repaint();
            });
            return;
        }
        let Some(folder) = id.strip_prefix("p:").map(str::to_string) else {
            self.logs = None;
            return;
        };
        let starting = self.launching.contains_key(&folder);
        if self.demo {
            let text = if self.exits.contains_key(&folder) {
                DEMO_CRASH_LOG
            } else if row.is_some() || starting {
                DEMO_DEV_LOG
            } else {
                ""
            };
            self.logs = Some(LogView { key: id, text: text.into(), fetched: now, source: "started by DevDuck", path: None, ours: true });
            return;
        }
        let ours = row.as_ref().is_none_or(|r| self.launched_log(r).is_some());
        let path = self.recent(&folder).and_then(|e| e.log.clone());
        let source = if starting || (row.is_some() && ours) { "started by DevDuck" } else { "last run" };
        let text = path.as_ref().and_then(|p| launch::tail(std::path::Path::new(p), 96 * 1024)).unwrap_or_default();
        self.logs = Some(LogView { key: id, text, fetched: now, source, path, ours });
    }

    // --------------------------------------------------------------- actions --

    fn act(&mut self, action: Action) {
        match action {
            Action::Select(id) => {
                // A listener's row key selects its project or container.
                let id = self.row(&id).map(row_id).unwrap_or(id);
                if self.selected.as_deref() == Some(id.as_str()) && self.drawer {
                    return;
                }
                self.selected = Some(id);
                self.drawer = true;
                self.logs = None;
                self.tab = Tab::Overview;
                // Re-check a server's HTTP status when you look at it.
                if let Some(r) = self.selected_row().cloned()
                    && r.kind == Kind::Dev
                    && self.state.settings.probe_http
                    && !self.demo
                {
                    for p in r.ports.iter().filter(|p| !p.is_ephemeral()) {
                        let fresh = self.probe_of(&r.key, p.port).is_some_and(|e| e.result.is_none() || e.at.elapsed() < Duration::from_secs(15));
                        if !fresh {
                            self.spawn_probe(&r.key, p.port, p.addrs.clone());
                        }
                    }
                }
            }
            Action::Show(id, tab) => {
                self.act(Action::Select(id));
                self.tab = tab;
            }
            Action::SetTab(tab) => self.tab = tab,
            Action::Run(folder, script) => self.run_project(&folder, script, false, None, true),
            Action::RunForce(folder, script, stop) => self.run_project(&folder, script, true, stop, true),
            Action::RunOnPort(folder, script, port) => self.run_on_port(&folder, script, port),
            Action::StopItem(id) => self.stop_item(&id),
            Action::StartContainer(name) => self.start_container(&name),
            Action::DismissExit(folder) => {
                self.exits.remove(&folder.to_lowercase());
            }
            Action::OpenPalette => self.palette = Some(Palette::default()),
            Action::StartStack(i) => self.start_stack(i),
            Action::StopStack(i) => self.stop_stack(i),
            Action::NewStack => {
                let members = self
                    .rows
                    .iter()
                    .filter_map(|r| match r.kind {
                        Kind::Dev if !r.folder.is_empty() => Some(Member { kind: FavoriteKind::Project, target: r.folder.clone() }),
                        Kind::Docker => Some(Member { kind: FavoriteKind::Container, target: r.docker_name.clone() }),
                        _ => None,
                    })
                    .collect();
                self.dialog = Some(Dialog::Stack(StackDraft { index: None, name: String::new(), members, filter: String::new() }));
            }
            Action::EditStack(i) => {
                if let Some(st) = self.state.stacks.get(i) {
                    self.dialog = Some(Dialog::Stack(StackDraft { index: Some(i), name: st.name.clone(), members: st.members.clone(), filter: String::new() }));
                }
            }
            Action::DeleteStack(i) => {
                if i < self.state.stacks.len() {
                    let name = self.state.stacks.remove(i).name;
                    self.chip = match self.chip {
                        Chip::Stack(j) if j == i => Chip::Projects,
                        Chip::Stack(j) if j > i => Chip::Stack(j - 1),
                        c => c,
                    };
                    self.toast(format!("Deleted stack {name}"), true);
                    self.save_now();
                }
            }
            Action::SaveStack(draft) => {
                let name = if draft.name.trim().is_empty() { format!("Stack {}", self.state.stacks.len() + 1) } else { draft.name.trim().to_string() };
                let stack = Stack { name: name.clone(), members: draft.members };
                match draft.index.filter(|i| *i < self.state.stacks.len()) {
                    Some(i) => self.state.stacks[i] = stack,
                    None => {
                        self.state.stacks.push(stack);
                        self.chip = Chip::Stack(self.state.stacks.len() - 1);
                        self.toast(format!("Created stack {name}"), true);
                    }
                }
                self.save_now();
            }
            Action::ToggleInStack(i, m) => {
                if let Some(st) = self.state.stacks.get_mut(i) {
                    if st.members.iter().any(|x| x.is(m.kind, &m.target)) {
                        st.members.retain(|x| !x.is(m.kind, &m.target));
                    } else {
                        st.members.push(m);
                    }
                    self.save_now();
                }
            }
            Action::StopAllDev => {
                let keys: Vec<String> = self.rows.iter().filter(|r| r.kind == Kind::Dev && r.kill_pid > 4).map(|r| r.key.clone()).collect();
                for k in keys {
                    self.stop(&k);
                }
            }
            Action::RescanProjects => self.rescan_projects(),
            Action::AddRoot(path) => {
                let path = path.trim().trim_matches('"').trim_end_matches(['\\', '/']).to_string();
                if !std::path::Path::new(&path).is_dir() {
                    self.toast("That folder doesn't exist", false);
                    return;
                }
                if self.state.settings.project_roots.is_empty() {
                    self.state.settings.project_roots = projects::default_roots();
                }
                if !self.state.settings.project_roots.iter().any(|r| r.eq_ignore_ascii_case(&path)) {
                    self.state.settings.project_roots.push(path);
                }
                self.root_input.clear();
                self.save_now();
                self.rescan_projects();
            }
            Action::RemoveRoot(path) => {
                if self.state.settings.project_roots.is_empty() {
                    self.state.settings.project_roots = projects::default_roots();
                }
                self.state.settings.project_roots.retain(|r| !r.eq_ignore_ascii_case(&path));
                self.save_now();
                self.rescan_projects();
            }
            Action::CloseDrawer => self.drawer = false,
            Action::OpenWeb(url) => {
                if !win::shell_open(&url) {
                    self.toast(format!("Couldn't open {url}"), false);
                }
            }
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
                if let Some(key) = self.selected_row().map(|r| r.key.clone()) {
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
                } else {
                    let e = self.recent(&folder).cloned().unwrap_or_default();
                    let name = self.display_name(&folder);
                    let framework = e.framework.clone().or_else(|| self.project(&folder).and_then(|p| p.framework.clone()));
                    self.state.favorites.push(Favorite {
                        kind: FavoriteKind::Project,
                        target: folder.clone(),
                        name: name.clone(),
                        framework,
                        ports: e.ports.clone(),
                        command: e.command.clone(),
                        cwd: e.cwd.clone(),
                    });
                    self.toast(format!("Added {name} to favorites"), true);
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
        if !r.folder.is_empty() {
            self.expected_exit.insert(r.folder.to_lowercase());
        }
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
        // A server DevDuck started with a script restarts with that script.
        let script = self.recent(&r.folder).and_then(|e| e.script.clone()).filter(|_| self.launched_log(&r).is_some());
        let spec = match script.and_then(|s| self.run_spec(&r.folder, Some(&s)).ok()) {
            // Keep it on the port it's on now if that isn't its framework's usual one.
            Some(spec) => {
                let port = self.main_port(&r);
                let usual = self.project(&r.folder).and_then(|p| p.framework.as_deref()).and_then(crate::projects::framework_port);
                if usual.is_some_and(|u| u != port) { self.on_port(&r.folder, spec, port) } else { spec }
            }
            None => match self.relaunch_spec(&r) {
                Some((command, cwd)) => RunSpec { command, cwd, shell: false, script: None, port: None },
                None => {
                    self.dialog = Some(Dialog::Error(
                        format!("Can't restart {}", r.app),
                        "DevDuck couldn't read this server's command line (it may be running as another user), so it doesn't know how to start it again.".into(),
                    ));
                    return;
                }
            },
        };
        self.busy.insert(key_s.clone(), "Restarting");
        if !r.folder.is_empty() {
            self.expected_exit.insert(r.folder.to_lowercase());
        }
        let ports = r.port_numbers();
        let folder = r.folder.clone();
        std::thread::spawn(move || {
            let msg = match launch::kill_tree(r.kill_pid) {
                Err(e) => Msg::Done { key: key_s, toast: None, error: Some((format!("Couldn't stop {}", r.app), e)), launched: None },
                Ok(()) => {
                    launch::wait_ports_free(&ports, Duration::from_secs(6));
                    match launch::start(&r.app, &spec.command, &spec.cwd, spec.shell, spec.port) {
                        Ok((child, log)) => Msg::Done {
                            key: key_s,
                            toast: Some(format!("Restarted {}", r.app)),
                            error: None,
                            launched: Some(Launched { folder, pid: child.id(), log: log.to_string_lossy().into_owned(), child: Some(child), script: spec.script, focus: false }),
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
        self.run_project(folder, None, false, None, true);
    }

    fn start_favorite(&mut self, id: &str) {
        let Some(f) = self.state.favorites.iter().find(|f| f.id() == id).cloned() else { return };
        if self.running_row(&f).is_some() {
            return;
        }
        match f.kind {
            FavoriteKind::Project => self.run_project(&f.target, None, false, None, false),
            FavoriteKind::Container => self.start_container(&f.target),
        }
    }

    /// Start a project: check it isn't running and that its port is free, then run it
    /// with its output captured. `focus` opens its page on the Output tab.
    fn run_project(&mut self, folder: &str, script: Option<String>, force: bool, stop_first: Option<String>, focus: bool) {
        let lc = folder.to_lowercase();
        if self.rows.iter().any(|r| r.kind == Kind::Dev && r.folder.eq_ignore_ascii_case(folder)) || self.launching.contains_key(&lc) || self.busy.contains_key(&start_key(folder)) {
            if focus {
                self.act(Action::Show(project_id(folder), Tab::Output));
            }
            return;
        }
        let name = self.display_name(folder);
        let spec = match self.run_spec(folder, script.as_deref()) {
            Ok(spec) => spec,
            Err(e) => {
                self.dialog = Some(Dialog::Error(format!("Can't start {name}"), e));
                return;
            }
        };
        if !force && let Some((port, holder)) = self.conflict(folder) {
            let holder = holder.key.clone();
            let free = self.next_free_port(port);
            self.dialog = Some(Dialog::PortConflict { folder: folder.to_string(), script, port, holder, free });
            return;
        }
        let pre_stop = stop_first.and_then(|k| self.row(&k).cloned());
        if let Some(r) = &pre_stop {
            if !r.folder.is_empty() {
                self.expected_exit.insert(r.folder.to_lowercase());
            }
            self.busy.insert(r.key.clone(), "Stopping");
        }
        self.launch_project(start_key(folder), &name, spec, folder, pre_stop, focus);
    }

    fn run_on_port(&mut self, folder: &str, script: Option<String>, port: u16) {
        let name = self.display_name(folder);
        match self.run_spec(folder, script.as_deref()) {
            Ok(spec) => {
                let spec = self.on_port(folder, spec, port);
                self.launch_project(start_key(folder), &name, spec, folder, None, true);
            }
            Err(e) => self.dialog = Some(Dialog::Error(format!("Can't start {name}"), e)),
        }
    }

    /// Stop an item: a running row (asks first if set to), or a project still starting.
    fn stop_item(&mut self, id: &str) {
        if let Some(key) = self.rows.iter().find(|r| row_id(r) == id || r.key == id).map(|r| r.key.clone()) {
            self.act(Action::AskStop(key));
            return;
        }
        let Some(lc) = id.strip_prefix("p:").map(str::to_string) else { return };
        self.launching.remove(&lc);
        if self.demo {
            return;
        }
        if let Some(pid) = self.children.get(&lc).map(std::process::Child::id) {
            self.expected_exit.insert(lc.clone());
            let name = self.display_name(&lc);
            let key = format!("stop:{lc}");
            self.busy.insert(key.clone(), "Stopping");
            let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
            std::thread::spawn(move || {
                let msg = match launch::kill_tree(pid) {
                    Ok(()) => Msg::Done { key, toast: Some(format!("Stopped {name}")), error: None, launched: None },
                    Err(e) => Msg::Done { key, toast: None, error: Some((format!("Couldn't stop {name}"), e)), launched: None },
                };
                let _ = tx.send(msg);
                ctx.request_repaint();
            });
        }
    }

    fn start_container(&mut self, name: &str) {
        if self.rows.iter().any(|r| r.kind == Kind::Docker && r.docker_name == name) {
            return;
        }
        if self.demo {
            self.toast(format!("Demo mode: {name} was not started"), false);
            return;
        }
        let busy_key = format!("fav:container:{name}");
        if self.busy.contains_key(&busy_key) {
            return;
        }
        self.busy.insert(busy_key.clone(), "Starting");
        let (tx, ctx, name) = (self.tx.clone(), self.ctx.clone(), name.to_string());
        std::thread::spawn(move || {
            let msg = match launch::docker(&["start", &name]) {
                Ok(_) => Msg::Done { key: busy_key, toast: Some(format!("Started {name}")), error: None, launched: None },
                Err(e) => {
                    let body = if e.contains("error during connect") || e.contains("Couldn't run docker") {
                        "Docker isn't running. Start Docker Desktop, then try again.".to_string()
                    } else {
                        e
                    };
                    Msg::Done { key: busy_key, toast: None, error: Some((format!("Couldn't start {name}"), body)), launched: None }
                }
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// Containers first; projects once the containers are up.
    fn start_stack(&mut self, i: usize) {
        let Some(stack) = self.state.stacks.get(i).cloned() else { return };
        for m in stack.members.iter().filter(|m| m.kind == FavoriteKind::Container) {
            self.start_container(&m.target);
        }
        for m in stack.members.iter().filter(|m| m.kind == FavoriteKind::Project) {
            if !self.stack_queue.iter().any(|f| f.eq_ignore_ascii_case(&m.target)) {
                self.stack_queue.push(m.target.clone());
            }
        }
        self.toast(format!("Starting {}", stack.name), true);
    }

    /// Projects first; containers after.
    fn stop_stack(&mut self, i: usize) {
        let Some(stack) = self.state.stacks.get(i).cloned() else { return };
        self.stack_queue.retain(|f| !stack.members.iter().any(|m| m.is(FavoriteKind::Project, f)));
        for m in &stack.members {
            let id = match m.kind {
                FavoriteKind::Project => project_id(&m.target),
                FavoriteKind::Container => container_id(&m.target),
            };
            if let Some(key) = self.rows.iter().find(|r| row_id(r) == id).map(|r| r.key.clone()) {
                self.stop(&key);
            } else if m.kind == FavoriteKind::Project {
                self.stop_item(&id);
            }
        }
    }

    /// Run a command in a project folder with its output captured to a log, stopping
    /// `pre_stop` (whatever holds the port) first.
    fn launch_project(&mut self, busy_key: String, app: &str, spec: RunSpec, folder: &str, pre_stop: Option<PortApp>, focus: bool) {
        if self.demo {
            let lc = folder.to_lowercase();
            self.launching.insert(lc.clone(), Instant::now());
            self.exits.remove(&lc);
            if focus {
                self.selected = Some(project_id(folder));
                self.drawer = true;
                self.tab = Tab::Output;
            }
            return;
        }
        self.busy.insert(busy_key.clone(), "Starting");
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let (app, folder) = (app.to_string(), folder.to_string());
        std::thread::spawn(move || {
            if let Some(r) = pre_stop {
                let _ = if r.kind == Kind::Docker { launch::docker(&["stop", "-t", "2", &r.docker_name]).map(drop) } else { launch::kill_tree(r.kill_pid) };
                launch::wait_ports_free(&r.port_numbers(), Duration::from_secs(6));
            }
            let msg = match launch::start(&app, &spec.command, &spec.cwd, spec.shell, spec.port) {
                Ok((child, log)) => Msg::Done {
                    key: busy_key,
                    toast: Some(format!("Starting {app}")),
                    error: None,
                    launched: Some(Launched { folder, pid: child.id(), log: log.to_string_lossy().into_owned(), child: Some(child), script: spec.script, focus }),
                },
                Err(err) => Msg::Done { key: busy_key, toast: None, error: Some((format!("Couldn't start {app}"), err)), launched: None },
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }
}

impl eframe::App for DevDuck {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if !self.titlebar_styled {
            self.titlebar_styled = win::style_title_bar(theme::rgb(theme::BG), theme::rgb(theme::TEXT_2), theme::rgb(theme::BORDER));
        }
        self.poll();
        self.poll_children();
        self.refresh_logs();
        self.run_shot_hook();

        let mut actions = Vec::new();
        if self.dialog.is_none() && self.palette.is_none() {
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
            || !self.busy.is_empty()
            || !self.launching.is_empty();
        if animating {
            self.ctx.request_repaint_after(Duration::from_millis(33));
        }
        if self.drawer {
            self.ctx.request_repaint_after(Duration::from_millis(if self.tab == Tab::Output { 700 } else { 1600 }));
        }
        if !self.children.is_empty() {
            self.ctx.request_repaint_after(Duration::from_millis(1000));
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
    let stacks = vec![Stack {
        name: "Acme".into(),
        members: vec![
            Member { kind: FavoriteKind::Project, target: r"C:\Users\dev\acme-storefront".into() },
            Member { kind: FavoriteKind::Container, target: "postgres-16".into() },
            Member { kind: FavoriteKind::Container, target: "redis".into() },
        ],
    }];
    State { recent, favorites, stacks, ..Default::default() }
}

/// git for many folders, eight at a time.
fn git_all(folders: Vec<String>) -> Vec<(String, Option<crate::git::GitInfo>)> {
    let per = folders.len().div_ceil(8).max(1);
    let chunks: Vec<Vec<String>> = folders.chunks(per).map(<[String]>::to_vec).collect();
    std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|c| s.spawn(move || c.into_iter().map(|f| (f.to_lowercase(), crate::git::read(std::path::Path::new(&f)))).collect::<Vec<_>>()))
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    })
}

fn demo_git(folders: &[String]) -> Vec<(String, Option<crate::git::GitInfo>)> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    folders
        .iter()
        .map(|f| {
            let name = f.rsplit(['\\', '/']).next().unwrap_or_default().to_string();
            let (branch, changed, ahead, subject, ago) = match name.as_str() {
                "acme-storefront" => ("main", 3, 1, "Spring collection hero and copy", 2 * 3600),
                "docs-site" => ("docs/new-nav", 0, 2, "Sidebar groups for the API pages", 3 * 3600),
                "ml-notebooks" => ("main", 1, 0, "Pin jupyterlab 4.2", 26 * 3600),
                "marketing-site" => ("launch-week", 5, 0, "Countdown banner", 40 * 60),
                _ => ("main", 0, 0, "Initial commit", 6 * 86400),
            };
            let info = crate::git::GitInfo {
                branch: branch.into(),
                detached: false,
                changed,
                ahead: Some(ahead),
                behind: Some(0),
                last_subject: subject.into(),
                last_time: now - ago,
                web: Some(format!("https://github.com/acme/{name}")),
            };
            (f.to_lowercase(), Some(info))
        })
        .collect()
}

fn demo_projects() -> Vec<Project> {
    use crate::projects::Script;
    let s = |name: &str, command: &str, detail: &str| Script { name: name.into(), command: command.into(), detail: detail.into() };
    let p = |name: &str, fw: &str, scripts: Vec<Script>| Project { folder: format!(r"C:\Users\dev\{name}"), name: name.into(), framework: Some(fw.into()), scripts };
    let mut v = vec![
        p("acme-storefront", "Next.js", vec![s("dev", "pnpm run dev", "next dev --turbopack"), s("start", "pnpm run start", "next start"), s("build", "pnpm run build", "next build"), s("lint", "pnpm run lint", "eslint .")]),
        p("marketing-site", "Vite", vec![s("dev", "npm run dev", "vite"), s("preview", "npm run preview", "vite preview"), s("build", "npm run build", "vite build")]),
        p("docs-site", "Astro", vec![s("dev", "npm run dev", "astro dev"), s("preview", "npm run preview", "astro preview"), s("build", "npm run build", "astro build")]),
        p("launch-film", "Remotion", vec![s("dev", "npm run dev", "remotion studio"), s("render", "npm run render", "remotion render")]),
        p("design-system", "Storybook", vec![s("storybook", "pnpm run storybook", "storybook dev -p 6006"), s("build", "pnpm run build", "tsup")]),
        p("billing-dashboard", "React Router", vec![s("dev", "npm run dev", "react-router dev"), s("typecheck", "npm run typecheck", "tsc")]),
        p("field-app", "Expo", vec![s("start", "npx expo start", "expo start"), s("android", "npm run android", "expo run:android")]),
        p("ml-notebooks", "Jupyter", vec![s("dev", "npm run dev", "jupyter lab")]),
    ];
    v.push(Project {
        folder: r"C:\Users\dev\internal-api".into(),
        name: "internal-api".into(),
        framework: Some("Uvicorn".into()),
        scripts: vec![s("dev", "uv run dev", "uvicorn app.main:app --reload --port 4000")],
    });
    v
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
        ("internal-api", _) => Probe::Http { status: 404, title: None, millis: 4, html: false, location: None },
        _ => Probe::Http { status: 200, title: title.map(str::to_string), millis: 18 + (port % 40) as u32, html: true, location: None },
    }
}

const DEMO_DEV_LOG: &str = "[devduck] 14:02:11 in C:\\Users\\dev\\acme-storefront\n[devduck] npm run dev\n\n> acme-storefront@0.4.0 dev\n> next dev --turbopack\n\n   \u{25B2} Next.js 16.0.1 (Turbopack)\n   - Local:        http://localhost:3000\n   - Network:      http://192.168.1.24:3000\n\n \u{2713} Starting...\n \u{2713} Ready in 1180ms\n \u{25CB} Compiling / ...\n GET / 200 in 2.1s\n GET /products/linen-shirt 200 in 412ms\n GET /api/cart 200 in 38ms\n";
const DEMO_CRASH_LOG: &str = "[devduck] 13:58:40 in C:\\Users\\dev\\ml-notebooks\n[devduck] npm run dev\n\n> ml-notebooks@0.1.0 dev\n> jupyter lab --port 8888\n\n[I 13:58:42 ServerApp] jupyterlab | extension was successfully linked.\nTraceback (most recent call last):\n  File \"C:\\Users\\dev\\ml-notebooks\\.venv\\Lib\\site-packages\\jupyter_server\\serverapp.py\", line 2731, in start\n    self.http_server.listen(port, self.ip)\nOSError: [WinError 10048] Only one usage of each socket address is normally permitted\n[C 13:58:43 ServerApp] ERROR | Port 8888 is in use\n";
const DEMO_DOCKER_LOG: &str = "PostgreSQL init process complete; ready for start up.\n\n2026-10-04 09:12:01.233 UTC [1] LOG:  starting PostgreSQL 16.4 on x86_64-pc-linux-gnu\n2026-10-04 09:12:01.234 UTC [1] LOG:  listening on IPv4 address \"0.0.0.0\", port 5432\n2026-10-04 09:12:01.240 UTC [1] LOG:  database system is ready to accept connections\n";
