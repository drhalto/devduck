//! The scan: listening ports -> owning processes -> one row per project (or Docker
//! container, or app), each with the root process that stopping it should kill.
//!
//! The grouping rules are a straight port of v1 (DevPorts.ps1, in the git history);
//! the identity columns (app, ports, process, kind, folder) matched its -Smoke output
//! row for row. On top of that each row now carries the whole process tree
//! under its root (memory, CPU, process count), bind addresses, a framework guess and
//! the root's command line + working directory, so the GUI can restart it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime};

use regex::Regex;

use crate::win;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum Kind {
    // Declaration order is the display order.
    Dev,
    Docker,
    App,
    System,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Dev => "dev",
            Kind::Docker => "docker",
            Kind::App => "app",
            Kind::System => "system",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Exposure {
    /// Only 127.0.0.1 / ::1.
    Loopback,
    /// 0.0.0.0 or :: - reachable from the LAN (firewall permitting).
    All,
    /// Bound to one specific non-loopback address.
    Specific,
}

#[derive(Clone, Debug)]
pub struct PortInfo {
    pub port: u16,
    pub addrs: Vec<IpAddr>,
}

impl PortInfo {
    pub fn exposure(&self) -> Exposure {
        if self.addrs.iter().any(|a| a.is_unspecified()) {
            Exposure::All
        } else if !self.addrs.is_empty() && self.addrs.iter().all(|a| a.is_loopback()) {
            Exposure::Loopback
        } else {
            Exposure::Specific
        }
    }

    /// Ports in the dynamic range are almost always internal (HMR sockets, debuggers,
    /// worker IPC) rather than something you'd open in a browser.
    pub fn is_ephemeral(&self) -> bool {
        self.port >= 49152
    }

    pub fn addrs_text(&self) -> String {
        let fmt = |a: &IpAddr| match a {
            IpAddr::V6(v6) => format!("[{v6}]"),
            IpAddr::V4(v4) => v4.to_string(),
        };
        self.addrs.iter().map(fmt).collect::<Vec<_>>().join(", ")
    }
}

#[derive(Clone, Debug)]
pub struct TreeProc {
    pub pid: u32,
    pub depth: usize,
    pub name: String,
    pub mem: u64,
}

#[derive(Clone, Debug)]
pub struct PortApp {
    /// Stable identity across refreshes: "pid:<root>" or "docker:<name>".
    pub key: String,
    pub app: String,
    /// Sorted by port, non-empty.
    pub ports: Vec<PortInfo>,
    pub ports_text: String,
    /// "node.exe (1234)" or "container (name)".
    pub process: String,
    pub folder: String,
    pub uptime: String,
    pub mem: String,
    pub mem_bytes: u64,
    /// Share of total machine CPU over the last interval, 0-100. None on the first scan.
    pub cpu: Option<f32>,
    pub kind: Kind,
    /// Command line of the first listener in the group.
    pub cmd: String,
    /// Command line and working directory of the kill root (what Restart re-runs).
    pub root_cmd: String,
    pub cwd: Option<String>,
    /// Root of the process tree to kill; 0 for Docker rows.
    pub kill_pid: u32,
    pub docker_name: String,
    pub docker_image: String,
    /// "Next.js", "Vite", "Python", "postgres"...
    pub framework: Option<String>,
    /// Everything Stop would kill, root first (depth-first). Empty for Docker/system rows.
    pub tree: Vec<TreeProc>,
}

impl PortApp {
    pub fn first_port(&self) -> u16 {
        self.ports[0].port
    }

    pub fn port_numbers(&self) -> Vec<u16> {
        self.ports.iter().map(|p| p.port).collect()
    }

}

const SYSTEM_NAMES: &[&str] = &[
    "system", "idle", "svchost.exe", "lsass.exe", "services.exe", "wininit.exe", "winlogon.exe",
    "csrss.exe", "spoolsv.exe", "dns.exe", "searchhost.exe", "memory compression",
];
const RUNNER_NAMES: &[&str] =
    &["node.exe", "cmd.exe", "powershell.exe", "pwsh.exe", "bun.exe", "deno.exe", "python.exe", "dotnet.exe"];
const NEVER_KILL_ROOT: &[&str] = &[
    "explorer.exe", "windowsterminal.exe", "wt.exe", "conhost.exe", "openconsole.exe", "code.exe",
    "cursor.exe", "devenv.exe", "services.exe", "svchost.exe", "wininit.exe", "userinit.exe",
];

static RUNNER_CMD_HINT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)npm|pnpm|yarn|bun|vite|next|turbo|nodemon|webpack|astro|remix|nuxt|ng serve|dotnet watch|uvicorn|flask|manage\.py").unwrap()
});
static PATH_IN_CMD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""([A-Za-z]:\\[^"]+)"|([A-Za-z]:\\[^\s"]+)"#).unwrap());
static NOT_A_PROJECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\\AppData\\|\\node_modules$|\.exe"?$"#).unwrap());
static DOCKER_HOST_PORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":(\d+)->").unwrap());
static DOCKER_UP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^Up\s+").unwrap());
static DOCKER_PAREN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(.*\)$").unwrap());

fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack.to_ascii_lowercase().find(&needle.to_ascii_lowercase())
}

fn starts_with_ignore_case(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

struct Proc {
    parent_pid: u32,
    name: String,
    name_lower: String,
    working_set: u64,
    create_time: u64,
    cpu_time: u64,
}

/// One point-in-time view of the process table. Command lines are fetched lazily:
/// only listeners and their ancestors are ever asked for.
struct Snapshot {
    procs: HashMap<u32, Proc>,
    cmdlines: HashMap<u32, String>,
}

impl Snapshot {
    fn take() -> Self {
        let procs = win::processes()
            .into_iter()
            .map(|p| {
                let name_lower = p.name.to_lowercase();
                let proc = Proc {
                    parent_pid: p.parent_pid,
                    name: p.name,
                    name_lower,
                    working_set: p.working_set,
                    create_time: p.create_time,
                    cpu_time: p.cpu_time,
                };
                (p.pid, proc)
            })
            .collect();
        Snapshot { procs, cmdlines: HashMap::new() }
    }

    /// Empty when the process is unknown or its command line can't be read.
    fn cmdline(&mut self, pid: u32) -> String {
        if !self.procs.contains_key(&pid) {
            return String::new();
        }
        self.cmdlines.entry(pid).or_insert_with(|| win::command_line(pid).unwrap_or_default()).clone()
    }

    /// parent -> children, ignoring "children" older than their parent (a reused pid).
    fn children(&self) -> HashMap<u32, Vec<u32>> {
        let mut map: HashMap<u32, Vec<u32>> = HashMap::new();
        for (&pid, p) in &self.procs {
            if let Some(parent) = self.procs.get(&p.parent_pid)
                && pid != p.parent_pid
                && p.create_time >= parent.create_time
            {
                map.entry(p.parent_pid).or_default().push(pid);
            }
        }
        for kids in map.values_mut() {
            kids.sort_unstable();
        }
        map
    }

    fn tree(&self, root: u32, children: &HashMap<u32, Vec<u32>>) -> Vec<TreeProc> {
        let mut out = Vec::new();
        let mut stack = vec![(root, 0usize)];
        let mut seen = BTreeSet::new();
        while let Some((pid, depth)) = stack.pop() {
            if !seen.insert(pid) || out.len() >= 400 {
                continue;
            }
            let Some(p) = self.procs.get(&pid) else { continue };
            out.push(TreeProc { pid, depth, name: p.name.clone(), mem: p.working_set });
            if let Some(kids) = children.get(&pid) {
                for &kid in kids.iter().rev() {
                    stack.push((kid, depth + 1));
                }
            }
        }
        out
    }
}

/// The project folder a command line belongs to: the part before `\node_modules\`,
/// or else the first path under the user profile that isn't AppData or an .exe.
fn project_dir(cmdline: &str, home: &str) -> String {
    let candidates: Vec<&str> = PATH_IN_CMD
        .captures_iter(cmdline)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)).map(|m| m.as_str()))
        .collect();
    for c in &candidates {
        if let Some(i) = find_ignore_case(c, r"\node_modules\")
            && i > 0
        {
            return c[..i].to_string();
        }
    }
    for c in &candidates {
        if starts_with_ignore_case(c, home) && !NOT_A_PROJECT.is_match(c) {
            if Path::new(c).is_dir() {
                return c.to_string();
            }
            if let Some(parent) = Path::new(c).parent().map(|p| p.to_string_lossy().into_owned())
                && !parent.is_empty()
                && !parent.eq_ignore_ascii_case(home)
            {
                return parent;
            }
        }
    }
    String::new()
}

