//! Settings and "recently stopped" projects, persisted to %APPDATA%\DevPorts\state.json.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum Editor {
    #[default]
    VsCode,
    Cursor,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum ViewMode {
    #[default]
    Cards,
    List,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Settings {
    /// Seconds between live refreshes; 0 = paused.
    pub interval_secs: u32,
    pub editor: Editor,
    pub probe_http: bool,
    pub confirm_stop: bool,
    pub view: ViewMode,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { interval_secs: 2, editor: Editor::VsCode, probe_http: true, confirm_stop: true, view: ViewMode::Cards }
    }
}

/// A dev project Dev Ports has seen running, kept so it can be started again.
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
#[serde(default)]
pub struct Recent {
    pub folder: String,
    pub app: String,
    pub framework: Option<String>,
    pub ports: Vec<u16>,
    /// The kill root's command line and working directory when it was last seen.
    pub command: String,
    pub cwd: Option<String>,
    /// Unix seconds.
    pub last_seen: u64,
    /// Set when Dev Ports itself started it: the pid and where its output goes.
    pub launched_pid: Option<u32>,
    pub log: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
#[serde(default)]
pub struct State {
    pub settings: Settings,
    pub recent: Vec<Recent>,
}

pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("DevPorts")
}

fn state_path() -> PathBuf {
    data_dir().join("state.json")
}

impl State {
    pub fn load() -> State {
        std::fs::read_to_string(state_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = state_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    /// Drop projects not seen for two weeks; keep at most 40.
    pub fn prune(&mut self) {
        let cutoff = now_unix().saturating_sub(14 * 86_400);
        self.recent.retain(|r| r.last_seen >= cutoff);
        self.recent.sort_by_key(|r| std::cmp::Reverse(r.last_seen));
        self.recent.truncate(40);
    }

    pub fn recent_mut(&mut self, folder: &str) -> Option<&mut Recent> {
        self.recent.iter_mut().find(|r| r.folder.eq_ignore_ascii_case(folder))
    }
}

/// "just now", "5m ago", "3h ago", "2d ago".
pub fn ago(unix: u64) -> String {
    let secs = now_unix().saturating_sub(unix);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}
