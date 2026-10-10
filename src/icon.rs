//! The DevDuck icon as raw RGBA, pre-rendered from the mascot by scripts/make-brand.py
//! (so the app needs no image decoder). The .exe icon is assets/brand/devduck.ico,
//! embedded by build.rs.

/// 64x64 straight-alpha RGBA for the window/taskbar icon.
pub const ICON_64: &[u8] = include_bytes!("../assets/brand/icon-64.rgba");

/// 96x96 straight-alpha RGBA for the logo in the header and sidebar.
pub const LOGO_96: &[u8] = include_bytes!("../assets/brand/logo-96.rgba");
