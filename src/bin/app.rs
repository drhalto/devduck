// devduck-app - the DevDuck window. Pin this one; `devduck` is the CLI.
#![windows_subsystem = "windows"]

fn main() {
    let demo = std::env::args().skip(1).any(|a| a.trim_start_matches(['-', '/']).eq_ignore_ascii_case("demo"));
    // One real window at a time (they share saved state): a second launch focuses the
    // first. Demo and screenshot runs are throwaway and may run alongside.
    let throwaway = demo || std::env::var_os("DEVDUCK_SHOT").is_some();
    if !throwaway && !devduck::win::claim_single_instance() {
        devduck::win::focus_existing_window();
        return;
    }
    if let Err(e) = devduck::gui::run(demo) {
        eprintln!("DevDuck could not open its window: {e}");
        std::process::exit(1);
    }
}
