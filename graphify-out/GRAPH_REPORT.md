# Graph Report - .  (2026-07-19)

## Corpus Check
- Corpus is ~44,218 words - fits in a single context window. You may not need a graph.

## Summary
- 834 nodes · 1577 edges · 59 communities (40 shown, 19 thin omitted)
- Extraction: 96% EXTRACTED · 4% INFERRED · 0% AMBIGUOUS · INFERRED: 64 edges (avg confidence: 0.78)
- Token cost: 355,942 input · 0 output

## Community Hubs (Navigation)
- Pipeline CLI Runner
- LLM Chat Client
- Tauri Shell / App State
- SQLite Store & Schema
- Capture Session Manager
- Meeting Detector Core
- macOS Screen Capture
- Architecture Overview Doc
- Extract Action Items
- macOS Notification Prompts
- macOS Detection Signals
- Diarization & Inference Routing
- App Icons & Bundling Config
- UI/UX Design Doc
- Transcript Chunking
- Action Item Extraction Tests
- Pipeline Worker Thread
- App Config
- Meetily Source Adapter
- Speaker Merge Logic
- ASR Transcription (Whisper)
- Diarization Engine (Sherpa)
- Tauri Capabilities & Windows
- Pipeline Resampling
- Store Integration Tests
- Timestamp Parsing Tests
- Merge Logic Tests
- Python Merge Module
- Meetily Reference Pipeline
- Action Item Parsing Tests
- Meetily DB Extractor
- Canonical Store Decision
- Structured Extraction Tests
- Cross-Language Test Harness
- Reference Schema Comparisons
- Fathom & Rail Design
- Chappie Mic Corroboration
- Teams Detection Signal
- Embeddings & Status Doc
- Project Scope Decision
- Shell Feature Flag
- Memory Budget Note
- Permissions Note
- WAL Mode Note
- NeMo Sortformer Note
- Senko Note
- CI Core Job
- Icon 128x128@2x
- Icon 128x128
- Icon 32x32
- Icon 64x64
- Icon (App Icon)

## God Nodes (most connected - your core abstractions)
1. `Store` - 30 edges
2. `MeetingApp` - 30 edges
3. `Turn` - 28 edges
4. `DbError` - 24 edges
5. `process_recording()` - 21 edges
6. `AppState` - 20 edges
7. `SessionEffect` - 14 edges
8. `Config` - 14 edges
9. `locked()` - 14 edges
10. `citations_and_action_items_round_trip()` - 13 edges

## Surprising Connections (you probably didn't know these)
- `openai Python client dependency` --semantically_similar_to--> `OpenAiCompatClient (OpenAI-compatible HTTP client)`  [INFERRED] [semantically similar]
  pipeline/requirements.txt → docs/05-inference-routing.md
- `mlx-whisper dependency (Apple Silicon ASR)` --semantically_similar_to--> `whisper.cpp via whisper-rs (in-process transcription)`  [INFERRED] [semantically similar]
  pipeline/requirements.txt → docs/05-inference-routing.md
- `full build + test CI job (whisper + sherpa)` --shares_data_with--> `sherpa-onnx ONNX diarization (Rust bindings)`  [INFERRED]
  .github/workflows/mac-build.yml → docs/04-diarization-evaluation.md
- `tauri bundle CI job (.app + .dmg)` --shares_data_with--> `sherpa-onnx ONNX diarization (Rust bindings)`  [INFERRED]
  .github/workflows/mac-build.yml → docs/04-diarization-evaluation.md
- `whosaidwhat README` --references--> `tauri bundle CI job (.app + .dmg)`  [EXTRACTED]
  README.md → .github/workflows/mac-build.yml

## Import Cycles
- None detected.

