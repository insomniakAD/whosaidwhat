# Build Log — whosaidwhat

This log records every decision made during the autonomous run, including every question
that would normally have been asked of the project owner, the answer chosen, and why.
Per the master prompt's guardrails: nothing here is invented; claims are labeled as
**verified** (URL fetched), **inference** (reasoned from verified facts), or
**unverified** (could not confirm).

Run date: 2026-07-14. Executed by Claude (model id `claude-fable-5`) on branch
`claude/master-prompt-execution-2r00qg`.

---

## Ground truth established from the attached materials

1. **The attached `meetily_pipeline.zip` contains no diarization code.** It contains:
   `pipeline.py` (chunk → extract → outline → blog-post MapReduce against a local oMLX
   server), `meetily_db_extractor.py` (reads Meetily CE's SQLite DB), a Python 3.14 venv
   (openai 2.45.0 client only — no audio/ML packages), and logs. The `speaker` field is
   read straight from Meetily's `transcripts.speaker` column. So the "diarization
   implementation" Task 4 asks me to evaluate is, in reality, *delegation of speaker
   labels to Meetily CE*. I evaluate exactly that, then build the missing stage.
   (Verified by reading every file in the zip.)

2. **The pipeline is currently broken.** `logs/summarizer_error.log` shows repeated
   `ModuleNotFoundError: No module named 'db_extractor'` — `pipeline.py` line 6 imports
   `db_extractor`, but the file shipped is `meetily_db_extractor.py`. The pipeline has
   never run end-to-end in its attached form. Fixed in the rewritten pipeline.

3. **The second attached markdown file is a duplicate of the master prompt**, not a
   pipeline description. Noted; the zip is the source of truth for the pipeline.

4. **Target machine**: Apple Silicon (M-series), 64 GB unified memory, macOS. The prompt
   says "M5 Pro Max"; I could not verify that exact SKU name exists and it changes
   nothing architecturally — I design for "modern M-series, 64 GB" and note per-component
   memory budgets. (Labeled: unverified SKU, immaterial.)

---

## Decision log

Each entry: **Q** (the question I would have asked), **A** (the call I made), **Why**.

### D-001 — Repo is empty; how much app do I build?
**Q:** The GitHub repo `insomniakad/whosaidwhat` has zero commits. Do I build a full
Tauri app or deliver the Arc's artifacts?
**A:** Deliver exactly what the Arc's Output Requirements name — Rust process-detection
code, updated pipeline scripts, architecture explanation — inside a real, coherent
project structure (a compiling Rust crate + runnable Python pipeline + docs), so every
artifact has a home and the repo becomes the seed of the actual app.
**Why:** The Output Requirements section is the definition of done; a half-generated
full app would dilute quality of the named deliverables.

### D-002 — Is oMLX real, and is the model name real?
**Q:** `pipeline.py` targets oMLX at `localhost:8000/v1` with model
`Qwen3.6-35B-A3B-oQ4e-mtp`. Are these real?
**A:** Yes to both. oMLX is a native macOS MLX inference server with OpenAI- and
Anthropic-compatible endpoints — **[search-verified]** only (omlx.ai and
github.com/jundot/omlx were blocked by this sandbox's egress proxy; the facts come
from search-result excerpts, corroborated by the original pipeline.py successfully
targeting `localhost:8000/v1`). Qwen3.6-35B-A3B is Alibaba's April 2026 MoE release,
35B total / 3B active — **[fetched]** https://huggingface.co/Qwen/Qwen3.6-35B-A3B
(HuggingFace was directly fetchable). Details (quant naming, `preserve_thinking`
param) verified in the research fan-out (docs/05).
*(Correction: an earlier draft of this entry said omlx.ai was "fetched"; that was
wrong and is fixed here — consistent with D-008 and docs/05 §4.)*
**Why:** Both post-date my training data; guardrail 1 requires fetching, not assuming.

### D-003 — Orchestration shape
**Q:** How to satisfy "use multi-agent workflows aggressively" without burning the run
on coordination overhead?
**A:** One research workflow (8 parallel researchers → skeptic verification of every
load-bearing claim → completeness critic), then I write the deliverables in the main
loop, then a second adversarial review workflow over the finished artifacts.
**Why:** Research and review parallelize well and benefit from independent skeptics;
the actual writing benefits from one coherent mind holding all five tasks at once.

### D-004 — Python at runtime vs pure Rust
**Q:** Task 5 says "if transcription and diarization require different inferencing
engines than oMLX, write the native Rust implementations for those stages directly
into the app." Do I keep any Python?
**A:** Both. The updated `pipeline/` Python scripts are delivered (the Arc explicitly
requires "updated pipeline scripts for local diarization" — they remain the fastest
path to a working pipeline today and are testable standalone), and the Rust crate
contains native transcription+diarization stage implementations so the app itself has
no Python runtime dependency. Engine choices are research-driven (see docs 04/05).
**Why:** The Arc asks for both artifacts explicitly; they serve different lifecycles
(scripts = today's working pipeline; Rust = the app's future built-in path).

### D-005 — Project name/branding
**Q:** Dashboard needs a product identity for the UI doc.
**A:** Use the repo name: **whosaidwhat** — it is literally the diarization value
proposition.
**Why:** Only name with any evidence of user intent (they created the repo).

### D-006 — Verification boundary in this sandbox
**Q:** How do I verify code when the build container blocks crates.io, PyPI, and
rustup dist (proxy allowlist)?
**A:** Three layers: (1) every dependency-free Rust module (the meeting-detection state
machine, transcript chunker, recording session logic) is compiled and unit-tested
locally with bare `rustc --test`, which needs no network; (2) all Python is
byte-compiled with `py_compile` and the DB/chunking layers exercised with stdlib-only
tests against a synthetic SQLite DB; (3) an adversarial multi-agent code-review
workflow reads everything for API misuse before commit. What this cannot catch:
type errors inside macOS-FFI code paths (objc2/ScreenCaptureKit bindings) and against
external crate APIs — those are written from fetched documentation and flagged in-file.
**Why:** Blocked is not an option; this is the strongest verification available here,
and the boundary is stated honestly rather than implied away.

### D-007 — Research fan-out results and the session-limit failure
**Q:** The research workflow's verification phase (24 skeptic agents + completeness
critic) and one researcher died on "session limit, resets 3:30am UTC". Wait, degrade,
or reroute?
**A:** Reroute: 7/8 researchers returned before the limit (their findings are the
evidence base cited throughout docs/), the eighth area (native Rust ASR stack) was
covered by targeted main-loop searches, and the adversarial verification pass is
re-run after the reset as a leaner workflow over the finished artifacts (which is
strictly more useful than verifying raw claims — the review sees claims *in context*).
**Why:** Guardrail 2: blocked is not an option; the definition of done includes the
skeptic pass, so it is deferred, not dropped.

