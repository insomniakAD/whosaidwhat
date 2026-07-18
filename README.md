# whosaidwhat

A local-first macOS meeting assistant that knows **who said what** — detection,
recording, transcription, speaker diarization, and summarization, all on-device.
Tauri + Rust backend, SQLite storage, oMLX (Apple MLX) for LLM inference.
No cloud. No bots joining your calls. The only socket is `localhost`.

Built autonomously against the master prompt in one run; every decision and its
evidence is logged in [`docs/BUILD_LOG.md`](docs/BUILD_LOG.md).

## The flow

Zoom / Teams / Meet meeting detected → "Start recording?" → two-track capture
(mic + system audio) → auto-stop when the meeting ends → whisper.cpp transcription
(Metal) + sherpa-onnx diarization → speaker-attributed transcript in SQLite (FTS5)
→ 3-stage MapReduce summary via oMLX (Qwen3.6-35B-A3B) → versioned notes.

Every transcript segment carries a millisecond offset, the summarizer preserves
`[mm:ss]` markers, and an extraction stage resolves those markers into
`summary_citations` rows plus a stage-4 pass that fills `action_items` — so the
dashboard's citation chips click through to the exact transcript moment. See
[status](#honest-status) for what remains.

Full diagram: [`docs/00-architecture.md`](docs/00-architecture.md)

## Repository map

| Path | What |
|---|---|
| `src-tauri/` | Rust backend: `detect/` (process watchers, state machine), `capture/` (two-track recording, auto-stop), `asr/` (whisper-rs), `diarize/` (sherpa-onnx + merge), `llm/` (oMLX client, router, summarizer, `extract.rs` citations/actions), `db.rs`, `notify.rs` (window prompt + `un_center`), `pipeline/` (stages + background worker), `shell.rs` (Tauri v2 app), `tauri.conf.json`, `capabilities/` |
| `dist/` | The webview frontend — hand-written static HTML/CSS/JS, no bundler ("Paper & Verdigris", docs/01): dashboard, consent prompt, recording pill |
| `pipeline/` | Runnable Python pipeline: local diarization (pyannote community-1), Apple-Silicon ASR (mlx-whisper), oMLX summarization + extraction; `run.py audio ...` or `run.py meetily` |
| `schema.sql` | The SQLite schema (single source of truth for Rust + Python) |
| `docs/` | Task deliverables 00–05 + build log |
| `reference/meetily_pipeline/` | The original attached scripts, unmodified, for diffing |

## Docs / deliverables

- [`docs/00-architecture.md`](docs/00-architecture.md) — capture → diarization → oMLX flow
- [`docs/01-ui-ux-design.md`](docs/01-ui-ux-design.md) — layout + "Paper & Verdigris" color scheme
- [`docs/02-process-detection.md`](docs/02-process-detection.md) — detection signals + evidence
- [`docs/03-database-schema.md`](docs/03-database-schema.md) — schema research (OpenWhispr, Minutes, Meetily, Screenpipe, Hyprnote)
- [`docs/04-diarization-evaluation.md`](docs/04-diarization-evaluation.md) — pyannote vs WhisperX vs NeMo vs sherpa/FluidAudio, and what the attached pipeline actually did
- [`docs/05-inference-routing.md`](docs/05-inference-routing.md) — oMLX endpoints, Qwen3.6, the Gemma 4 native-audio verdict
- [`docs/BUILD_LOG.md`](docs/BUILD_LOG.md) — every decision, every self-answered question, verification limits

## Running what runs today

**Python pipeline** (macOS, Apple Silicon, with [oMLX](https://omlx.ai) serving on
`localhost:8000`):

```bash
cd pipeline
pip install -r requirements.txt
python3 run.py audio meeting.system.wav --mic meeting.mic.wav --title "Weekly sync"
# or summarize the latest Meetily CE meeting (the original workflow, repaired):
python3 run.py meetily
```

**Tests** (no network, no heavy deps — run anywhere):

```bash
cd pipeline && python3 -m unittest discover tests        # 34 tests
cd src-tauri && cargo test                               # on a normal network
# no network at all? the bare-rustc harness runs the pure core's 50 tests:
rustc --edition 2021 --test src-tauri/harness/harness.rs -o /tmp/wsw && /tmp/wsw
```

**Rust daemon** (macOS): `cd src-tauri && cargo run` — watches for meetings,
records per policy (`config.json`: prompt/auto/manual), pipelines on meeting end.

**Tauri shell** (macOS): `cd src-tauri && cargo build --features shell --bin
whosaidwhat-app` (build.rs runs Tauri codegen only under this feature, so
`cargo test` stays webview-free), or `tauri dev` / `tauri build` with the
`shell` feature enabled. Bundled builds prompt via UNUserNotificationCenter
action buttons; unbundled builds use the always-on-top prompt window.

## Honest status

Authored in a sandbox with a default-deny egress proxy (no crates.io/PyPI, most
sites unfetchable): the pure-logic core is compiled and tested here (50 Rust
tests via the bare-rustc harness, now committed at
`src-tauri/harness/harness.rs`, + 34 Python tests, schema exercised
end-to-end; more rusqlite-backed Rust tests run under `cargo test`); macOS FFI,
Tauri, and third-party-crate surfaces were written from cited evidence with the
first `cargo build` on a Mac flagged as the real type-check. That build has now
happened: the `mac-build` GitHub Actions workflow compiles and tests all of it
on `macos-latest` — core FFI (no default features), the Tauri shell, and the
full whisper.cpp/sherpa-onnx build (67 lib tests green on macOS). The
CI-validated `Cargo.lock` is committed. Details: BUILD_LOG D-006/D-008, the
run-2 appendix, and `.github/workflows/mac-build.yml`.

**Built in run 2** (previously listed here as pending): structured
`summary_citations` + `action_items` extraction in both Rust and Python; the
post-recording pipeline moved to a background worker thread; the Tauri v2 shell
(`tauri.conf.json`, capabilities, `shell.rs`, static `dist/` frontend —
dashboard with citation chips, who-said-what rail, consent prompt window,
recording pill) making `RecordPolicy::Prompt` functional; and the
`un_center` UNUserNotificationCenter path with real action buttons.

**Still not built** (tracked honestly rather than claimed done): audio-player
seek from citation chips (chips jump the transcript; the webview has no
`asset:` protocol grant yet); `.icns` bundle icons (`icons/icon.png` is a
generated placeholder — run `tauri icon` on a Mac); and cross-meeting speaker
re-identification via stored embeddings (schema-ready). The Tauri and
objc2-user-notifications surfaces, previously unverified from this sandbox,
now compile against their real crates in CI; what CI still can't prove is
runtime behavior that needs live meetings, mic/screen-recording permissions,
and an oMLX server — that remains a manual pass on real hardware.
