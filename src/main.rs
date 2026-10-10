// devports - the command-line side of Dev Ports. A console app, so shells wait for it
// and get its exit code; run with no arguments it opens the window (devports-app.exe).

use std::os::windows::process::CommandExt;

use dev_ports::cli;

fn main() {
    // Accept --smoke, -Smoke and /smoke alike (the PowerShell version used -Smoke).
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags: Vec<String> = args.iter().map(|a| a.trim_start_matches(['-', '/']).to_ascii_lowercase()).collect();
    let has = |name: &str| flags.iter().any(|f| f == name);
    let command = args.iter().find(|a| !a.starts_with(['-', '/'])).map(|a| a.to_ascii_lowercase());
    let port = args.iter().filter_map(|a| a.trim_start_matches(':').parse::<u16>().ok()).next();

    let text_mode = has("smoke") || has("json") || has("help") || has("h") || has("?") || command.is_some();
    if !text_mode {
        open_window(has("demo"));
        return;
    }
    let code = match (command.as_deref(), port) {
        _ if has("help") || has("h") || has("?") => {
            cli::print_help();
            0
        }
        (None | Some("list") | Some("ls"), _) => cli::list(has("json")),
        (Some("who"), Some(p)) => cli::who(p),
        (Some("kill") | Some("stop"), Some(p)) => cli::kill(p),
        (Some("open"), Some(p)) => cli::open(p),
        _ => {
            cli::print_help();
            2
        }
    };
    std::process::exit(code);
}

/// Hand off to devports-app.exe (kept beside this exe) so the terminal gets its prompt
/// back straight away. The CLI doesn't link the GUI, which keeps it small and quick.
fn open_window(demo: bool) {
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let app = std::env::current_exe().ok().map(|p| p.with_file_name("devports-app.exe"));
    let Some(app) = app.filter(|p| p.is_file()) else {
        eprintln!("devports-app.exe isn't next to devports.exe; run `devports --help` for the text commands.");
        std::process::exit(1);
    };
    let mut cmd = std::process::Command::new(app);
    if demo {
        cmd.arg("--demo");
    }
    if let Err(e) = cmd.creation_flags(DETACHED_PROCESS).spawn() {
        eprintln!("Couldn't open the Dev Ports window: {e}");
        std::process::exit(1);
    }
}