### D-008 — Evidence quality tiers (proxy-constrained research)
**Q:** The sandbox egress proxy allowlists huggingface.co and the WebSearch API but
blocks most direct fetches (github.com, apple.com, docs.rs...). How to satisfy
"trace to a real URL you actually fetched"?
**A:** Three explicit tiers used everywhere: **[fetched]** — page retrieved directly
(all HuggingFace model cards, GitHub code-search API fragments); **[search-verified]**
— claim supported by substantial page excerpts returned by the search engine, primary
URL given but not fetched from this sandbox; **[inference]** — my reasoning from
tiered facts. Each doc's sources section labels its tier per claim.
**Why:** Honest labeling per guardrail 1 beats pretending the proxy wasn't there.

### D-009 — Teams meeting-end signal
**Q:** Parse Microsoft's `audiomxd` unified-log state (the only authoritative
Teams-end signal found in shipped code) or degrade?
**A:** Degrade: Teams end = mic-quiet + marker-gone with a 10 s debounce.
**Why:** `log show` parsing of a vendor daemon's private format breaks silently on
Teams updates; the mic signal is first-party API. Noted as optional refinement in
docs/02 §1.

### D-010 — Poll with event gating, not pure events
**Q:** Fully event-driven detection (NSWorkspace + CoreAudio listeners only)?
**A:** 2 s/15 s poll loop, with NSWorkspace events only switching cadence.
**Why:** Meeting markers (CptHost, browser tabs) have no notification API anyway;
polls of a process table cost microseconds; a missed event can never wedge a poller.

