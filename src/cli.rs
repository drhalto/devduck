//! Terminal commands: list (table or JSON), who/kill/open by port.

use std::io::Write;
use std::time::Instant;

use crate::launch;
use crate::scan::{Kind, PortApp, Scanner};
use crate::win;

pub const HELP: &str = "\
Dev Ports - see what's listening on your local ports, grouped by project.

Usage:
  devports                 open the window
  devports list            print every listener as a table (alias: --smoke)
  devports list --json     ...as JSON (alias: --json)
  devports who <port>      what's on a port: app, folder, process tree, command
  devports kill <port>     stop whatever owns a port (whole process tree, or docker stop)
  devports open <port>     open http://localhost:<port> in the browser
  devports --demo          open the window with made-up rows (for screenshots)
";

fn out(text: &str) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(text.as_bytes());
    let _ = o.flush();
}

pub fn print_help() {
    out(HELP);
}

pub fn list(json: bool) -> i32 {
    let started = Instant::now();
    let rows = Scanner::new().scan(true);
    let secs = started.elapsed().as_secs_f64();
    out(&if json { to_json(&rows) } else { to_table(&rows, secs) });
    0
}

fn find(port: u16) -> Option<PortApp> {
    Scanner::new().scan(true).into_iter().find(|r| r.ports.iter().any(|p| p.port == port))
}

pub fn who(port: u16) -> i32 {
    let Some(r) = find(port) else {
        out(&format!("Nothing is listening on :{port}\n"));
        return 1;
    };
    let mut s = format!("\n{}  ({})\n", r.app, r.framework.as_deref().unwrap_or(r.kind.as_str()));
    let ports: Vec<String> = r.ports.iter().map(|p| format!(":{} [{}]", p.port, p.addrs_text())).collect();
    s += &format!("  ports    {}\n", ports.join("  "));
    if !r.folder.is_empty() {
        s += &format!("  folder   {}\n", r.folder);
    }
    s += &format!("  process  {}   up {}   {}\n", r.process, r.uptime, r.mem);
    if r.tree.len() > 1 {
        s += &format!("  tree     {} processes\n", r.tree.len());
        for t in r.tree.iter().take(25) {
            s += &format!("           {}{} ({})\n", "  ".repeat(t.depth), t.name, t.pid);
        }
    }
    let cmd = if r.root_cmd.is_empty() { &r.cmd } else { &r.root_cmd };
    s += &format!("  command  {cmd}\n");
    if let Some(cwd) = &r.cwd {
        s += &format!("  cwd      {cwd}\n");
    }
    out(&(s + "\n"));
    0
}

pub fn kill(port: u16) -> i32 {
    let Some(r) = find(port) else {
        out(&format!("Nothing is listening on :{port}\n"));
        return 1;
    };
    if r.kind == Kind::System {
        out(&format!(":{port} belongs to {} (a Windows system process). Not stopping it.\n", r.process));
        return 2;
    }
    let result = if r.kind == Kind::Docker {
        launch::docker(&["stop", "-t", "2", &r.docker_name]).map(drop)
    } else {
        launch::kill_tree(r.kill_pid)
    };
    match result {
        Ok(()) => {
            let what = if r.kind == Kind::Docker { format!("container {}", r.docker_name) } else { format!("{} ({} processes)", r.process, r.tree.len().max(1)) };
            out(&format!("Stopped {} - {what}; freed {}\n", r.app, r.ports_text));
            0
        }
        Err(e) => {
            out(&format!("Couldn't stop {}: {e}\n", r.app));
            1
        }
    }
}

pub fn open(port: u16) -> i32 {
    if win::shell_open(&format!("http://localhost:{port}")) { 0 } else { 1 }
}

fn to_table(rows: &[PortApp], secs: f64) -> String {
    let headers = ["App", "Ports", "Process", "Kind", "Uptime", "Mem", "Folder"];
    let cells: Vec<[&str; 7]> = rows
        .iter()
        .map(|r| [r.app.as_str(), &r.ports_text, &r.process, r.kind.as_str(), &r.uptime, &r.mem, &r.folder])
        .collect();
    let mut widths = headers.map(|h| h.chars().count());
    for row in &cells {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let line = |values: [&str; 7]| {
        let mut s = String::new();
        for (i, (v, w)) in values.iter().zip(widths).enumerate() {
            s.push_str(v);
            if i < 6 {
                s.extend(std::iter::repeat_n(' ', w - v.chars().count() + 1));
            }
        }
        s.trim_end().to_string() + "\n"
    };
    let mut out = String::from("\n");
    out += &line(headers);
    out += &line(widths.map(|w| "-".repeat(w)).each_ref().map(String::as_str));
    for row in cells {
        out += &line(row);
    }
    out += &format!("\n{} rows in {secs:.2}s\n", rows.len());
    out
}

fn to_json(rows: &[PortApp]) -> String {
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "app": r.app,
                "ports": r.ports.iter().map(|p| p.port).collect::<Vec<_>>(),
                "bind": r.ports.iter().map(|p| serde_json::json!({ "port": p.port, "addrs": p.addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>() })).collect::<Vec<_>>(),
                "kind": r.kind.as_str(),
                "framework": r.framework,
                "process": r.process,
                "pid": if r.kill_pid == 0 { None } else { Some(r.kill_pid) },
                "docker": if r.docker_name.is_empty() { None } else { Some(&r.docker_name) },
                "uptime": r.uptime,
                "mem": r.mem,
                "mem_bytes": r.mem_bytes,
                "processes": r.tree.len(),
                "folder": r.folder,
                "cwd": r.cwd,
                "cmd": r.cmd,
                "root_cmd": r.root_cmd,
            })
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".into()) + "\n"
}
