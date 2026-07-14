# Task 4 — Diarization Evaluation: the meetily_pipeline vs. Open-Source Standards

Scope per the master prompt: local, open-source methods only (pyannote, WhisperX,
local NeMo); no cloud APIs. Evidence tiers: **[fetched]** (HuggingFace model cards
were directly fetchable from this sandbox), **[search-verified]**, **[inference]**.

## 1. What the attached pipeline actually does — the honest baseline

Reading every file in `meetily_pipeline.zip` (see `reference/meetily_pipeline/`):

1. **There is no diarization implementation in the pipeline.** `pipeline.py` reads
   `speaker` straight from Meetily CE's `transcripts.speaker` column
   (`meetily_db_extractor.py`, line 70-75) and never touches audio at all. The venv
   contains exactly one inference-related package: the `openai` client. The
   "diarization stage" named in the attachment's filename is *delegation to
   Meetily CE* — and Meetily markets diarization as a Pro-edition feature
   [search-verified: https://github.com/Zackriya-Solutions/meetily], so the CE
   `speaker` column this pipeline depends on may be empty or degraded.
2. **The pipeline as shipped has never run end-to-end**: `logs/summarizer_error.log`
   shows repeated `ModuleNotFoundError: No module named 'db_extractor'` —
   `pipeline.py:6` imports `db_extractor`, the file is `meetily_db_extractor.py`
   (a note on line 4-5 even acknowledges it). Fixed in the rewrite.
3. Downstream consequences of missing diarization: `chunk_transcript` and all three
   prompt stages are speaker-aware ("Include the speaker's name") — the summarizer's
   output quality is capped by speaker labels it has no control over, with no
   timestamps (Meetily rows are used without ms offsets), so citations back into
   audio are impossible.

So the comparative analysis below is: *the delegation approach* vs. the open-source
standards, followed by the concrete replacement that was built.

## 2. The open-source field, July 2026

### pyannote.audio 4.x / speaker-diarization-community-1 — the accuracy reference
- Pipeline: powerset segmentation (`segmentation-3.0`, MIT, 10 s windows, ≤3
  concurrent speakers) + WeSpeaker embeddings + VBx clustering. **[fetched]**
  https://huggingface.co/pyannote/speaker-diarization-community-1,
  https://huggingface.co/pyannote/segmentation-3.0
- DER: **17.0** AMI-IHM / **20.2** DIHARD-3 / **11.2** VoxConverse / 20.3 AliMeeting
  (vs legacy 3.1: 18.8 / 21.4 / 11.2 / 24.5). **[fetched]** (same card)
- License CC-BY-4.0; gated on HF, but an ungated mirror exists
  (`pyannote-community/speaker-diarization-community-1`) — no token friction.
  **[fetched]** https://huggingface.co/pyannote-community/speaker-diarization-community-1
- New "exclusive speaker diarization" output: overlap-free, built for aligning with
  transcription timestamps. **[fetched]** (card)
- Apple Silicon: runs CPU by default; `pipeline.to(torch.device("mps"))` works but is
  community-supported with a history of MPS timestamp bugs. **[search-verified]**
  https://github.com/pyannote/pyannote-audio/discussions/1155 (+ issue #1337)
- The paid `precision-2` model is cloud — excluded per the prompt's constraint.

### WhisperX — the standard glue, weak on Apple Silicon
faster-whisper (CTranslate2) ASR + VAD + wav2vec2 forced alignment + pyannote
diarization; maintained (v3.8.7rc1, June 2026); switched its diarization backend to
community-1. **But CTranslate2 has no reliable MPS path — M-series users run CPU.**
The `whispermlx` fork swaps the backend for mlx-whisper (Metal-native).
**[search-verified]** https://github.com/m-bain/whisperX, https://pypi.org/project/whispermlx/

### NVIDIA NeMo Sortformer — impressive, wrong platform
- Offline `diar_sortformer_4spk-v1`: 123 M params, DER 14.76 DIHARD-3, but
  **CC-BY-NC-4.0 (non-commercial — product-blocking)**, ≤4 speakers, ~12 min per
  pass, GPU-oriented. **[fetched]** https://huggingface.co/nvidia/diar_sortformer_4spk-v1
- Streaming `diar_streaming_sortformer_4spk-v2`: 117 M params, CC-BY-4.0, true
  streaming (0.32 s latency, DER 13.24 DIHARD-3) — but NeMo/CUDA-centric, ≤4
  speakers (28.74 DER at 6), and ONNX export of the streaming model currently fails
  (dynamic slicing), so no macOS path. **[fetched]**
  https://huggingface.co/nvidia/diar_streaming_sortformer_4spk-v2;
  **[search-verified]** https://github.com/NVIDIA-NeMo/NeMo/issues/15077
- Verdict: excluded from the shipping path on license (v1) and platform (v2) grounds;
  revisit v2 if ONNX export lands.

### Apple-native ports — the practical sweet spot
- **FluidAudio / speaker-diarization-coreml**: the pyannote-family pipeline as CoreML
  on the Neural Engine; DER within ~1% of PyTorch; **[fetched]**
  https://huggingface.co/FluidInference/speaker-diarization-coreml. Reported RTF
  ~0.02 (141× realtime on M1), 17.7 DER AMI-SDM, streaming supported; Swift SDK
  (Apache-2.0). **[search-verified]** https://github.com/FluidInference/FluidAudio
  Companion Parakeet TDT v3 CoreML ASR: ~110× realtime on M4 Pro, 25 languages.
  **[fetched]** https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v3-coreml
- **sherpa-onnx**: offline diarization from pure ONNX Runtime — pyannote
  `segmentation-3.0` ONNX export + a speaker-embedding ONNX model (3D-Speaker/
  WeSpeaker/NeMo, int8 variants); macOS arm64; **Rust bindings** (upstream examples +
  the `sherpa-rs` crate). **[fetched]**
  https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0;
  **[search-verified]** https://github.com/thewh1teagle/sherpa-rs
- **senko**: 3D-Speaker CAM++ pipeline via CoreML — 1 h audio in ~7.7 s on M3, but
  DER 26.5 on AMI-IHM (meeting audio!) vs 13.5 VoxConverse — a warning that fast
  pipelines can degrade badly on exactly our domain. **[search-verified]**
  https://github.com/narcotic-sh/senko

## 3. Verdict table

| Criterion | Attached pipeline (delegate to Meetily CE) | pyannote community-1 | WhisperX | NeMo Sortformer | sherpa-onnx (pyannote ONNX) | FluidAudio (CoreML) |
|---|---|---|---|---|---|---|
| Runs locally, open-source | ✅ (but depends on Meetily Pro for speakers) | ✅ CC-BY-4.0 | ✅ | v1 ❌ NC / v2 ✅ | ✅ | ✅ Apache-2.0 SDK |
| Meeting-audio DER | n/a (no diarization) | **17.0 AMI-IHM** | ≈ community-1 | 13-15 DIHARD (≤4 spk) | ≈ segmentation-3.0-family | 17.7 AMI-SDM |
| Apple Silicon accel | n/a | MPS unofficial | ❌ CPU | ❌ | CPU (CoreML EP possible) | **ANE, RTF ~0.02** |
| Memory (64 GB budget) | ~0 | ~1-2 GB PyTorch runtime | 2-4 GB | GPU-class | **tens of MB (ONNX int8)** | ANE-resident, minimal |
| Word/turn timestamps | ❌ none at all | ✅ | ✅ (forced alignment) | ✅ | ✅ | ✅ |
| No-Python runtime possible | — | ❌ | ❌ | ❌ | **✅ (Rust)** | ✅ (Swift sidecar) |
| Integration effort from Tauri/Rust | zero (status quo) | Python sidecar | Python sidecar | high | **low (sherpa-rs)** | medium (swift-bridge/XPC) |

## 4. Strengths & weaknesses of the attached code (as asked)

**Strengths (genuine):**
- The 3-stage MapReduce (extract → outline → rewrite) with a strict extraction format
  is a sound long-transcript strategy; prompts are speaker-aware and well-written.
- Chunk overlap carryover preserves context across boundaries (ported byte-compatibly
  to Rust and kept in Python).
- Zero-cost reuse of Meetily's capture/ASR — a pragmatic v0.
- **Apple Silicon memory:** effectively 0 extra bytes — nothing runs locally except
  an HTTP client. Can't be beaten, but only because the work isn't being done.

**Weaknesses:**
- **Accuracy:** speaker labels are only as good as Meetily CE's column (diarization
  is Pro-gated upstream); no timestamps → no audio citations; overlap regions and
  cross-talk unhandled; local user is not separated from remote speakers.