### D-011 — SQLite-canonical, markdown-exportable
**Q:** Adopt Minutes' markdown-as-canonical-store (SQLite as disposable index)?
**A:** No: segments carry ms offsets + speaker FKs + FTS that markdown round-trips
poorly. Summaries are stored as export-ready markdown; an exporter stays trivial.
**Why:** whosaidwhat's differentiator (citations into audio) lives in exactly the
data markdown loses. Reversible later.

### D-012 — sherpa-onnx over FluidAudio for the native diarization stage
**Q:** FluidAudio's CoreML/ANE pipeline benchmarks faster (RTF ~0.02); why ship
sherpa-onnx?
**A:** sherpa is pure ONNX with Rust bindings — links straight into the Tauri
process; FluidAudio is Swift and needs a sidecar/bridge. Same pyannote-family
models, so accuracy is comparable (DER within ~1% of PyTorch per FluidAudio's own
card). The `Diarizer` trait is the seam for a FluidAudio sidecar if ANE throughput
ever matters. Post-meeting batch on an M-series is not throughput-bound.

### D-013 — OpenAI-compatible surface only, toward oMLX
**Q:** Use oMLX's Anthropic endpoint / structured-output mode?
**A:** No: exact Anthropic path unverified; structured-output claim risks conflation
with mlx-omni-server (skeptic note in the research). The OpenAI surface is verified
by the original pipeline having worked against it.
**Why:** never build on an unverified contract when a verified one suffices.

### D-014 — Gemma 4 augments, never replaces, Whisper
**Q:** The master prompt asks to evaluate Gemma 4's "encoder-free native audio" as a
Whisper replacement. Verdict?
**A:** Replacement: no — 30 s/request audio cap (fetched from every audio-capable
Gemma 4 card), no timestamps, no diarization, no published WER vs Whisper, non-speech
audio out-of-distribution. Augmentation: yes — fallback summarization model +
future 30 s-clip audio-understanding lane. The encoder-free claim itself is true
only of the 12B Unified variant (E2B/E4B carry a USM-style conformer encoder).
**Why:** full evidence chain in docs/05 §2.

---

## Questions I would have asked, answered myself (index)

| Question | Answer | Where |
|---|---|---|
| Is the repo really empty — build from scratch? | Yes; deliverables as a coherent project seed | D-001 |
| Are oMLX / Qwen3.6 / "oQ4e-mtp" / Gemma 4 real? | All verified real (post-training-cutoff releases) | D-002, docs/05 |
| What does "evaluate the diarization implementation" mean when the zip has none? | Evaluate the delegation-to-Meetily approach honestly, then build the stage | docs/04 §1 |
| Which diarization engine for the app vs the scripts? | sherpa-onnx (Rust) / pyannote community-1 (Python) | D-012, docs/04 |
| Replace Whisper with Gemma 4? | No — augment only | D-014 |
| Mix mic + system audio or keep two tracks? | Two tracks (diarization + echo structurally better) | docs/00 §2 |
| How to prompt "start recording" when Tauri notifications have no buttons on macOS? | Pill window (dev) + UNUserNotificationCenter (bundled) | docs/02 §3 |
| Which color scheme, given no-navy/slate/black? | "Paper & Verdigris" — warm paper + terracotta/verdigris | docs/01 §2.2 |
| What when the session limit killed the verification agents? | Defer skeptic pass to post-reset, over finished artifacts | D-007 |

## Verification record

- Rust pure core: 30 unit tests, compiled with bare `rustc --test` (no network) — pass.
- Python pipeline logic: 18 stdlib tests — pass. All files `py_compile` clean.
- `schema.sql`: executed twice (idempotency), FTS5 trigger sync + snippet/rank +
  UNIQUE versioning + cascade deletes verified against SQLite 3.45.1.
- Not verifiable here (flagged in-file): macOS FFI type-checks, third-party crate
  API surfaces (sherpa-rs, screencapturekit, whisper-rs constructor details), and
  any claim marked [search-verified].

## Adversarial review pass (4 parallel skeptic agents) — findings & dispositions

