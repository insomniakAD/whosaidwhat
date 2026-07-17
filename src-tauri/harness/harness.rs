//! Bare-rustc test harness (BUILD_LOG D-006): compiles the dependency-free
//! core modules straight from src/ with no cargo and no network, so the pure
//! logic is testable even where crates.io is unreachable:
//!
//! ```sh
//! rustc --edition 2021 --test src-tauri/harness/harness.rs -o /tmp/wsw-harness \
//!   && /tmp/wsw-harness
//! ```
//!
//! asr/diarize are shimmed to their pure data types below (the real modules
//! carry thiserror-derived error enums, which needs cargo). Everything else
//! is included from the real source files — the tests that run here are the
//! same #[cfg(test)] blocks `cargo test` runs.

#[path = "../src/detect/mod.rs"]
pub mod detect;

// #[path] on the inline module sets the base DIRECTORY for its children —
// required here because a child path like "../../src/…" would traverse
// through a harness/llm/ directory that doesn't exist (POSIX resolves every
// component, even before a `..`).
#[path = "../src/llm"]
pub mod llm {
    #[path = "chunk.rs"]
    pub mod chunk;
    #[path = "extract.rs"]
    pub mod extract;
}

#[path = "../src/capture"]
pub mod capture {
    #[path = "session.rs"]
    pub mod session;
}

#[path = "../src/pipeline"]
pub mod pipeline {
    #[path = "worker.rs"]
    pub mod worker;
}

#[path = "../src/notify.rs"]
pub mod notify;

// Shims: the pure data types merge.rs consumes (real defs in asr/mod.rs and
// diarize/mod.rs, minus their thiserror error enums).
pub mod asr {
    #[derive(Debug, Clone, PartialEq)]
    pub struct AsrSegment {
        pub start_ms: u64,
        pub end_ms: u64,
        pub text: String,
    }
}

#[path = "../src/diarize"]
pub mod diarize {
    #[derive(Debug, Clone, PartialEq)]
    pub struct SpeakerSegment {
        pub start_ms: u64,
        pub end_ms: u64,
        pub speaker: String,
    }
    #[path = "merge.rs"]
    pub mod merge;
}

// Cross-implementation edge cases mirrored from pipeline/tests + the Python
// spot-checks: the same adversarial strings must produce the same values in
// the Rust and Python marker parsers (llm/extract.rs ↔ wsw/extract.py).
#[cfg(test)]
mod twin_parity {
    use crate::llm::extract::parse_timestamps;

    #[test]
    fn adversarial_edges_match_python() {
        assert_eq!(parse_timestamps("[[01:05]"), vec![65_000]);
        assert_eq!(parse_timestamps("[01:05][01:06]"), vec![65_000, 66_000]);
        assert_eq!(parse_timestamps("a [0:05] b"), vec![5_000]);
        assert_eq!(parse_timestamps("[1:05]"), vec![65_000]);
        assert_eq!(
            parse_timestamps("[123:45:12] x"),
            vec![(123u64 * 3600 + 45 * 60 + 12) * 1000]
        );
        assert!(parse_timestamps("[01:05:99]").is_empty());
        assert!(parse_timestamps("text [99999:00]").is_empty());
    }
}
