//! The projects on this PC that DevDuck can start, found by walking a few folders
//! (your home folder by default) for `package.json`, `deno.json` and `manage.py`.
//!
//! A project is listed when it has something that looks like a server to run (`dev`,
//! `start`, `serve`, `preview`, `storybook`...). Discovery only reads folder listings and
//! those small files; it never runs anything.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One runnable thing in a project: an npm script, a deno task, `manage.py runserver`.
#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    /// "dev", "start", "storybook", "runserver".
    pub name: String,
    /// What DevDuck runs, through `cmd /c`, in the project folder: "pnpm run dev".
    pub command: String,
    /// What the script itself does, for display: "next dev --turbopack".
    pub detail: String,
}

impl Script {
    /// Whether this looks like something that serves (vs. build, test, lint).
    pub fn serves(&self) -> bool {
        serves(&self.name)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub folder: String,
    pub name: String,
    pub framework: Option<String>,
    /// Serving scripts first, in the order a person would reach for them.
    pub scripts: Vec<Script>,
}

impl Project {
    pub fn script(&self, name: &str) -> Option<&Script> {
        self.scripts.iter().find(|s| s.name == name)
    }

    /// What Start runs when you haven't picked: the first serving script.
    pub fn default_script(&self) -> Option<&Script> {
        self.scripts.iter().find(|s| s.serves())
    }
}

const SERVE_ORDER: [&str; 9] = ["dev", "start", "serve", "develop", "preview", "storybook", "studio", "watch", "runserver"];

fn serves(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let head = n.split([':', '-', '_']).next().unwrap_or(&n);
    SERVE_ORDER.contains(&head) || n.starts_with("dev") || n.ends_with(":dev") || n.ends_with("-dev")
}

fn serve_rank(name: &str) -> usize {
    let n = name.to_ascii_lowercase();
    SERVE_ORDER.iter().position(|s| *s == n).unwrap_or(SERVE_ORDER.len() + usize::from(!serves(name)))
}

/// Folders never worth descending into.
fn skip_dir(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with('.')
        || n.starts_with('$')
        || matches!(
            n.as_str(),
            "node_modules" | "appdata" | "target" | "dist" | "build" | "out" | "venv" | "env" | "__pycache__"
                | "vendor" | "bin" | "obj" | "downloads" | "music" | "pictures" | "videos" | "saved games"
                | "searches" | "links" | "contacts" | "favorites" | "scoop" | "go" | "miniconda3" | "anaconda3"
                | "onedrive" | "site-packages" | "packages" | "coverage" | "temp" | "tmp" | "cache"
        )
}

/// How deep below each root to look. Home/Project is 1, Home/Tools/Project is 2.
pub const DEPTH: usize = 3;
/// Stop after this many folder listings, so a huge drive can't stall discovery.
const MAX_DIRS: usize = 8000;

pub fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

/// Where to look when you haven't said: your home folder.
pub fn default_roots() -> Vec<String> {
    home().map(|h| vec![h.to_string_lossy().into_owned()]).unwrap_or_default()
}

/// Every project under `roots`, sorted by name. A folder that is a project isn't
/// searched further (its `apps\web` is reached through its own scripts).
pub fn discover(roots: &[String]) -> Vec<Project> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut budget = MAX_DIRS;
    for root in roots {
        walk(Path::new(root), 0, &mut out, &mut seen, &mut budget);
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<Project>, seen: &mut BTreeSet<String>, budget: &mut usize) {
    if *budget == 0 {
        return;
    }
    if depth > 0
        && let Some(p) = inspect(dir)
    {
        if seen.insert(p.folder.to_lowercase()) {
            out.push(p);
        }
        return;
    }
    if depth >= DEPTH {
        return;
    }
    *budget -= 1;
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| !skip_dir(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for d in dirs {
        walk(&d, depth + 1, out, seen, budget);
    }
}

/// The project in `folder`, if it has something to run.
pub fn inspect(folder: &Path) -> Option<Project> {
    let name = folder.file_name()?.to_string_lossy().into_owned();
    let folder_s = folder.to_string_lossy().into_owned();
    let mut scripts = Vec::new();
    let mut framework = None;

    if let Some(json) = read_json(&folder.join("package.json")) {
        let pm = package_manager(folder);
        if let Some(obj) = json.get("scripts").and_then(|v| v.as_object()) {
            for (k, v) in obj {
                let Some(detail) = v.as_str() else { continue };
                // npm runs pre/post hooks itself.
                if (k.starts_with("pre") || k.starts_with("post")) && obj.contains_key(k.trim_start_matches("pre").trim_start_matches("post")) {
                    continue;
                }
                scripts.push(Script { name: k.clone(), command: run_command(pm, k), detail: detail.to_string() });
            }
        }
        let mut deps = BTreeSet::new();
        for section in ["dependencies", "devDependencies"] {
            if let Some(obj) = json.get(section).and_then(|v| v.as_object()) {
                deps.extend(obj.keys().cloned());
            }
        }
        let cmds: String = scripts.iter().filter(|s| s.serves()).map(|s| format!("{} ", s.detail)).collect();
        framework = crate::scan::detect_framework(&cmds, "", &folder_s, Some(&deps));
    } else if let Some(json) = read_json(&folder.join("deno.json")).or_else(|| read_json(&folder.join("deno.jsonc"))) {
        if let Some(obj) = json.get("tasks").and_then(|v| v.as_object()) {
            for (k, v) in obj {
                let detail = v.as_str().map(str::to_string).or_else(|| v.get("command").and_then(|c| c.as_str()).map(str::to_string)).unwrap_or_default();
                scripts.push(Script { name: k.clone(), command: format!("deno task {k}"), detail });
            }
        }
        framework = Some("Deno".into());
    } else if folder.join("manage.py").is_file() {
        let python = venv_python(folder).unwrap_or_else(|| "python".into());
        scripts.push(Script { name: "runserver".into(), command: format!("{python} manage.py runserver"), detail: "python manage.py runserver".into() });
        framework = Some("Django".into());
    }

    if !scripts.iter().any(Script::serves) {
        return None;
    }
    scripts.sort_by_key(|s| serve_rank(&s.name));
    Some(Project { folder: folder_s, name, framework, scripts })
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// npm, pnpm, yarn or bun, from the nearest lockfile (looking up a few levels, for
/// workspaces) or the `packageManager` field.
pub fn package_manager(folder: &Path) -> &'static str {
    let mut dir = Some(folder);
    for _ in 0..4 {
        let Some(d) = dir else { break };
        for (file, pm) in [("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"), ("bun.lockb", "bun"), ("bun.lock", "bun"), ("package-lock.json", "npm")] {
            if d.join(file).is_file() {
                return pm;
            }
        }
        if let Some(pm) = read_json(&d.join("package.json")).and_then(|j| j.get("packageManager").and_then(|v| v.as_str()).map(str::to_string)) {
            for name in ["pnpm", "yarn", "bun", "npm"] {
                if pm.starts_with(name) {
                    return name;
                }
            }
        }
        dir = d.parent();
    }
    "npm"
}

fn run_command(pm: &str, script: &str) -> String {
    let quoted = if script.contains([' ', '&', '|', '<', '>', '^']) { format!("\"{script}\"") } else { script.to_string() };
    match pm {
        "yarn" => format!("yarn {quoted}"),
        other => format!("{other} run {quoted}"),
    }
}

fn venv_python(folder: &Path) -> Option<String> {
    for v in [".venv", "venv", "env"] {
        let p = folder.join(v).join("Scripts").join("python.exe");
        if p.is_file() {
            return Some(format!("\"{}\"", p.display()));
        }
    }
    None
}

/// The project's own icon, if it has one: big raster icons first, favicons last. The
/// favicon create-next-app ships (25,931 bytes) is a placeholder, not the project's.
pub fn find_icon(folder: &Path) -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "app/icon.png",
        "src/app/icon.png",
        "app/apple-icon.png",
        "src/app/apple-icon.png",
        "public/apple-touch-icon.png",
        "public/icon.png",
        "public/logo.png",
        "public/android-chrome-192x192.png",
        "static/apple-touch-icon.png",
        "static/favicon.png",
        "public/favicon.png",
        "public/favicon-32x32.png",
        "app/favicon.ico",
        "src/app/favicon.ico",
        "public/favicon.ico",
        "static/favicon.ico",
        "favicon.ico",
    ];
    CANDIDATES.iter().map(|c| folder.join(c)).find(|p| {
        p.metadata().is_ok_and(|m| m.is_file() && m.len() > 0 && m.len() < 2_000_000 && !(m.len() == 25_931 && p.extension().is_some_and(|e| e == "ico")))
    })
}

