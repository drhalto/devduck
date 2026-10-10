// devports-app - the Dev Ports window. Pin this one; `devports` is the CLI.
#![windows_subsystem = "windows"]

fn main() {
    let demo = std::env::args().skip(1).any(|a| a.trim_start_matches(['-', '/']).eq_ignore_ascii_case("demo"));
    if let Err(e) = dev_ports::gui::run(demo) {
        eprintln!("Dev Ports could not open its window: {e}");
        std::process::exit(1);
    }
}