## Hyperedges (group relationships)
- **mac-build.yml independent CI jobs (core, harness, shell, full, bundle)** — github_workflows_mac_build_core, github_workflows_mac_build_harness, github_workflows_mac_build_shell, github_workflows_mac_build_full, github_workflows_mac_build_bundle [EXTRACTED 1.00]
- **Diarization engine candidates compared in Task 4 verdict table** — docs_04_diarization_evaluation_pyannote_community1, docs_04_diarization_evaluation_whisperx, docs_04_diarization_evaluation_nemo_sortformer, docs_04_diarization_evaluation_fluidaudio, docs_04_diarization_evaluation_sherpa_onnx [EXTRACTED 1.00]
- **whosaidwhat end-to-end pipeline stages (DETECT->PROMPT->CAPTURE->PIPELINE->STORE->SUMMARIZE)** — docs_00_architecture_detect_module, docs_00_architecture_prompt_module, docs_00_architecture_capture_module, docs_00_architecture_pipeline_module, docs_00_architecture_store_module, docs_00_architecture_summarize_module [EXTRACTED 1.00]

## Communities (59 total, 19 thin omitted)

### Community 0 - "Pipeline CLI Runner"
Cohesion: 0.06
Nodes (35): Namespace, cmd_audio(), cmd_meetily(), _extract_structured(), main(), notify(), Stage 4 + structured storage. An LLM failure here must not fail the     run — th, Best-effort macOS notification; silent no-op elsewhere.      The message is user (+27 more)

### Community 1 - "LLM Chat Client"
Cohesion: 0.08
Nodes (36): Client, Into, ChatChoice, ChatMessage, ChatRequest, ChatResponse, ChatResponseMessage, LlmError (+28 more)

### Community 2 - "Tauri Shell / App State"
Cohesion: 0.13
Nodes (46): AppHandle, CmdResult, MutexGuard, Receiver, AppState, build_hidden_windows(), build_tray(), ControlMsg (+38 more)

### Community 3 - "SQLite Store & Schema"
Cohesion: 0.14
Nodes (27): ActionItemRow, anonymous_speakers_do_not_collide_across_meetings(), CitationRow, citations_and_action_items_round_trip(), DbError, end_to_end_meeting_flow(), fts5_sanitize(), MeetingRow (+19 more)

### Community 4 - "Capture Session Manager"
Cohesion: 0.12
Nodes (30): B, auto_policy_records_without_prompt(), backend_failure_surfaces_error_and_resets(), decline_then_end_is_quiet(), FakeBackend, manual_start_and_stop(), meeting_end_before_answer_dismisses_prompt(), mgr() (+22 more)

### Community 5 - "Meeting Detector Core"
Cohesion: 0.10
Nodes (30): Duration, Detector, Detector<S>, frame(), full_zoom_meeting_lifecycle_through_detector(), macos::MacSignalSource, poll_interval_switches_with_app_presence(), HashMap (+22 more)

### Community 6 - "macOS Screen Capture"
Cohesion: 0.10
Nodes (26): BufWriter, File, Instant, SCStream, ActiveRecording, CaptureError, MacRecorder, MacRecorder<S> (+18 more)

### Community 7 - "Architecture Overview Doc"
Cohesion: 0.06
Nodes (34): CAPTURE: two-track recording (cpal + ScreenCaptureKit), DETECT: process/mic poll loop + AppDetector state machine, Hyprnote hypr-audio two-track precedent, Meetily two-source 48kHz capture precedent, PIPELINE: post-meeting batch worker thread, PROMPT: Start recording? notification surface, STORE: db.rs + schema.sql, SUMMARIZE: oMLX MapReduce summarization stages (+26 more)

### Community 8 - "Extract Action Items"
Cohesion: 0.10
Nodes (31): Match, _marker_ms(), parse_action_items(), parse_timestamps(), quote_snippet(), Structured extraction: ``[mm:ss]`` citation markers and action items.  Mirror of, Citation quote bound (mirror of pipeline::quote_snippet in Rust)., Convert one marker match to milliseconds; None if fields are invalid. (+23 more)

### Community 9 - "macOS Notification Prompts"
Cohesion: 0.14
Nodes (20): FnOnce, PendingResponse, copy_names_the_app(), DelegateIvars, dismiss_drops_pending_callback(), NotifDelegate, prompt_copy(), PromptPresenter (+12 more)

### Community 10 - "macOS Detection Signals"
Cohesion: 0.12
Nodes (19): AnyObject, AudioDeviceID, AudioObjectPropertyAddress, AppLifecycle, BrowserKind, detect_meet_tab(), MacSignalSource, MicMonitor (+11 more)