/// `command` (a script run like "npm run dev") asked to serve on `port`. Frameworks that
/// take `--port` get the flag (npm needs `--` to pass it through); Django gets the port as
/// runserver's argument. Everything also gets PORT in its environment, which Express-style
/// servers read.
pub fn with_port(command: &str, framework: Option<&str>, port: u16) -> String {
    let cmd = command.trim_end();
    match framework {
        Some("Django") => format!("{cmd} {port}"),
        Some(
            "Next.js" | "Vite" | "SvelteKit" | "Astro" | "Nuxt" | "Angular" | "Storybook" | "Remotion" | "Wrangler" | "Expo" | "Remix" | "React Router" | "Uvicorn"
            | "Flask",
        ) => {
            let pass = if cmd.starts_with("npm ") && !cmd.contains(" -- ") { " --" } else { "" };
            format!("{cmd}{pass} --port {port}")
        }
        _ => cmd.to_string(),
    }
}

/// The port a framework's dev server binds unless told otherwise.
pub fn framework_port(framework: &str) -> Option<u16> {
    Some(match framework {
        "Next.js" | "Remix" | "React Router" | "Express" | "Remotion" => 3000,
        "Vite" | "SvelteKit" => 5173,
        "Astro" => 4321,
        "Nuxt" => 3000,
        "Angular" => 4200,
        "Storybook" => 6006,
        "Django" => 8000,
        "Uvicorn" => 8000,
        "Flask" => 5000,
        "Wrangler" => 8787,
        "Expo" => 8081,
        "Jupyter" => 8888,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("devduck-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn port_flags() {
        assert_eq!(with_port("npm run dev", Some("Next.js"), 3001), "npm run dev -- --port 3001");
        assert_eq!(with_port("pnpm run dev", Some("Vite"), 5174), "pnpm run dev --port 5174");
        assert_eq!(with_port("python manage.py runserver", Some("Django"), 8001), "python manage.py runserver 8001");
        assert_eq!(with_port("npm run start", Some("Express"), 3001), "npm run start");
    }

    #[test]
    fn serving_scripts() {
        for s in ["dev", "start", "dev:web", "storybook", "web:dev", "preview", "serve"] {
            assert!(serves(s), "{s}");
        }
        for s in ["build", "test", "lint", "typecheck", "format"] {
            assert!(!serves(s), "{s}");
        }
    }

    #[test]
    fn finds_node_projects_with_their_package_manager() {
        let root = tmp("node");
        let app = root.join("work").join("shop");
        std::fs::create_dir_all(app.join("node_modules").join("next")).unwrap();
        std::fs::write(
            app.join("package.json"),
            r#"{"scripts":{"build":"next build","predev":"echo hi","dev":"next dev","lint":"eslint ."},"dependencies":{"next":"16"}}"#,
        )
        .unwrap();
        std::fs::write(app.join("pnpm-lock.yaml"), "").unwrap();
        // A library with nothing to serve isn't listed.
        let lib = root.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("package.json"), r#"{"scripts":{"build":"tsc","test":"vitest"}}"#).unwrap();

        let found = discover(&[root.to_string_lossy().into_owned()]);
        assert_eq!(found.len(), 1);
        let p = &found[0];
        assert_eq!(p.name, "shop");
        assert_eq!(p.framework.as_deref(), Some("Next.js"));
        assert_eq!(p.scripts[0].name, "dev");
        assert_eq!(p.scripts[0].command, "pnpm run dev");
        assert!(p.script("predev").is_none());
        assert_eq!(p.default_script().map(|s| s.name.as_str()), Some("dev"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_django() {
        let root = tmp("django");
        let site = root.join("site");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(site.join("manage.py"), "").unwrap();
        let found = discover(&[root.to_string_lossy().into_owned()]);
        assert_eq!(found[0].framework.as_deref(), Some("Django"));
        assert_eq!(found[0].scripts[0].command, "python manage.py runserver");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn yarn_scripts_and_quoting() {
        assert_eq!(run_command("yarn", "dev"), "yarn dev");
        assert_eq!(run_command("npm", "dev web"), "npm run \"dev web\"");
    }
}

#[cfg(test)]
mod machine {
    /// `cargo test -- --ignored --nocapture discover_home`: how discovery does on this PC.
    #[test]
    #[ignore]
    fn discover_home() {
        let t = std::time::Instant::now();
        let found = super::discover(&super::default_roots());
        println!("{} projects in {:?}", found.len(), t.elapsed());
        for p in found.iter().take(60) {
            println!("  {:<28} {:<14} {}", p.name, p.framework.as_deref().unwrap_or("-"), p.scripts.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(" "));
        }
    }
}
