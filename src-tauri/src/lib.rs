//! whosaidwhat — local-first macOS meeting assistant core.
//!
//! Layer map (each module's doc comment carries its design evidence):
//!
//! - [`detect`]  — who's meeting: process/mic/tab signals → debounced events
//! - [`capture`] — two-track audio recording (mic + system), auto-stop
//! - [`asr`]     — whisper.cpp transcription (Metal), in-process
//! - [`diarize`] — sherpa-onnx speaker diarization, in-process; merge logic
//! - [`llm`]     — oMLX routing (OpenAI-compatible) + MapReduce summarization
//! - [`db`]      — SQLite store (schema.sql), FTS5 search, provenance
//! - [`notify`]  — the "start recording?" prompt
//! - [`pipeline`]— glues capture output through ASR+diarization to summary
//! - [`config`]  — user configuration
//!
//! Everything OS-specific is cfg-gated; everything pure is unit-tested on any
//! host (see pipeline/tests and the rustc harness in docs/BUILD_LOG.md D-006).

pub mod asr;
pub mod capture;
pub mod config;
pub mod db;
pub mod detect;
pub mod diarize;
pub mod llm;
pub mod notify;
pub mod pipeline;