/// Walk up from a listener while the parent is the same project or a dev runner
/// (npm, vite, next dev, uvicorn...), so Stop takes the whole dev server down.
/// Never walks into shells, editors or core Windows processes.
fn kill_root(pid: u32, snap: &mut Snapshot, project: &str) -> u32 {
    let mut current = pid;
    for _ in 0..8 {
        let Some(p) = snap.procs.get(&current) else { break };
        let parent_pid = p.parent_pid;
        if parent_pid <= 4 {
            break;
        }
        let Some(parent) = snap.procs.get(&parent_pid) else { break };
        if NEVER_KILL_ROOT.contains(&parent.name_lower.as_str()) {
            break;
        }
        let is_runner_name = RUNNER_NAMES.contains(&parent.name_lower.as_str());
        let parent_cmd = snap.cmdline(parent_pid);
        let same_project = !project.is_empty() && !parent_cmd.is_empty() && find_ignore_case(&parent_cmd, project).is_some();
        let is_runner = is_runner_name && RUNNER_CMD_HINT.is_match(&parent_cmd);
        if same_project || is_runner {
            current = parent_pid;
        } else {
            break;
        }
    }
    current
}

#[derive(Clone)]
struct Container {
    name: String,
    image: String,
    up: String,
}

/// Host port -> container, from `docker ps`. Skipped unless Docker Desktop is running,
/// and given up on after a few seconds if the daemon is wedged.
fn docker_ports(snap: &Snapshot) -> HashMap<u16, Container> {
    let mut map = HashMap::new();
    if !snap.procs.values().any(|p| p.name_lower == "com.docker.backend.exe") {
        return map;
    }
    let Some(out) = run_with_timeout(
        Command::new("docker").args(["ps", "--format", "{{.Names}}|{{.Image}}|{{.Status}}|{{.Ports}}"]),
        Duration::from_secs(5),
    ) else {
        return map;
    };
    for line in out.lines() {
        let parts: Vec<&str> = line.splitn(4, '|').collect();
        if parts.len() < 4 {
            continue;
        }
        let up = DOCKER_PAREN.replace(&DOCKER_UP.replace(parts[2], ""), "").into_owned();
        let container = Container { name: parts[0].to_string(), image: parts[1].to_string(), up };
        for m in DOCKER_HOST_PORT.captures_iter(parts[3]) {
            if let Ok(port) = m[1].parse::<u16>() {
                map.insert(port, container.clone());
            }
        }
    }
    map
}