A verification workflow ran four adversarial reviewers (Rust correctness, Python+SQL
correctness, evidence audit, completeness-vs-master-prompt). It surfaced real defects;
this section records each substantive finding and what was done. Fixes were re-tested
(30 dependency-free Rust tests via the bare-rustc harness, 21 Python tests, schema
re-executed; the 4 new rusqlite-backed `db.rs` tests for speaker namespacing and FTS
sanitization run under `cargo test` on a network that can reach crates.io).

**Fixed — compile-blockers (Rust):**
- `screencapturekit` was pinned to 0.3 but the code targets the rewritten
  (1.x) API → bumped to `screencapturekit = "1"` with a comment (Cargo.toml).
- `objc2-foundation` had no features, but `addObserverForName…` + `NSNotification`
  are feature-gated → added `NSNotification`/`NSString`/`NSOperation`/`block2`.
- SCK `AudioOut` used `&mut self` + a captured `FnMut`; the trait method is `&self`
  and the handler must be `Send`+`'static` → moved the sink behind a `Mutex`.

**Fixed — correctness (Rust):**
- **Pre-Sonoma self-recording mic bug (would truncate every Teams/Meet recording
  ~10 s in):** `mic_in_use` returned `false` when the per-process API was
  unavailable, which fabricated a meeting-end → changed to assume `true`
  (`unwrap_or(true)`), so end degrades to app-quit/tab-close as documented.
- **SCK planar-vs-interleaved audio scramble:** SCK delivers one AudioBuffer per
  channel (planar); the writer expected interleaved frames → now zips channels
  into interleaved order before writing.
- **Concurrent meeting lost after a recording ended:** added
  `Detector::active_meetings()` and a re-offer loop in `main.rs` when the session
  returns to Idle.
- **`meeting_id` second-collision + wrong `started_at`:** id now uses nanoseconds
  (collision-proof); `started_at` computed as `end − duration` instead of the end
  time; both audio paths carried explicitly in `SavedRecording` (no string surgery).
- **FTS5 MATCH crashed on ordinary punctuation** (`don't`, `covid-19`) → added
  `db::fts5_sanitize` (quote each token); mirrored in Python `store.fts5_sanitize`.
- **Cross-meeting speaker collision + missing `is_self`:** `insert_transcript` now
  gives anonymous `SPEAKER_*` labels a fresh per-meeting row, reuses one global
  `is_self` "Me", and dedups only named speakers — mirrored exactly in Python
  `store._speaker_id_for`. New tests lock both paths.

**Fixed — correctness (Python):**
- **`store.py` dropped `segments.source`** (mic turns stored as system) → now sets
  `'mic'`/`'system'` per turn.
- **Meetily import discarded timestamps** (all segments at 0) → synthesizes a
  monotonic ordering sequence, documented as ordering-only (no real offsets).
- **`--out-db` couldn't follow the subcommand** (documented usage failed) → moved
  onto each subparser.
- **AppleScript injection in `notify()`** (the exact bug docs/04 claimed fixed) →
  now escapes backslashes and quotes; noted list-argv only stops shell injection.

**Fixed — SQL:**
- Added FK-covering indexes `idx_citations_segment`, `idx_action_items_meeting`,
  `idx_action_items_summary` (cascade deletes were table scans); documented why
  `speakers.display_name` is intentionally not UNIQUE.

**Fixed — evidence integrity (the guardrail-1 core):**
- **D-002 said omlx.ai was "fetched"** contradicting D-008/docs-05 → relabeled
  `[search-verified]` with an explicit correction note.
- **Sampling `Strict` profile was mislabeled as official** → docs/05 and
  `router.rs` now mark 0.6/0.95/0.0 as `[inference]` adapted from the card's
  thinking-mode precise profile (the card has no non-thinking precise profile).
- Dangling cross-refs fixed: docs/00 Hyprnote/Meetily now carry real URLs;
  `capture/macos.rs` pointed at a nonexistent `audio.py` → corrected; `notify.rs`
  doc-linked a nonexistent `UnNotificationPrompt` type → corrected; docs/02 "both
  presenters supported" → clarified only `WindowPrompt` is implemented.

