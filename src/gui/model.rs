//! The things you control: every project (found on disk, seen running, remembered or
//! starred) and every container, merged into one `Item` with a run state. Plus what
//! Start would run, port conflicts, stack status and the command palette's commands.

use std::collections::HashMap;

use super::icons::Icon;
use super::{Action, DevDuck, Health, Tab, UNAVAILABLE};
use crate::projects::{self, Project, Script};
use crate::scan::{Kind, PortApp};
use crate::store::{Favorite, FavoriteKind, Member, Recent};

pub(crate) fn project_id(folder: &str) -> String {
    format!("p:{}", folder.to_lowercase())
}

pub(crate) fn container_id(name: &str) -> String {
    format!("c:{name}")
}

/// The item id a running row belongs to: its project, its container, or (for other
/// listeners) the row itself.
pub(crate) fn row_id(r: &PortApp) -> String {
    match r.kind {
        Kind::Dev if !r.folder.is_empty() => project_id(&r.folder),
        Kind::Docker => container_id(&r.docker_name),
        _ => r.key.clone(),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum RunState {
    Running,
    /// DevDuck started it and it isn't listening yet.
    Starting,
    /// Stopping, restarting or starting through a command still in flight.
    Busy(&'static str),
    Exited(Option<i32>),
    Stopped,
}

impl RunState {
    pub(crate) fn is_live(self) -> bool {
        matches!(self, RunState::Running | RunState::Starting | RunState::Busy(_))
    }
}

#[derive(Clone)]
pub(crate) struct Item<'a> {
    pub id: String,
    pub name: String,
    /// The project folder; empty for containers and other listeners.
    pub folder: String,
    /// The container name, for containers.
    pub container: Option<String>,
    pub framework: Option<String>,
    pub row: Option<&'a PortApp>,
    pub project: Option<&'a Project>,
    pub recent: Option<&'a Recent>,
    pub favorite: Option<&'a Favorite>,
    pub state: RunState,
    /// Unix seconds it last ran (now while running), 0 if never seen.
    pub last_used: u64,
}

impl Item<'_> {
    pub(crate) fn member(&self) -> Option<Member> {
        if let Some(c) = &self.container {
            Some(Member { kind: FavoriteKind::Container, target: c.clone() })
        } else if !self.folder.is_empty() {
            Some(Member { kind: FavoriteKind::Project, target: self.folder.clone() })
        } else {
            None
        }
    }

    pub(crate) fn is_container(&self) -> bool {
        self.container.is_some()
    }

    pub(crate) fn is_project(&self) -> bool {
        !self.folder.is_empty()
    }

    pub(crate) fn scripts(&self) -> &[Script] {
        self.project.map_or(&[], |p| p.scripts.as_slice())
    }
}

/// What Start runs for a project.
#[derive(Clone, Debug)]
pub(crate) struct RunSpec {
    pub command: String,
    pub cwd: String,
    /// Through `cmd /c` (scripts) rather than an exact command line seen running.
    pub shell: bool,
    pub script: Option<String>,
    /// Serve on this port instead of the usual one (already folded into `command`).
    pub port: Option<u16>,
}

fn folder_name(folder: &str) -> String {
    std::path::Path::new(folder).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| folder.to_string())
}

impl DevDuck {
    /// Every project and container, running or not. Other listeners (apps, Windows
    /// services) aren't items; they live in the port views.
    pub(crate) fn items(&self) -> Vec<Item<'_>> {
        let mut out: Vec<Item<'_>> = Vec::new();
        let mut at: HashMap<String, usize> = HashMap::new();
        let blank = |id: String, name: String, folder: String, container: Option<String>| Item {
            id,
            name,
            folder,
            container,
            framework: None,
            row: None,
            project: None,
            recent: None,
            favorite: None,
            state: RunState::Stopped,
            last_used: 0,
        };
        let mut slot = |out: &mut Vec<Item<'_>>, id: String, make: &dyn Fn(String) -> Item<'static>| -> usize {
            *at.entry(id.clone()).or_insert_with(|| {
                out.push(make(id));
                out.len() - 1
            })
        };

