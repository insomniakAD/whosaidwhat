//! Audio capture: session lifecycle (pure) + macOS backend.

pub mod session;

#[cfg(target_os = "macos")]
pub mod macos;
