//! DevDuck: see what's listening on your local ports, grouped by project, and manage
//! the dev servers behind them. Two binaries share this library: `devduck` (the
//! console CLI) and `devduck-app` (the window).

pub mod cli;
pub mod gui;
pub mod icon;
pub mod launch;
pub mod probe;
pub mod scan;
pub mod store;
pub mod win;