        for r in self.rows.iter().filter(|r| matches!(r.kind, Kind::Dev | Kind::Docker)) {
            let id = row_id(r);
            let i = match r.kind {
                Kind::Docker => slot(&mut out, id, &|id| blank(id, r.app.clone(), String::new(), Some(r.docker_name.clone()))),
                _ => slot(&mut out, id, &|id| blank(id, r.app.clone(), r.folder.clone(), None)),
            };
            let it = &mut out[i];
            it.row = Some(r);
            it.framework = r.framework.clone();
        }
        for p in &self.projects {
            let i = slot(&mut out, project_id(&p.folder), &|id| blank(id, p.name.clone(), p.folder.clone(), None));
            let it = &mut out[i];
            it.project = Some(p);
            if it.framework.is_none() {
                it.framework = p.framework.clone();
            }
        }
        for e in &self.state.recent {
            let i = slot(&mut out, project_id(&e.folder), &|id| blank(id, e.app.clone(), e.folder.clone(), None));
            let it = &mut out[i];
            it.recent = Some(e);
            it.last_used = it.last_used.max(e.last_seen);
            if it.framework.is_none() {
                it.framework = e.framework.clone();
            }
        }
        for f in &self.state.favorites {
            let i = match f.kind {
                FavoriteKind::Project => slot(&mut out, project_id(&f.target), &|id| blank(id, f.name.clone(), f.target.clone(), None)),
                FavoriteKind::Container => slot(&mut out, container_id(&f.target), &|id| blank(id, f.name.clone(), String::new(), Some(f.target.clone()))),
            };
            let it = &mut out[i];
            it.favorite = Some(f);
            if it.framework.is_none() {
                it.framework = f.framework.clone();
            }
        }
        for m in self.state.stacks.iter().flat_map(|s| &s.members) {
            match m.kind {
                FavoriteKind::Project => slot(&mut out, project_id(&m.target), &|id| blank(id, folder_name(&m.target), m.target.clone(), None)),
                FavoriteKind::Container => slot(&mut out, container_id(&m.target), &|id| blank(id, m.target.clone(), String::new(), Some(m.target.clone()))),
            };
        }