### Community 11 - "Diarization & Inference Routing"
Cohesion: 0.10
Nodes (27): FluidAudio CoreML diarization (Apple Neural Engine), pyannote speaker-diarization-community-1, Python pipeline rewrite (wsw/diarize.py, transcribe.py, merge.py, summarize.py), Rust native diarization module (src-tauri/src/diarize/), sherpa-onnx ONNX diarization (Rust bindings), WhisperX (ASR+diarization glue), D-013: OpenAI-compatible-only integration decision, Gemma 4 model family (native-audio evaluation) (+19 more)

### Community 12 - "App Icons & Bundling Config"
Cohesion: 0.07
Nodes (26): app, dmg, icons/128x128@2x.png, icons/128x128.png, icons/32x32.png, icons/64x64.png, icons/icon.icns, icons/icon.png (+18 more)

### Community 13 - "UI/UX Design Doc"
Cohesion: 0.09
Nodes (25): whosaidwhat Architecture doc, whosaidwhat UI/UX Design doc, Granola (competitor product), Notes-first detail pane with citation chips, Notion AI Meeting Notes (competitor product), Paper & Verdigris color scheme, Recording pill (user-movable always-on-top window), superwhisper (competitor product) (+17 more)

### Community 14 - "Transcript Chunking"
Cohesion: 0.13
Nodes (18): Progress, chunk_turns(), everything_fits_in_one_chunk(), format_chunk(), overlap_zero_has_no_carryover(), oversized_single_turn_is_not_split(), String, Vec (+10 more)

### Community 15 - "Action Item Extraction Tests"
Cohesion: 0.14
Nodes (15): ActionItem, bracketed_owner_format_is_tolerated(), only_a_truly_trailing_marker_is_stripped(), parse_action_items(), parse_marker(), parse_timestamps(), parses_action_items_with_owner_timestamp_variants(), quote_snippet() (+7 more)

### Community 16 - "Pipeline Worker Thread"
Cohesion: 0.16
Nodes (15): F, I, JoinHandle, drop_with_empty_queue_exits_cleanly(), Job, PipelineWorker, processes_jobs_fifo_with_thread_local_state(), Drop (+7 more)

### Community 17 - "App Config"
Cohesion: 0.16
Nodes (11): From, Config, defaults_roundtrip_through_json(), RecordPolicy, RecordPolicyConfig, Default, Path, PathBuf (+3 more)

### Community 18 - "Meetily Source Adapter"
Cohesion: 0.20
Nodes (16): _connect_readonly(), describe_schema(), get_latest_meeting(), get_latest_transcript(), get_transcript(), Connection, RuntimeError, Turn (+8 more)

### Community 19 - "Speaker Merge Logic"
Cohesion: 0.27
Nodes (15): asr(), attribute_speakers(), coalesce_merges_same_speaker_within_gap(), coalesce_turns(), interleave(), interleave_is_chronological_and_stable(), majority_overlap_wins(), no_overlap_falls_back() (+7 more)

### Community 20 - "ASR Transcription (Whisper)"
Cohesion: 0.17
Nodes (12): AsrError, AsrSegment, Send, String, Transcriber, AsrSegment, Result, Self (+4 more)

### Community 21 - "Diarization Engine (Sherpa)"
Cohesion: 0.17
Nodes (11): Diarize, DiarizeError, Diarizer, Send, String, SpeakerSegment, Result, Self (+3 more)

### Community 22 - "Tauri Capabilities & Windows"
Cohesion: 0.13
Nodes (14): core:default, core:window:allow-close, core:window:allow-hide, core:window:allow-set-focus, core:window:allow-show, core:window:allow-start-dragging, main, pill (+6 more)

### Community 23 - "Pipeline Resampling"
Cohesion: 0.28
Nodes (13): load_wav_16k_mono(), PipelineError, process_recording(), resample_halves_length_48k_to_16k_ratio(), resample_linear(), FnMut, Option, Path (+5 more)

