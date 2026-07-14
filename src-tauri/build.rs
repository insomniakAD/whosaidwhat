//! Runs Tauri's build-time codegen (capability schemas under gen/schemas,
//! manifest checks) only for shell builds. Feature flags reach build scripts
//! as CARGO_FEATURE_* env vars — cfg!(feature) is NOT set here, which is why
//! this is an env check and why tauri-build is an unconditional build-dep
//! (a build script cannot reference a crate that wasn't compiled).
//! Evidence: stock create-tauri-app build.rs is `tauri_build::build()`
//! unconditionally; the gating is ours so `cargo test` stays webview-free.

fn main() {
    if std::env::var_os("CARGO_FEATURE_SHELL").is_some() {
        tauri_build::build();
    }
}
