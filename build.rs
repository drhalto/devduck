// Embeds the app icon (drawn in src/icon.rs) and version info into devports.exe,
// so Explorer and a pinned taskbar shortcut show it, not just the running window.
#[path = "src/icon.rs"]
mod icon;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ico = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("devports.ico");
    std::fs::write(&ico, icon::ico(&[16, 20, 24, 32, 40, 48, 64, 256])).unwrap();

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().unwrap())
        .set("FileDescription", "Dev Ports")
        .set("ProductName", "Dev Ports");
    if let Err(e) = res.compile() {
        // No rc.exe (Windows SDK) on this machine: the build still works, the exe just has no icon.
        println!("cargo:warning=exe icon not embedded: {e}");
    }
}
