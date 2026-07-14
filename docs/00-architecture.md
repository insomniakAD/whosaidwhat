# whosaidwhat — Architecture

The flow the Output Requirements ask for: **audio capture → diarization engine →
oMLX summarization endpoint**, with everything on-device.

## 1. The whole system on one page

```
                        macOS
┌─────────────────────────────────────────────────────────────────────────┐
│  DETECT (src/detect)                          always-on, ~0 CPU at idle │
│  NSWorkspace launch/quit events ──gate──► poll loop (2s active/15s idle)│
│    signals per app:  Zoom: CptHost process (exact match)                │
│                      Teams: MSTeams + mic-in-use                        │
│                      Meet: browser + meet.google.com/<code> tab + mic   │
│    mic ground truth: CoreAudio DeviceIsRunningSomewhere                 │
│                      (per-process variant while self-recording)         │
│    → AppDetector state machine (debounced) → MeetingStarted/Ended       │
├─────────────────────────────────────────────────────────────────────────┤
│  PROMPT (src/notify)                                                    │
│  MeetingStarted ─► "Start recording?" ─ pill window (dev) /             │
│                     UNUserNotificationCenter buttons (bundled)          │
├─────────────────────────────────────────────────────────────────────────┤
│  CAPTURE (src/capture)                       two tracks, 48 kHz f32 WAV │
│  mic ──── cpal ─────────────► {stem}.mic.wav     (= the local user)     │
│  system ─ ScreenCaptureKit ─► {stem}.system.wav  (= everyone remote)    │
│  MeetingEnded ─► auto-stop: stop streams, finalize WAVs, emit           │
│                  RecordingSaved ──────────────┐                         │
├───────────────────────────────────────────────▼─────────────────────────┤
│  PIPELINE (src/pipeline.rs)                    post-meeting batch       │
│   both tracks → resample 16 kHz mono                                    │
│   system.wav ─┬─► whisper.cpp (Metal, in-process) ── AsrSegments        │
│               └─► sherpa-onnx (ONNX, in-process) ─── SpeakerSegments    │
│                     └─ attribute_speakers (max-overlap) + coalesce      │
│   mic.wav ───────► whisper.cpp ── turns pre-labeled "Me"                │
│   interleave(local, remote) ──► one chronological transcript            │
├─────────────────────────────────────────────────────────────────────────┤
│  STORE (src/db.rs + schema.sql)                                         │
│   segments (ms offsets, speaker FK, mic/system source) + FTS5 index     │
│   meetings.status: recorded → transcribed → summarized                  │
├─────────────────────────────────────────────────────────────────────────┤
│  SUMMARIZE (src/llm)                          the only network hop —    │
│   chunk (1200 words, 2-turn overlap)          and it's localhost        │
│   stage 1 extract (strict) ─ per chunk ─► oMLX POST /v1/chat/completions│
│   stage 2 outline (strict) ──────────────► Qwen3.6-35B-A3B-oQ4e-mtp     │
│   stage 3 rewrite (prose) ───────────────► (or router fallback model)   │
│   → summaries (versioned, model provenance) + citations → notification  │
└─────────────────────────────────────────────────────────────────────────┘
   UI (Tauri webview, docs/01): sidebar list · notes+transcript ·
   who-said-what rail · recording pill — reads the same SQLite via commands
```

The Python pipeline (`pipeline/`) is the same PIPELINE→STORE→SUMMARIZE section as
runnable scripts (mlx-whisper + pyannote community-1 instead of whisper-rs +
sherpa-onnx), for use today and for offline accuracy evaluation of engine swaps.

## 2. The two-track design (the load-bearing idea)

Capture writes **mic** and **system** audio separately instead of mixing:

1. **Diarization gets easier.** The mic track is the local user by construction —
   zero clustering risk for "Me". Diarization only distinguishes remote voices.
2. **Echo double-capture disappears structurally.** On speakers, the mic hears the
   remote side again; a mixed file needs AEC (Apple's voice-processing IO has
   documented AGC side-effects). Two tracks: transcripts of remote speech come from
   the clean system track; near-duplicates on the mic track lose the overlap vote.
3. **Attribution survives storage** — `segments.source` ('mic'/'system').

Hyprnote ships the same shape (its mic input filters out its own system tap device);
Meetily runs a 48 kHz two-source pipeline. [search-verified in docs/02/04 sources]

## 3. Trust boundaries & failure behavior

- **Nothing leaves the machine.** The only socket is `localhost:8000` (oMLX).
- oMLX down → transcription/diarization still complete and persist
  (`status='transcribed'`); summarization retries when `healthy()` flips.
- Preferred model missing → router fallback + `model_was_fallback=1` recorded.
- App crash mid-meeting → WAVs are flushed-per-buffer and finalize on next start;
  detector state machine rebuilds from live signals (no persisted detector state).
- Meeting app crash → process-death path emits MeetingEnded → clean auto-stop.

## 4. Permissions (macOS)

| Grant | Needed by | When asked |
|---|---|---|
| Microphone (`NSMicrophoneUsageDescription`) | cpal mic track | first recording |
| Screen & System Audio Recording | ScreenCaptureKit system track | first recording (macOS 15 re-prompts ~monthly for unused apps) |
| Automation (per browser) | Meet tab probe | first Meet detection |
| Notifications | bundled-build prompt | first prompt |

Future: Core Audio process taps (macOS 14.4+) can replace SCK behind the
`SystemAudioSource` trait — per-process capture under the quieter "System Audio
Recording Only" grant (docs/02 sources; Rust binding maturity was the blocker).

## 5. Memory budget (64 GB M-series, from fetched model sizes)

| Component | Resident | Notes |
|---|---|---|
| Qwen3.6-35B-A3B oQ4e-mtp (oMLX) | ~21.6 GB | user-managed server; SSD KV tier |
| whisper large-v3-turbo q5 | ~1.7 GB | loaded per recording, then dropped |
| sherpa-onnx diarization models | tens of MB | int8 ONNX |
| daemon + Tauri UI | ~hundreds of MB | detector idle cost ≈ a process poll |
| **Total during summarization** | **< 25 GB** | < 40% of unified memory |
