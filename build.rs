// Embeds the DevDuck icon (assets/brand/devduck.ico, made by scripts/make-brand.py) and
// version info into both exes, so Explorer and a pinned taskbar shortcut show the duck.

fn main() {
    println!("cargo:rerun-if-changed=assets/brand/devduck.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/brand/devduck.ico").set("FileDescription", "DevDuck").set("ProductName", "DevDuck");
    if let Err(e) = res.compile() {
        // No rc.exe (Windows SDK) on this machine: the build still works, the exe just has no icon.
        println!("cargo:warning=exe icon not embedded: {e}");
    }
}
