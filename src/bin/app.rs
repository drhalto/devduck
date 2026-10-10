// devports-app - the Dev Ports window. Pin this one; `devports` is the CLI.
#![windows_subsystem = "windows"]

fn main() {
    let demo = std::env::args().skip(1).any(|a| a.trim_start_matches(['-', '/']).eq_ignore_ascii_case("demo"));
    // One real window at a time (they share saved state): a second launch focuses the
    // first. Demo and screenshot runs are throwaway and may run alongside.
    let throwaway = demo || std::env::var_os("DEVPORTS_SHOT").is_some();
    if !throwaway && !dev_ports::win::claim_single_instance() {
        dev_ports::win::focus_existing_window();
        return;
    }
    if let Err(e) = dev_ports::gui::run(demo) {
        eprintln!("Dev Ports could not open its window: {e}");
        std::process::exit(1);
    }
}
