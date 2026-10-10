//! Doing things to processes: stop (whole tree), start a remembered dev server with its
//! output captured to a log, Docker stop/restart/logs, and opening editors/terminals.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, UdpSocket};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::store::{self, Editor};
use crate::win::{self, CREATE_NO_WINDOW};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// `taskkill /T /F` on the root: the dev server and every child it spawned.
pub fn kill_tree(pid: u32) -> Result<(), String> {
    if pid <= 4 {
        return Err("That is a Windows kernel listener and can't be stopped.".into());
    }
    let out = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Couldn't run taskkill: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stderr).trim(), String::from_utf8_lossy(&out.stdout).trim());
    Err(format!("taskkill failed:\n{}\n\nTry running Dev Ports as administrator.", text.trim()))
}

pub fn docker(args: &[&str]) -> Result<String, String> {
    let out = Command::new("docker")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Couldn't run docker: {e}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text) } else { Err(text.trim().to_string()) }
}

/// Wait (up to `timeout`) until none of `ports` has a listener, so a restart can rebind.
pub fn wait_ports_free(ports: &[u16], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let listening = win::tcp_listeners();
        if !listening.iter().any(|l| ports.contains(&l.port)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Split a Windows command line into the program and the raw rest of the line.
fn split_command(command: &str) -> Option<(String, String)> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    if let Some(rest) = command.strip_prefix('"') {
        let end = rest.find('"')?;
        Some((rest[..end].to_string(), rest[end + 1..].trim_start().to_string()))
    } else {
        let end = command.find(char::is_whitespace).unwrap_or(command.len());
        Some((command[..end].to_string(), command[end..].trim_start().to_string()))
    }
}

pub fn log_path(app: &str) -> PathBuf {
    let safe: String = app.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    store::data_dir().join("logs").join(format!("{safe}.log"))
}

/// Re-run a dev server's root command in its working directory, detached and windowless,
/// with stdout and stderr going to a log file Dev Ports can tail. Returns the new pid.
pub fn start(app: &str, command: &str, cwd: &str) -> Result<(u32, PathBuf), String> {
    let (program, args) = split_command(command).ok_or("No command recorded for this project.")?;
    if !Path::new(cwd).is_dir() {
        return Err(format!("The folder {cwd} no longer exists."));
    }
    let log = log_path(app);
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut file = File::create(&log).map_err(|e| format!("Couldn't create {}: {e}", log.display()))?;
    let _ = writeln!(file, "[dev ports] {} in {cwd}\n[dev ports] {command}\n", win::local_clock());
    drop(file);
    let out = OpenOptions::new().append(true).open(&log).map_err(|e| e.to_string())?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&program);
    if !args.is_empty() {
        cmd.raw_arg(&args);
    }
    let child = cmd
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0")
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map_err(|e| format!("Couldn't start {program}: {e}"))?;
    Ok((child.id(), log))
}

static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[()][A-Z0-9]").unwrap());

/// Terminal output made readable: ANSI escapes removed, carriage-return progress
/// lines collapsed to their final state.
pub fn clean_log(raw: &str) -> String {
    let stripped = ANSI.replace_all(raw, "");
    stripped
        .split('\n')
        .map(|line| line.trim_end_matches('\r').rsplit('\r').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The last `max` bytes of a log file, cleaned, starting at a line boundary.
pub fn tail(path: &Path, max: u64) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(max);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let text = if start > 0 { text.split_once('\n').map_or("", |(_, rest)| rest).to_string() } else { text.into_owned() };
    Some(clean_log(&text))
}

fn editor_exe(editor: Editor) -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| r"C:\Program Files".into());
    let candidates: Vec<PathBuf> = match editor {
        Editor::VsCode => vec![
            local.join(r"Programs\Microsoft VS Code\Code.exe"),
            program_files.join(r"Microsoft VS Code\Code.exe"),
        ],
        Editor::Cursor => vec![local.join(r"Programs\cursor\Cursor.exe"), local.join(r"Programs\Cursor\Cursor.exe")],
    };
    candidates.into_iter().find(|p| p.is_file())
}

pub fn editor_installed(editor: Editor) -> bool {
    editor_exe(editor).is_some()
}

pub fn editor_name(editor: Editor) -> &'static str {
    match editor {
        Editor::VsCode => "VS Code",
        Editor::Cursor => "Cursor",
    }
}

pub fn open_in_editor(editor: Editor, folder: &str) -> Result<(), String> {
    match editor_exe(editor) {
        Some(exe) => Command::new(exe).arg(folder).spawn().map(drop).map_err(|e| e.to_string()),
        None => {
            let cli = if editor == Editor::VsCode { "code" } else { "cursor" };
            Command::new("cmd")
                .args(["/c", cli, folder])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map(drop)
                .map_err(|_| format!("{} isn't installed.", editor_name(editor)))
        }
    }
}

/// Windows Terminal in the folder if it's installed, else a plain console.
pub fn open_terminal(folder: &str) -> Result<(), String> {
    let wt = std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join(r"Microsoft\WindowsApps\wt.exe"));
    if let Some(wt) = wt.filter(|p| p.exists()) {
        return Command::new(wt).args(["-d", folder]).spawn().map(drop).map_err(|e| e.to_string());
    }
    Command::new("cmd").args(["/c", "start", "", "/D", folder, "cmd"]).creation_flags(CREATE_NO_WINDOW).spawn().map(drop).map_err(|e| e.to_string())
}

/// This machine's address on the local network (the interface with the default route).
pub fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?; // TEST-NET-1: no packet is sent for a UDP connect
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_and_bare_programs() {
        assert_eq!(
            split_command(r#""C:\Program Files\nodejs\node.exe" "C:\x\npm-cli.js" run dev"#),
            Some((r"C:\Program Files\nodejs\node.exe".into(), r#""C:\x\npm-cli.js" run dev"#.into()))
        );
        assert_eq!(split_command(r"C:\WINDOWS\system32\cmd.exe /d /s /c vite"), Some((r"C:\WINDOWS\system32\cmd.exe".into(), "/d /s /c vite".into())));
        assert_eq!(split_command("python"), Some(("python".into(), String::new())));
        assert_eq!(split_command("   "), None);
    }

    #[test]
    fn cleans_terminal_output() {
        let raw = "\x1b[32m✓\x1b[39m Ready in 1.2s\r\nCompiling 10%\rCompiling 100%\n";
        assert_eq!(clean_log(raw), "✓ Ready in 1.2s\nCompiling 100%\n");
    }
}