### Community 24 - "Store Integration Tests"
Cohesion: 0.29
Nodes (3): StoreTests, Turn, TypedDict

### Community 25 - "Timestamp Parsing Tests"
Cohesion: 0.20
Nodes (3): Tests for wsw.extract (marker parsing, resolution, action items) and the store.s, TestParseTimestamps, TestResolveSegment

### Community 26 - "Merge Logic Tests"
Cohesion: 0.33
Nodes (4): asr(), MergeTests, Stdlib-only tests for merge.py and store.py (mirrors the Rust merge tests)., spk()

### Community 27 - "Python Merge Module"
Cohesion: 0.27
Nodes (9): attribute_speakers(), coalesce_turns(), interleave(), _overlap_ms(), Turn, Merge ASR segments with diarization segments into speaker-attributed turns.  Pur, Assign each ASR segment the speaker with the largest time overlap.     Ties brea, Merge consecutive same-speaker turns separated by at most max_gap_ms. (+1 more)

### Community 28 - "Meetily Reference Pipeline"
Cohesion: 0.27
Nodes (9): chunk_transcript(), format_chunks(), process_transcript(), Runs the full MapReduce pipeline: Chunk -> Extract -> Outline -> Rewrite., # NOTE: If you saved your previous script as 'meetily_db_extractor.py',, Splits the transcript into semantic chunks based on word count,      injecting a, Formats the list of turn dictionaries into plain text blocks for the LLM., Wrapper function to execute a prompt against the local oMLX server. (+1 more)

### Community 30 - "Meetily DB Extractor"
Cohesion: 0.33
Nodes (3): inspect_schema(), # NOTE: Updated to use 'transcript' instead of 'text' based on the schema, Prints the database tables and columns to verify the schema.

### Community 32 - "Canonical Store Decision"
Cohesion: 0.40
Nodes (5): Google Meet detection via browser tab scanning, minutes (silverstein) browser tab detection precedent, D-011: SQLite-canonical, markdown-exportable decision, Minutes (markdown-canonical store precedent), D-011: reject markdown-canonical store, keep SQLite canonical

### Community 34 - "Cross-Language Test Harness"
Cohesion: 0.50
Nodes (3): AsrSegment, String, SpeakerSegment

### Community 35 - "Reference Schema Comparisons"
Cohesion: 0.50
Nodes (4): Screenpipe (open-source Tauri competitor), Hyprnote numbered-migration precedent, schema.sql — single source of truth DDL, Screenpipe DB (audio paths, migrations table precedent)

## Knowledge Gaps
- **66 isolated node(s):** `$schema`, `identifier`, `description`, `main`, `prompt` (+61 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **19 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `Store` connect `SQLite Store & Schema` to `Tauri Shell / App State`, `Capture Session Manager`, `Pipeline Resampling`?**
  _High betweenness centrality (0.120) - this node is a cross-community bridge._
- **Why does `process_recording()` connect `Pipeline Resampling` to `LLM Chat Client`, `SQLite Store & Schema`, `Transcript Chunking`, `Action Item Extraction Tests`, `Speaker Merge Logic`, `ASR Transcription (Whisper)`, `Diarization Engine (Sherpa)`?**
  _High betweenness centrality (0.107) - this node is a cross-community bridge._
- **Why does `MeetingApp` connect `Capture Session Manager` to `Tauri Shell / App State`, `Meeting Detector Core`, `macOS Screen Capture`, `macOS Notification Prompts`, `macOS Detection Signals`?**
  _High betweenness centrality (0.096) - this node is a cross-community bridge._
- **Are the 18 inferred relationships involving `Turn` (e.g. with `cmd_audio()` and `TestParseActionItems`) actually correct?**
  _`Turn` has 18 INFERRED edges - model-reasoned connections that need verification._
- **What connects `$schema`, `identifier`, `description` to the rest of the system?**
  _66 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Pipeline CLI Runner` be split into smaller, more focused modules?**
  _Cohesion score 0.0602322206095791 - nodes in this community are weakly interconnected._
- **Should `LLM Chat Client` be split into smaller, more focused modules?**
  _Cohesion score 0.083710407239819 - nodes in this community are weakly interconnected._