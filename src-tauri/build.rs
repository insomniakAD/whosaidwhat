//! Runs Tauri's build-time codegen (capability schemas under gen/schemas,
//! manifest checks) only for shell builds. Feature flags reach build scripts
//! as CARGO_FEATURE_* env vars — cfg!(feature) is NOT set here, which is why
//! this is an env check and why tauri-build is an unconditional build-dep
//! (a build script cannot reference a crate that wasn't compiled).
//! Evidence: stock create-tauri-app build.rs is `tauri_build::build()`
//! unconditionally; the gating is ours so `cargo test` stays webview-free.

fn main() {
    // The screencapturekit crate links a static Swift shim whose code needs
    // the Swift runtime (libswift_Concurrency et al). Its own build script
    // emits the /usr/lib/swift rpath, but cargo only applies rustc-link-arg
    // to the emitting package's targets — it never reaches our bins/tests,
    // which then die at dyld load. Emit it ourselves.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    if std::env::var_os("CARGO_FEATURE_SHELL").is_some() {
        tauri_build::build();
    }
}