/// Run a command with no console window and return its stdout, or None on failure/timeout.
pub fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<String> {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    let mut child = cmd
        .creation_flags(win::CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().ok()?;
                return status.success().then_some(out);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Best guess at what a dev server is, from its command lines, then its package.json.
fn detect_framework(cmds: &str, exe: &str, folder: &str, package_deps: Option<&BTreeSet<String>>) -> Option<String> {
    let c = cmds.to_ascii_lowercase().replace('/', "\\");
    let has = |s: &str| c.contains(s);
    let by_cmd = if has("storybook") {
        Some("Storybook")
    } else if has("remotion") {
        Some("Remotion")
    } else if has("\\next\\") || has("next dev") || has("next start") || has("next-server") {
        Some("Next.js")
    } else if has("\\astro\\") || has("astro dev") {
        Some("Astro")
    } else if has("nuxt") {
        Some("Nuxt")
    } else if has("svelte-kit") || has("@sveltejs") {
        Some("SvelteKit")
    } else if has("wrangler") {
        Some("Wrangler")
    } else if has("\\expo\\") || has("expo start") {
        Some("Expo")
    } else if has("\\vite\\") || has("vite ") || c.ends_with("vite") {
        Some("Vite")
    } else if has("uvicorn") {
        Some("Uvicorn")
    } else if has("flask") {
        Some("Flask")
    } else if has("manage.py") {
        Some("Django")
    } else if has("jupyter") {
        Some("Jupyter")
    } else if has("http.server") {
        Some("Python http")
    } else if has("supabase") {
        Some("Supabase")
    } else if has("webpack") {
        Some("Webpack")
    } else {
        None
    };
    if let Some(f) = by_cmd {
        return Some(f.to_string());
    }
    if let Some(deps) = package_deps {
        let d = |s: &str| deps.contains(s);
        let by_dep = if d("next") {
            Some("Next.js")
        } else if d("@remotion/cli") || d("remotion") {
            Some("Remotion")
        } else if d("astro") {
            Some("Astro")
        } else if d("nuxt") {
            Some("Nuxt")
        } else if d("@sveltejs/kit") {
            Some("SvelteKit")
        } else if d("@react-router/dev") {
            Some("React Router")
        } else if d("@remix-run/dev") {
            Some("Remix")
        } else if d("@angular/core") {
            Some("Angular")
        } else if d("expo") {
            Some("Expo")
        } else if d("electron") {
            Some("Electron")
        } else if d("vite") {
            Some("Vite")
        } else if d("@nestjs/core") {
            Some("NestJS")
        } else if d("hono") {
            Some("Hono")
        } else if d("fastify") {
            Some("Fastify")
        } else if d("express") {
            Some("Express")
        } else {
            None
        };
        if let Some(f) = by_dep {
            return Some(f.to_string());
        }
    }
    let exe = exe.to_ascii_lowercase();
    let generic = match exe.as_str() {
        "node.exe" => Some("Node"),
        "python.exe" | "pythonw.exe" => Some("Python"),
        "bun.exe" => Some("Bun"),
        "deno.exe" => Some("Deno"),
        "dotnet.exe" => Some(".NET"),
        "java.exe" => Some("Java"),
        "ruby.exe" => Some("Ruby"),
        "php.exe" => Some("PHP"),
        "go.exe" => Some("Go"),
        _ => None,
    };
    let _ = folder;
    generic.map(str::to_string)
}

/// "supabase/postgres:15.8" -> "postgres"; "redis:7-alpine" -> "redis".
fn image_base(image: &str) -> String {
    let no_tag = image.rsplit_once(':').filter(|(_, t)| !t.contains('/')).map_or(image, |(i, _)| i);
    no_tag.rsplit('/').next().unwrap_or(no_tag).to_string()
}

fn format_uptime(ticks: u64) -> String {
    let secs = ticks / 10_000_000;
    let (days, hours, mins) = (secs / 86_400, secs / 3_600, secs / 60);
    if days >= 1 {
        format!("{days}d {}h", hours % 24)
    } else if hours >= 1 {
        format!("{hours}h {}m", mins % 60)
    } else {
        format!("{}m", mins.max(1))
    }
}

pub fn format_mem(bytes: u64) -> String {
    const MB: f64 = (1u64 << 20) as f64;
    const GB: f64 = (1u64 << 30) as f64;
    let b = bytes as f64;
    if b >= GB {
        return format!("{:.1} GB", b / GB);
    }
    let mb = (b / MB).round() as u64;
    if mb >= 1000 { format!("{},{:03} MB", mb / 1000, mb % 1000) } else { format!("{mb} MB") }
}

struct Group {
    key: String,
    kind: Kind,
    app: String,
    folder: String,
    ports: BTreeMap<u16, Vec<IpAddr>>,
    pids: BTreeSet<u32>,
    docker_name: String,
    docker_image: String,
    docker_up: String,
    kill_pid: u32,
    cmd: String,
    root_name: String,
}

/// Scans keep a little state between calls: CPU times (for CPU %), and caches for the
/// slow-ish lookups (docker ps, package.json, process working directories).
#[derive(Default)]
pub struct Scanner {
    prev_cpu: HashMap<u32, (u64, u64)>,
    prev_wall: u64,
    docker: Option<(Instant, HashMap<u16, Container>)>,
    packages: HashMap<String, (Option<SystemTime>, Option<BTreeSet<String>>)>,
    cwds: HashMap<(u32, u64), Option<String>>,
}

impl Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    fn package_deps(&mut self, folder: &str) -> Option<BTreeSet<String>> {
        if folder.is_empty() {
            return None;
        }
        let path = Path::new(folder).join("package.json");
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if let Some((cached_mtime, deps)) = self.packages.get(folder)
            && *cached_mtime == mtime
        {
            return deps.clone();
        }
        let deps = mtime.and_then(|_| {
            let text = std::fs::read_to_string(&path).ok()?;
            let json: serde_json::Value = serde_json::from_str(&text).ok()?;
            let mut set = BTreeSet::new();
            for section in ["dependencies", "devDependencies"] {
                if let Some(obj) = json.get(section).and_then(|v| v.as_object()) {
                    set.extend(obj.keys().cloned());
                }
            }
            Some(set)
        });
        self.packages.insert(folder.to_string(), (mtime, deps.clone()));
        deps
    }

    /// One full scan. `fresh_docker` forces `docker ps` instead of using the ~8 s cache.
    pub fn scan(&mut self, fresh_docker: bool) -> Vec<PortApp> {
        let mut snap = Snapshot::take();
        let docker_stale = self.docker.as_ref().is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(8));
        if fresh_docker || docker_stale {
            self.docker = Some((Instant::now(), docker_ports(&snap)));
        }
        let docker = self.docker.as_ref().map(|(_, m)| m.clone()).unwrap_or_default();
        let home = std::env::var("USERPROFILE").unwrap_or_default();

        let mut seen = BTreeSet::new();
        let mut groups: HashMap<String, Group> = HashMap::new();
        for listener in win::tcp_listeners() {
            let (port, pid) = (listener.port, listener.pid);
            let first_time = seen.insert((port, pid));
            let (name, name_lower) = match snap.procs.get(&pid) {
                Some(p) => (p.name.clone(), p.name_lower.clone()),
                None => ("unknown".to_string(), "unknown".to_string()),
            };
            let cmd = snap.cmdline(pid);

            let key = if let Some(c) = docker.get(&port) {
                let key = format!("docker:{}", c.name);
                groups.entry(key.clone()).or_insert_with(|| Group {
                    key: key.clone(),
                    kind: Kind::Docker,
                    app: c.name.clone(),
                    folder: format!("image: {}", c.image),
                    ports: BTreeMap::new(),
                    pids: BTreeSet::new(),
                    docker_name: c.name.clone(),
                    docker_image: c.image.clone(),
                    docker_up: c.up.clone(),
                    kill_pid: 0,
                    cmd: cmd.clone(),
                    root_name: "container".to_string(),
                });
                key
            } else {
                let is_system = pid <= 4 || SYSTEM_NAMES.contains(&name_lower.as_str());
                let project = if is_system { String::new() } else { project_dir(&cmd, &home) };
                let root = if is_system { pid } else { kill_root(pid, &mut snap, &project) };
                let key = format!("pid:{root}");
                if !groups.contains_key(&key) {
                    let root_name = snap.procs.get(&root).map_or(name.clone(), |p| p.name.clone());
                    let kind = if is_system {
                        Kind::System
                    } else if !project.is_empty() {
                        Kind::Dev
                    } else {
                        Kind::App
                    };
                    let app = if !project.is_empty() {
                        Path::new(&project).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                    } else {
                        Path::new(&root_name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                    };
                    groups.insert(key.clone(), Group {
                        key: key.clone(),
                        kind,
                        app,
                        folder: project,
                        ports: BTreeMap::new(),
                        pids: BTreeSet::new(),
                        docker_name: String::new(),
                        docker_image: String::new(),
                        docker_up: String::new(),
                        kill_pid: root,
                        cmd: cmd.clone(),
                        root_name,
                    });
                }
                key
            };
            let g = groups.get_mut(&key).unwrap();
            let addrs = g.ports.entry(port).or_default();
            if !addrs.contains(&listener.addr) {
                addrs.push(listener.addr);
            }
            if first_time {
                g.pids.insert(pid);
            }
        }

        let now = win::now_filetime();
        let wall = now.saturating_sub(self.prev_wall);
        let cpus = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
        let children = snap.children();
        let mut rows = Vec::with_capacity(groups.len());
        for g in groups.into_values() {
            let docker_row = g.kind == Kind::Docker;
            let mut oldest = now;
            for pid in &g.pids {
                if let Some(p) = snap.procs.get(pid)
                    && p.create_time != 0
                    && p.create_time < oldest
                {
                    oldest = p.create_time;
                }
            }
            let tree = if matches!(g.kind, Kind::Dev | Kind::App) { snap.tree(g.kill_pid, &children) } else { Vec::new() };
            let accounted: Vec<u32> = if tree.is_empty() { g.pids.iter().copied().collect() } else { tree.iter().map(|t| t.pid).collect() };
            let mem: u64 = accounted.iter().filter_map(|p| snap.procs.get(p)).map(|p| p.working_set).sum();
            let cpu = (self.prev_wall != 0 && wall > 0 && !docker_row).then(|| {
                let used: u64 = accounted
                    .iter()
                    .filter_map(|pid| snap.procs.get(pid).map(|p| (pid, p)))
                    .map(|(pid, p)| match self.prev_cpu.get(pid) {
                        Some(&(created, before)) if created == p.create_time => p.cpu_time.saturating_sub(before),
                        _ if p.create_time > self.prev_wall => p.cpu_time, // started since the last scan
                        _ => 0,
                    })
                    .sum();
                ((used as f64 / (wall as f64 * cpus)) * 100.0).clamp(0.0, 100.0) as f32
            });

            let root_cmd = if docker_row { String::new() } else { snap.cmdline(g.kill_pid) };
            let cwd = if g.kind == Kind::Dev {
                let created = snap.procs.get(&g.kill_pid).map_or(0, |p| p.create_time);
                self.cwds.entry((g.kill_pid, created)).or_insert_with(|| win::current_dir(g.kill_pid)).clone()
            } else {
                None
            };
            let framework = match g.kind {
                Kind::Docker => Some(image_base(&g.docker_image)),
                Kind::Dev => {
                    let deps = self.package_deps(&g.folder);
                    detect_framework(&format!("{} {}", g.cmd, root_cmd), &g.root_name, &g.folder, deps.as_ref())
                }
                Kind::App | Kind::System => None,
            };
            let ports: Vec<PortInfo> = g.ports.into_iter().map(|(port, addrs)| PortInfo { port, addrs }).collect();
            rows.push(PortApp {
                key: g.key,
                ports_text: ports.iter().map(|p| p.port.to_string()).collect::<Vec<_>>().join(", "),
                ports,
                process: format!("{} ({})", g.root_name, if docker_row { g.docker_name.clone() } else { g.kill_pid.to_string() }),
                app: g.app,
                folder: g.folder,
                uptime: if docker_row { g.docker_up } else { format_uptime(now.saturating_sub(oldest)) },
                mem: if docker_row { String::new() } else { format_mem(mem) },
                mem_bytes: if docker_row { 0 } else { mem },
                cpu,
                kind: g.kind,
                cmd: if g.cmd.is_empty() { "(command line unavailable)".to_string() } else { g.cmd },
                root_cmd,
                cwd,
                kill_pid: g.kill_pid,
                docker_name: g.docker_name,
                docker_image: g.docker_image,
                framework,
                tree,
            });
        }
        rows.sort_by(|a, b| (a.kind, a.first_port(), &a.app).cmp(&(b.kind, b.first_port(), &b.app)));

        self.prev_cpu = snap.procs.iter().map(|(&pid, p)| (pid, (p.create_time, p.cpu_time))).collect();
        self.prev_wall = now;
        let live: BTreeSet<u32> = snap.procs.keys().copied().collect();
        self.cwds.retain(|(pid, _), _| live.contains(pid));
        rows
    }
}

/// Made-up rows for screenshots (`devduck --demo`). Nothing here is real, and the
/// GUI refuses to stop or start anything in demo mode.
pub fn demo_rows(tick: u64) -> Vec<PortApp> {
    use std::net::{Ipv4Addr, Ipv6Addr};
    let all = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
    let local = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let wobble = |base: f32, amp: f32, phase: u64| base + amp * (((tick + phase) as f32) * 0.7).sin().abs();
    #[allow(clippy::too_many_arguments)]
    let row = |key: &str, app: &str, ports: &[(u16, IpAddr)], process: &str, kind: Kind, up: u64, mem_mb: f32, cpu: Option<f32>, folder: &str, fw: Option<&str>, cmd: &str| {
        let ports: Vec<PortInfo> = ports.iter().map(|&(port, a)| PortInfo { port, addrs: vec![a] }).collect();
        let tree: Vec<TreeProc> = if matches!(kind, Kind::Dev) {
            vec![
                TreeProc { pid: 18244, depth: 0, name: "cmd.exe".into(), mem: 4 << 20 },
                TreeProc { pid: 18250, depth: 1, name: "node.exe".into(), mem: 61 << 20 },
                TreeProc { pid: 18302, depth: 2, name: "node.exe".into(), mem: ((mem_mb as u64) << 20).saturating_sub(65 << 20) },
            ]
        } else {
            Vec::new()
        };
        PortApp {
            key: key.into(),
            app: app.into(),
            ports_text: ports.iter().map(|p| p.port.to_string()).collect::<Vec<_>>().join(", "),
            ports,
            process: process.into(),
            folder: folder.into(),
            uptime: format_uptime(up * 10_000_000),
            mem: if kind == Kind::Docker { String::new() } else { format_mem((mem_mb as u64) << 20) },
            mem_bytes: if kind == Kind::Docker { 0 } else { (mem_mb as u64) << 20 },
            cpu,
            kind,
            cmd: cmd.into(),
            root_cmd: cmd.into(),
            cwd: (!folder.is_empty() && kind == Kind::Dev).then(|| folder.to_string()),
            kill_pid: process.rsplit_once('(').and_then(|(_, p)| p.trim_end_matches(')').parse().ok()).unwrap_or(0),
            docker_name: if kind == Kind::Docker { app.into() } else { String::new() },
            docker_image: if kind == Kind::Docker { folder.trim_start_matches("image: ").into() } else { String::new() },
            framework: fw.map(str::to_string),
            tree,
        }
    };
    vec![
        row("pid:1", "acme-storefront", &[(3000, all), (61427, local)], "cmd.exe (18244)", Kind::Dev, 2 * 3600 + 14 * 60, wobble(612.0, 40.0, 0), Some(wobble(2.0, 6.0, 1)), r"C:\Users\dev\acme-storefront", Some("Next.js"), r"C:\WINDOWS\system32\cmd.exe /d /s /c next dev --turbopack"),
        row("pid:2", "marketing-site", &[(5173, local)], "node.exe (22960)", Kind::Dev, 48 * 60, wobble(188.0, 12.0, 3), Some(wobble(0.4, 1.0, 2)), r"C:\Users\dev\marketing-site", Some("Vite"), r#""C:\Program Files\nodejs\node.exe" C:\Users\dev\marketing-site\node_modules\vite\bin\vite.js"#),
        row("pid:3", "internal-api", &[(4000, all)], "python.exe (7788)", Kind::Dev, 27 * 3600, wobble(96.0, 4.0, 5), Some(wobble(0.2, 0.6, 4)), r"C:\Users\dev\internal-api", Some("Uvicorn"), r"python.exe -m uvicorn app.main:app --reload --port 4000"),
        row("pid:4", "launch-film", &[(3003, local)], "node.exe (30112)", Kind::Dev, 2 * 3600 + 14 * 60, wobble(274.0, 30.0, 7), Some(wobble(1.0, 9.0, 6)), r"C:\Users\dev\launch-film", Some("Remotion"), r#""C:\Program Files\nodejs\node.exe" C:\Users\dev\launch-film\node_modules\@remotion\cli\remotion-cli.js studio"#),
        row("docker:postgres-16", "postgres-16", &[(5432, IpAddr::V4(Ipv4Addr::UNSPECIFIED))], "container (postgres-16)", Kind::Docker, 6 * 86400, 0.0, None, "image: postgres:16", Some("postgres"), "(command line unavailable)"),
        row("docker:redis", "redis", &[(6379, IpAddr::V4(Ipv4Addr::UNSPECIFIED))], "container (redis)", Kind::Docker, 6 * 86400, 0.0, None, "image: redis:7-alpine", Some("redis"), "(command line unavailable)"),
        row("pid:7", "Spotify", &[(4381, local)], "Spotify.exe (14520)", Kind::App, 3 * 3600 + 20 * 60, 241.0, Some(0.3), "", None, r#""C:\Users\dev\AppData\Roaming\Spotify\Spotify.exe""#),
        row("pid:8", "com.docker.backend", &[(9223, local)], "com.docker.backend.exe (6020)", Kind::App, 6 * 86400, 58.0, Some(0.1), "", None, r#""C:\Program Files\Docker\Docker\resources\com.docker.backend.exe""#),
        row("pid:9", "svchost", &[(135, all)], "svchost.exe (1112)", Kind::System, 6 * 86400, 23.0, None, "", None, "(command line unavailable)"),
        row("pid:10", "System", &[(139, all), (445, all), (5357, all)], "System (4)", Kind::System, 6 * 86400, 24.0, None, "", None, "(command line unavailable)"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = r"C:\Users\dev";

    #[test]
    fn project_from_node_modules_path() {
        let cmd = r#""C:\Program Files\nodejs\node.exe" C:\Users\dev\acme-web\node_modules\next\dist\server\lib\start-server.js"#;
        assert_eq!(project_dir(cmd, HOME), r"C:\Users\dev\acme-web");
    }

    #[test]
    fn project_from_quoted_script_under_home() {
        // Not a real directory, so the script's parent folder is used.
        let cmd = r#"python.exe "C:\Users\dev\no-such-project-xyz\server.py" --port 8000"#;
        assert_eq!(project_dir(cmd, HOME), r"C:\Users\dev\no-such-project-xyz");
    }

    #[test]
    fn appdata_and_exes_are_not_projects() {
        assert_eq!(project_dir(r"C:\Users\dev\AppData\Local\Programs\thing\thing.exe --serve", HOME), "");
        assert_eq!(project_dir(r#""C:\Users\dev\tools\app.exe""#, HOME), "");
        assert_eq!(project_dir(r"C:\Windows\System32\svchost.exe -k netsvcs", HOME), "");
    }

    #[test]
    fn file_directly_in_home_is_not_a_project() {
        assert_eq!(project_dir(r"node C:\Users\dev\script.js", HOME), "");
    }

    #[test]
    fn uptime_truncates_instead_of_rounding() {
        let t = |secs: u64| format_uptime(secs * 10_000_000);
        assert_eq!(t(10), "1m");
        assert_eq!(t(59 * 60 + 50), "59m");
        assert_eq!(t(3600 + 59 * 60), "1h 59m");
        assert_eq!(t(86_400 + 23 * 3600 + 59 * 60), "1d 23h"); // PS [int] rounding said "2d 23h"
    }

    #[test]
    fn memory_formatting() {
        assert_eq!(format_mem(8 << 20), "8 MB");
        assert_eq!(format_mem((1023 << 20) + (900 << 10)), "1,024 MB");
        assert_eq!(format_mem(3 << 30), "3.0 GB");
    }

    #[test]
    fn docker_status_cleanup() {
        let up = DOCKER_PAREN.replace(&DOCKER_UP.replace("Up 6 days (healthy)", ""), "").into_owned();
        assert_eq!(up, "6 days");
    }

    #[test]
    fn docker_image_base_names() {
        assert_eq!(image_base("postgres:16"), "postgres");
        assert_eq!(image_base("supabase/postgres:15.8.1"), "postgres");
        assert_eq!(image_base("localhost:5000/team/api"), "api");
        assert_eq!(image_base("redis"), "redis");
    }

    #[test]
    fn frameworks_from_command_lines() {
        let f = |cmd: &str, exe: &str| detect_framework(cmd, exe, "", None);
        assert_eq!(f(r"node C:\p\node_modules\next\dist\server\lib\start-server.js", "node.exe").as_deref(), Some("Next.js"));
        assert_eq!(f(r"node C:\p\node_modules\vite\bin\vite.js", "node.exe").as_deref(), Some("Vite"));
        assert_eq!(f(r"node C:\p\node_modules\@remotion\cli\remotion-cli.js studio", "node.exe").as_deref(), Some("Remotion"));
        assert_eq!(f(r"node C:\p\node_modules\storybook\bin\index.cjs dev", "node.exe").as_deref(), Some("Storybook"));
        assert_eq!(f("python -m uvicorn app:app", "python.exe").as_deref(), Some("Uvicorn"));
        assert_eq!(f(r"node C:\p\server.js", "node.exe").as_deref(), Some("Node"));
    }
}