        let now = crate::store::now_unix();
        for it in &mut out {
            it.state = self.state_of(it);
            if it.row.is_some() {
                it.last_used = now;
            }
        }
        out
    }

    fn state_of(&self, it: &Item<'_>) -> RunState {
        if let Some(r) = it.row {
            return match self.busy.get(&r.key) {
                Some(label) => RunState::Busy(label),
                None => RunState::Running,
            };
        }
        if let Some(c) = &it.container {
            return if self.busy.contains_key(&format!("fav:container:{c}")) { RunState::Busy("Starting") } else { RunState::Stopped };
        }
        let lc = it.folder.to_lowercase();
        if self.busy.contains_key(&start_key(&it.folder)) || self.busy.contains_key(&format!("fav:project:{lc}")) {
            return RunState::Busy("Starting");
        }
        if self.launching.contains_key(&lc) {
            return RunState::Starting;
        }
        if let Some(e) = self.exits.get(&lc) {
            return RunState::Exited(e.code);
        }
        RunState::Stopped
    }

    /// One item by id, including other listeners (by row key).
    pub(crate) fn item(&self, id: &str) -> Option<Item<'_>> {
        if let Some(it) = self.items().into_iter().find(|i| i.id == id) {
            return Some(it);
        }
        let r = self.rows.iter().find(|r| r.key == id)?;
        Some(Item {
            id: r.key.clone(),
            name: r.app.clone(),
            folder: String::new(),
            container: None,
            framework: r.framework.clone(),
            row: Some(r),
            project: None,
            recent: None,
            favorite: None,
            state: match self.busy.get(&r.key) {
                Some(l) => RunState::Busy(l),
                None => RunState::Running,
            },
            last_used: 0,
        })
    }

    pub(crate) fn display_name(&self, folder: &str) -> String {
        self.project(folder).map(|p| p.name.clone()).or_else(|| self.recent(folder).map(|e| e.app.clone()).filter(|a| !a.is_empty())).unwrap_or_else(|| folder_name(folder))
    }

    /// What Start would run in `folder`: the named script; else the script DevDuck last
    /// ran there; else the command line last seen running; else the dev script.
    pub(crate) fn run_spec(&self, folder: &str, script: Option<&str>) -> Result<RunSpec, String> {
        let project = self.project(folder);
        let recent = self.recent(folder);
        let fav = self.state.favorites.iter().find(|f| f.kind == FavoriteKind::Project && f.target.eq_ignore_ascii_case(folder));
        let by_script = |s: &Script| RunSpec {
            command: s.command.clone(),
            cwd: project.map_or_else(|| folder.to_string(), |p| p.folder.clone()),
            shell: true,
            script: Some(s.name.clone()),
            port: None,
        };
        if let Some(name) = script {
            return project.and_then(|p| p.script(name)).map(by_script).ok_or_else(|| format!("This project has no script called {name}."));
        }
        if let Some(s) = recent.and_then(|e| e.script.as_deref()).and_then(|n| project.and_then(|p| p.script(n))) {
            return Ok(by_script(s));
        }
        for (cmd, cwd) in [recent.map(|e| (&e.command, &e.cwd)), fav.map(|f| (&f.command, &f.cwd))].into_iter().flatten() {
            if !cmd.is_empty() && cmd != UNAVAILABLE {
                return Ok(RunSpec { command: cmd.clone(), cwd: cwd.clone().unwrap_or_else(|| folder.to_string()), shell: false, script: None, port: None });
            }
        }
        if let Some(s) = project.and_then(Project::default_script) {
            return Ok(by_script(s));
        }
        Err("DevDuck doesn't know how to start this one: it has no dev script, and DevDuck hasn't seen it running. Run it once as usual and DevDuck will remember the command.".into())
    }

    /// The label for Start: the script it would run ("dev"), or None for a remembered
    /// command line.
    pub(crate) fn start_label(&self, folder: &str) -> Option<String> {
        self.run_spec(folder, None).ok().and_then(|s| s.script)
    }

    pub(crate) fn can_start(&self, folder: &str) -> bool {
        self.run_spec(folder, None).is_ok()
    }

    /// Ports a project usually takes: what it bound last time, else its framework's default.
    pub(crate) fn expected_ports(&self, folder: &str) -> Vec<u16> {
        let fav = self.state.favorites.iter().find(|f| f.kind == FavoriteKind::Project && f.target.eq_ignore_ascii_case(folder));
        let mut ports: Vec<u16> = self.recent(folder).map(|e| e.ports.clone()).filter(|p| !p.is_empty()).or_else(|| fav.map(|f| f.ports.clone())).unwrap_or_default();
        ports.retain(|p| *p < 49152);
        if ports.is_empty()
            && let Some(p) = self.project(folder).and_then(|p| p.framework.as_deref()).and_then(projects::framework_port)
        {
            ports.push(p);
        }
        ports
    }

    /// `spec` asked to serve on `port` instead (for scripts; a remembered command line is
    /// left alone apart from PORT in its environment).
    pub(crate) fn on_port(&self, folder: &str, mut spec: RunSpec, port: u16) -> RunSpec {
        if spec.script.is_some() {
            let fw = self.project(folder).and_then(|p| p.framework.as_deref());
            spec.command = projects::with_port(&spec.command, fw, port);
        }
        spec.port = Some(port);
        spec
    }

    /// The first port after `from` that nothing is listening on.
    pub(crate) fn next_free_port(&self, from: u16) -> u16 {
        let taken = |p: u16| self.rows.iter().any(|r| r.ports.iter().any(|x| x.port == p));
        let bindable = |p: u16| self.demo || (std::net::TcpListener::bind(("127.0.0.1", p)).is_ok() && std::net::TcpListener::bind(("0.0.0.0", p)).is_ok());
        (from.saturating_add(1)..from.saturating_add(200)).find(|p| !taken(*p) && bindable(*p)).unwrap_or(from.saturating_add(1))
    }

    /// Someone else already listening on a port this project wants.
    pub(crate) fn conflict(&self, folder: &str) -> Option<(u16, &PortApp)> {
        for port in self.expected_ports(folder) {
            if let Some(r) = self.rows.iter().find(|r| !(r.kind == Kind::Dev && r.folder.eq_ignore_ascii_case(folder)) && r.ports.iter().any(|p| p.port == port)) {
                return Some((port, r));
            }
        }
        None
    }

    fn member_running(&self, m: &Member) -> bool {
        match m.kind {
            FavoriteKind::Project => {
                self.rows.iter().any(|r| r.kind == Kind::Dev && r.folder.eq_ignore_ascii_case(&m.target)) || self.launching.contains_key(&m.target.to_lowercase())
            }
            FavoriteKind::Container => self.rows.iter().any(|r| r.kind == Kind::Docker && r.docker_name == m.target),
        }
    }

    /// (running, total) members of a stack.
    pub(crate) fn stack_status(&self, i: usize) -> (usize, usize) {
        let Some(s) = self.state.stacks.get(i) else { return (0, 0) };
        (s.members.iter().filter(|m| self.member_running(m)).count(), s.members.len())
    }

    pub(crate) fn stacks_of(&self, m: &Member) -> Vec<usize> {
        self.state.stacks.iter().enumerate().filter(|(_, s)| s.members.iter().any(|x| x.is(m.kind, &m.target))).map(|(i, _)| i).collect()
    }

    /// Everything a stack could hold, for the stack editor: (member, name, detail).
    pub(crate) fn stack_candidates(&self) -> Vec<(Member, String, String)> {
        let mut items = self.items();
        items.sort_by_key(|i| (!i.state.is_live(), i.favorite.is_none(), std::cmp::Reverse(i.last_used), i.name.to_lowercase()));
        items
            .into_iter()
            .filter_map(|i| {
                let m = i.member()?;
                let detail = match &i.container {
                    Some(_) => "container".to_string(),
                    None => super::widgets::abbreviate_home(&i.folder),
                };
                Some((m, i.name, detail))
            })
            .collect()
    }

    // ------------------------------------------------------------ palette --

    /// Every command the palette offers, most useful first.
    pub(crate) fn commands(&self, query: &str) -> Vec<Command> {
        let mut out = Vec::new();
        let q = query.trim();
        if let Ok(port) = q.trim_start_matches(':').parse::<u16>()
            && port > 0
        {
            out.push(Command::new(Icon::Open, format!("Open localhost:{port}"), String::new(), Action::OpenPort(port)));
            if let Some(r) = self.rows.iter().find(|r| r.ports.iter().any(|p| p.port == port)) {
                out.push(Command::new(Icon::Search, format!("What's on :{port}? {}", r.app), r.process.clone(), Action::Select(row_id(r))));
                if r.kind != Kind::System && (r.kill_pid > 4 || r.kind == Kind::Docker) {
                    out.push(Command::new(Icon::Stop, format!("Stop what's on :{port} ({})", r.app), String::new(), Action::AskStop(r.key.clone())));
                }
            }
        }

        let mut items = self.items();
        items.sort_by_key(|i| (!i.state.is_live(), i.favorite.is_none(), std::cmp::Reverse(i.last_used), i.name.to_lowercase()));
        let mut later = Vec::new();
        for it in &items {
            let n = &it.name;
            match (it.state, it.row) {
                (RunState::Running, Some(r)) => {
                    if r.kind == Kind::Dev && !matches!(self.health(r), Health::Internal | Health::NotHttp) {
                        let port = self.main_port(r);
                        out.push(Command::new(Icon::Open, format!("Open {n}"), format!("localhost:{port}"), Action::OpenMain(r.key.clone())));
                    }
                    out.push(Command::new(Icon::Restart, format!("Restart {n}"), String::new(), Action::Restart(r.key.clone())));
                    out.push(Command::new(Icon::Stop, format!("Stop {n}"), String::new(), Action::AskStop(r.key.clone())));
                    let what = if it.is_container() { "logs" } else { "output" };
                    later.push(Command::new(Icon::Terminal, format!("Show {what} of {n}"), String::new(), Action::Show(it.id.clone(), Tab::Output)));
                }
                (RunState::Starting, _) => {
                    out.push(Command::new(Icon::Terminal, format!("Show output of {n}"), "starting".into(), Action::Show(it.id.clone(), Tab::Output)));
                    out.push(Command::new(Icon::Stop, format!("Stop {n}"), String::new(), Action::StopItem(it.id.clone())));
                }
                (RunState::Stopped | RunState::Exited(_), _) => {
                    if let Some(c) = &it.container {
                        out.push(Command::new(Icon::Play, format!("Start {n}"), "docker start".into(), Action::StartContainer(c.clone())));
                    } else if self.can_start(&it.folder) {
                        let hint = self.start_label(&it.folder).unwrap_or_else(|| "last command".into());
                        out.push(Command::new(Icon::Play, format!("Start {n}"), hint, Action::Run(it.folder.clone(), None)));
                    }
                    for s in it.scripts() {
                        later.push(Command::new(Icon::Play, format!("Run {n}: {}", s.name), s.detail.clone(), Action::Run(it.folder.clone(), Some(s.name.clone()))));
                    }
                }
                _ => {}
            }
            if it.is_project() {
                let editor = crate::launch::editor_name(self.state.settings.editor);
                later.push(Command::new(Icon::Code, format!("Open {n} in {editor}"), String::new(), Action::Editor(it.folder.clone())));
                later.push(Command::new(Icon::Folder, format!("Open {n} folder"), String::new(), Action::Folder(it.folder.clone())));
                later.push(Command::new(Icon::Terminal, format!("Terminal in {n}"), String::new(), Action::Terminal(it.folder.clone())));
            }
        }
        for (i, s) in self.state.stacks.iter().enumerate() {
            let (up, total) = self.stack_status(i);
            let hint = format!("{up} of {total} running");
            if up < total {
                out.push(Command::new(Icon::Play, format!("Start stack {}", s.name), hint.clone(), Action::StartStack(i)));
            }
            if up > 0 {
                out.push(Command::new(Icon::Stop, format!("Stop stack {}", s.name), hint, Action::StopStack(i)));
            }
        }
        out.extend(later);
        if self.rows.iter().any(|r| r.kind == Kind::Dev && r.kill_pid > 4) {
            out.push(Command::new(Icon::Stop, "Stop all dev servers".into(), String::new(), Action::StopAllDev));
        }
        out.push(Command::new(Icon::Layers, "New stack\u{2026}".into(), "start and stop projects together".into(), Action::NewStack));
        out.push(Command::new(Icon::Refresh, "Look for projects again".into(), String::new(), Action::RescanProjects));
        out.push(Command::new(Icon::Grid, "Show projects".into(), String::new(), Action::SetChip(super::Chip::Projects)));
        out.push(Command::new(Icon::List, "Show every listening port".into(), String::new(), Action::SetChip(super::Chip::All)));
        out.push(Command::new(Icon::Settings, "Settings".into(), String::new(), Action::OpenSettings));

        if q.is_empty() {
            return out;
        }
        let tokens: Vec<String> = q.to_lowercase().split_whitespace().map(str::to_string).collect();
        let mut scored: Vec<(usize, usize, Command)> = out
            .into_iter()
            .enumerate()
            .filter_map(|(order, c)| {
                let label = c.label.to_lowercase();
                let mut score = 0;
                for t in &tokens {
                    let pos = label.find(t.as_str()).or_else(|| if t.chars().all(|ch| ch.is_ascii_digit()) { c.hint.find(t.as_str()).map(|p| p + 40) } else { None })?;
                    let word_start = pos == 0 || !label.as_bytes()[pos - 1].is_ascii_alphanumeric();
                    score += if word_start { pos / 4 } else { pos + 20 };
                }
                Some((score, order, c))
            })
            .collect();
        scored.sort_by_key(|(s, o, _)| (*s, *o));
        scored.into_iter().map(|(_, _, c)| c).collect()
    }
}

pub(crate) fn start_key(folder: &str) -> String {
    format!("start:{}", folder.to_lowercase())
}

#[derive(Clone)]
pub(crate) struct Command {
    pub icon: Icon,
    pub label: String,
    pub hint: String,
    pub action: Action,
}

impl Command {
    fn new(icon: Icon, label: String, hint: String, action: Action) -> Self {
        Command { icon, label, hint, action }
    }
}
