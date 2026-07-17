//! Tauri shell binary (built only with `--features shell`; see Cargo.toml).
//! The headless daemon remains `src/main.rs` / the `whosaidwhat` binary.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    whosaidwhat::shell::run()
}
