//! VectorCraft desktop app entry point.
//!
//! Usage: `vectorcraft [--control <port> [--automation-read-root <dir>] [--automation-write-root <dir>]]
//! [--in-window-menus] [files…]`, `vectorcraft --version`, `vectorcraft --help` (or `-h`)
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    vectorcraft::run_desktop()
}
