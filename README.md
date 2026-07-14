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

Every transcript segment carries a millisecond offset, and the summarizer is
prompted to preserve `[mm:ss]` markers, so notes can point back into the audio.
The dedicated `summary_citations` / `action_items` tables that turn those markers
into structured deep-links are schema-defined and ready, but not yet populated —
see [status](#honest-status).

Full diagram: [`docs/00-architecture.md`](docs/00-architecture.md)

## Repository map

| Path | What |
|---|---|
| `src-tauri/` | Rust backend: `detect/` (process watchers, state machine), `capture/` (two-track recording, auto-stop), `asr/` (whisper-rs), `diarize/` (sherpa-onnx + merge), `llm/` (oMLX client, router, summarizer), `db.rs`, `notify.rs`, `pipeline.rs` |
| `pipeline/` | Runnable Python pipeline: local diarization (pyannote community-1), Apple-Silicon ASR (mlx-whisper), oMLX summarization; `run.py audio ...` or `run.py meetily` |
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
cd pipeline && python3 -m unittest discover tests        # 18 tests
cd src-tauri && cargo test                               # on a normal network
```

**Rust daemon** (macOS): `cd src-tauri && cargo run` — watches for meetings,
records per policy (`config.json`: prompt/auto/manual), pipelines on meeting end.

## Honest status

Authored in a sandbox with a default-deny egress proxy (no crates.io/PyPI, most
sites unfetchable): the pure-logic core is compiled and tested here (30 Rust +
21 Python tests, schema exercised end-to-end; 4 more rusqlite-backed Rust tests
run under `cargo test`); macOS FFI and third-party-crate
surfaces are written from cited evidence and flagged in-file where the first
`cargo build` on a Mac is the type-check. Details: BUILD_LOG D-006/D-008.

**What is not yet built** (schema/traits ready, code pending — tracked honestly
rather than claimed done): structured `summary_citations` + `action_items`
extraction (the summarizer preserves `[mm:ss]` markers inline today); the Tauri
frontend shell (the notification prompt, recording pill, and dashboard from
docs/01 are designed and the backend traits exist, but no `tauri.conf.json` /
webview is in this repo yet); the `un_center` bundled-build notification path;
and moving the post-recording pipeline off the detection thread. The headless
daemon runs the full detect → capture → transcribe → diarize → summarize → store
loop under `RecordPolicy::Auto`.