**Fixed — over-claims softened to match reality (guardrail 1: don't claim done what
isn't):**
- README + docs/01 + docs/03 now state plainly that `summary_citations` /
  `action_items` are schema-ready but not yet populated, that there is no Tauri
  frontend in the repo yet, and that the headless `Prompt` policy does not record
  (consent needs the UI).

**D-015 — consent honesty in the headless daemon:** the shipped daemon has no UI, so
under `RecordPolicy::Prompt` it now logs and does NOT record (only `Auto` records
headlessly). The earlier draft auto-accepted the prompt, which misrepresented consent.

**Accepted as known limitations (documented, not fixed this run):**
- Teams marker (`teams_running && mic`) can false-positive on unrelated mic use while
  Teams idles in the background — no first-party Teams-meeting signal exists (D-009);
  documented in docs/02.
- The headless pipeline runs on the detection thread (`block_on`), so a meeting
  starting mid-processing is detected late — documented in docs/00 §3 and the code;
  the Tauri shell runs it on a background task.
- Gemma 4 is wired only as a fallback model id (now in `RouterConfig::default`), not
  a first-class audio lane — by design (docs/05, D-014).

Net: every critical and every correctness-major finding was fixed and re-tested; the
remaining open items are either genuine platform limitations with no clean fix or
explicitly-scoped future work, and all are now stated honestly in the docs rather
than papered over.

---

# Run 2 — 2026-07-14, branch `claude/build-planning-orchestration-xga7vw`

Scope: exactly the four items run 1's honest-status listed as pending —
structured citations/action-items extraction, pipeline off the detection
thread, the Tauri v2 shell, and the `un_center` notification path. Same
sandbox constraints (crates.io/PyPI/npm blocked; same evidence tiers).
Research: two parallel agents (Tauri v2 shell surface; objc2
UserNotifications), then implementation in the main loop, then an
adversarial review workflow over the diff.

### D-016 — Stage-3 prompt now preserves `[mm:ss]` markers
**Q:** Citations need markers in the *notes*, but only the stage-2 prompt
preserved them; stage 3 (prose rewrite) could drop every timestamp.
**A:** Added one line to STAGE_3_PROMPT (Rust + Python, kept in lockstep):
keep the outline's markers on the points they belong to, invent none.
**Why:** The notes are the primary UI surface (docs/01); citations that only
exist on the outline tab would miss the main reading path. Marker fidelity is
enforced downstream anyway: unresolvable markers are dropped, never linked.

### D-017 — Line-format stage 4, not JSON mode
**Q:** Extract action items as JSON for easy parsing?
**A:** No — strict `* Owner: task [mm:ss]` line format, parsed by a
hand-rolled tolerant scanner (`llm::extract` / `wsw.extract`), mirrored in
both languages with mirrored tests.
**Why:** D-013 stands: the only verified oMLX contract is plain
OpenAI-compatible chat. A malformed line degrades to a skipped item; malformed
JSON would degrade to zero items or a retry loop.

### D-018 — Citation resolution tolerance
**Q:** A marker is truncated to whole seconds and models round; when does a
marker stop being evidence?
**A:** Containment first; else nearest segment start within 10 s; else drop.
Owners resolve meeting-scoped only (`speaker_in_meeting` — a global name
lookup would recreate the cross-meeting SPEAKER_00 collision run 1 fixed).
**Why:** Force-linking a hallucinated `[59:59]` to the last segment would
fabricate provenance; dropping it is the honest failure.

### D-019 — Pipeline worker: one thread + FIFO channel, no async pool
**Q:** tokio task pool for pipeline jobs?
**A:** One dedicated worker thread draining `std::sync::mpsc`, private
Store/router/runtime built on the thread (`pipeline::worker`), graceful
drain-then-join on Drop. Detection thread only creates the meeting row and
queues a Job; worker death is recorded as `failed:worker`.
**Why:** Jobs are minutes long, arrive once per meeting, and must serialize
anyway (whisper/Metal + one WAL writer). A pool adds failure modes, not
throughput. `submit()` is queue-and-return — verified by a test that pushes
100 jobs against a gated worker in <500 ms.

### D-020 — Shell behind a `shell` feature; tauri-build unconditional
**Q:** How does a Tauri app coexist with a crate whose `cargo test` must keep
working on Linux CI without webkit2gtk?
**A:** `tauri` is an optional dep behind `shell`; the app is a second binary
(`whosaidwhat-app`, `required-features = ["shell"]`); build.rs gates
`tauri_build::build()` on `CARGO_FEATURE_SHELL` (feature flags reach build
scripts as env vars, and a build script cannot reference an uncompiled crate,
so tauri-build itself is an unconditional build-dep — pure Rust, no system
deps).
**Why:** Keeps run 1's promise that the pure core tests anywhere, while the
shell is one `--features shell` away. `mainBinaryName` in tauri.conf.json
points the CLI at the right binary. [unverified: whether `tauri dev` passes
`--features` through on every CLI version — plain `cargo build --features
shell` is the documented fallback.]

### D-021 — No-bundler frontend
**Q:** Vite scaffold or static files?
**A:** Hand-written static HTML/CSS/JS in `/dist` (`frontendDist: "../dist"`),
`app.withGlobalTauri: true`, `window.__TAURI__.core.invoke` /
`window.__TAURI__.event.listen`, with a bridge that tolerates late global
injection (tauri#12990) and degrades to empty data in a plain browser. All
dynamic markup built from escaped text; markdown rendered by a minimal
escaping renderer, `[mm:ss]` markers become citation chips. `window.prompt`
avoided (unreliable in wry) — speaker rename is an inline input.
**Why:** npm is blocked here, and the UI is three pages — a bundler buys
nothing. The webview needs no framework to render SQLite rows.

### D-022 — un_center: background Start action, CustomDismissAction
**A:** `WSW_START` is a background action (recording starts in-process;
foregrounding the app would yank focus from the meeting); `WSW_DISMISS` is
Destructive; the category opts into CustomDismissAction so swipe-dismiss
reaches the delegate and clears the pending callback. Only the explicit
Start button records — a body click is not consent. The delegate is kept
alive by the presenter (the center holds it weakly — wezterm's lesson);
`available()` is an NSBundle bundle-identifier check because UN APIs abort
in unbundled binaries (Apple forums 679326/649583).
**Evidence:** API surface verbatim from madsmtm/objc2-generated fragments
([fetched] via code search); delegate/RcBlock patterns from wezterm
([fetched]); bundle constraint [search-verified]. `UnCenterPrompt`'s
`unsafe impl Send` is justified in-code by Apple's any-thread documentation
for the center. Not compilable here — D-006 boundary, flagged in-file.

### D-023 — Shell consent surface selection
**A:** `shell.rs::PromptSurface` picks un_center when `available()` (bundled),
else the hidden always-on-top prompt window; both deliver through the same
ControlMsg channel as the webview buttons, and the detection loop sleeps on
`recv_timeout(poll_interval)` so clicks are handled in milliseconds without
busy-polling.

## Run-2 verification record

- Bare-rustc harness (no network): **49 tests pass** — run 1's 30 plus
  `llm::extract` (marker parsing, resolution tolerance, action-item format,
  quote bounds), `pipeline::worker` (FIFO, non-blocking submit,
  drain-on-drop), and a Rust↔Python twin-parity block. The harness itself is
  now committed (`src-tauri/harness/harness.rs`) instead of living only in
  the run's scratch space.
- Python: **34 tests pass** (run 1's 21 plus `test_extract.py`: parser mirrors
  + a full `save_structured_extraction` round trip against the real schema,
  including the hallucinated-marker drop and meeting-scoped owner rules).
  All files `py_compile` clean.
- `tauri.conf.json` / `capabilities/default.json`: JSON-validated here;
  frontend JS `node --check` clean.
- NOT verifiable here (flagged in-file, same tier as run 1's FFI):
  `shell.rs` and `notify::un_center` against the real `tauri` /
  `objc2-user-notifications` crates; the new `db.rs` methods run under
  `cargo test` on a networked machine (in-memory SQLite tests included).
- Placeholder `icons/icon.png` generated programmatically (stdlib PNG
  encoder); real `.icns` requires `tauri icon` on a Mac.