- **Latency:** batch-only and serial — every chunk is a synchronous round-trip; a
  1-hour meeting at ~8 chunks means 8 sequential LLM calls before stage 2 begins.
- **Robustness:** the import bug (§1.2); `run_stage` returns the literal string
  `"ERROR: {e}"` into downstream prompts on failure; `last_processed.txt` state
  hidden inside the extractor; hardcoded user path; `os.system` osascript with
  f-string interpolation (quote-injection).
- **Misplaced parameter:** `extra_body={"preserve_thinking": True}` — per the Qwen3.6
  card **[fetched: https://huggingface.co/Qwen/Qwen3.6-35B-A3B]**, `preserve_thinking`
  retains reasoning across *multi-turn* history; a stateless 3-stage pipeline has
  none. The rewrite disables thinking outright (`enable_thinking: false`), which cuts
  latency instead of adding trace tokens.

## 5. What was built (the concrete improvements)

**Python (`pipeline/`)** — the "updated pipeline scripts for local diarization"
required by the Output Requirements:
- `wsw/diarize.py`: pyannote **community-1** via the ungated mirror, CPU-default with
  `WSW_DIARIZE_DEVICE=mps` opt-in (MPS bug history, §2), known-speaker-count pinning,
  prefers the exclusive (overlap-free) output for alignment.
- `wsw/transcribe.py`: mlx-whisper (Metal) first, faster-whisper CPU fallback —
  sidestepping WhisperX's CTranslate2-on-Mac problem while keeping its architecture
  (independent ASR + diarization, merged by overlap).
- `wsw/merge.py`: max-overlap speaker attribution + same-speaker coalescing +
  two-track interleave (mirrors the tested Rust implementation).
- `wsw/summarize.py`: fixed import chain, fail-fast errors, Qwen-card sampling,
  thinking disabled, single-pass path for short meetings.
- `run.py audio ...` (new capture-first flow) and `run.py meetily` (the original
  flow, kept working).
- Stdlib-only logic is tested (18 tests pass in this sandbox); model-dependent code
  paths are import-guarded with actionable errors (heavy deps not installable here —
  D-006).

**Rust (`src-tauri/src/diarize/`)** — the native stage for the app itself:
sherpa-onnx (`sherpa-rs`) running pyannote segmentation-3.0 ONNX + 3D-Speaker
embeddings, ~tens of MB, no Python at runtime; `merge.rs` unit-tested. FluidAudio
CoreML is the documented upgrade path behind the same `Diarizer` trait if ANE
throughput becomes worth a Swift sidecar (D-012).

**The structural optimization (inference, but decisive):** two-track capture makes
diarization *easier* — the mic WAV is the local user by construction (no clustering
needed for "Me"), and diarization only clusters the system track's remote voices.
Overlap between local and remote speech survives as separate turns rather than being
lost to single-track overlap resolution. This is an accuracy improvement no
single-track pipeline gets, and it costs nothing at capture time (docs/00 §2).
