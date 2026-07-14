# Task 5 — Inference Routing & LLM Integration

Code: `src-tauri/src/llm/` (client, router, summarize) + `src-tauri/src/asr/` and
`src-tauri/src/diarize/` (the non-oMLX engines). Evidence tiers as in BUILD_LOG D-008.

## 1. The oMLX endpoint structure

oMLX is real and verified: an MLX inference server for Apple Silicon by Jun Kim
("Jundot" — HF profile links omlx.ai and github.com/jundot **[fetched]**
https://huggingface.co/Jundot), with continuous batching and a two-tier paged KV
cache (RAM hot tier with prefix sharing; SSD cold tier that survives restarts)
**[search-verified]** https://github.com/jundot/omlx. It exposes:

- `POST /v1/chat/completions`, `GET /v1/models` — OpenAI-compatible, default
  `http://localhost:8000`, models auto-discovered from `~/.omlx/models`, config via
  `OMLX_*` env vars / `~/.omlx/settings.json`. **[search-verified]** (README via
  search; the original pipeline.py targeting `localhost:8000/v1` corroborates)
- An Anthropic-compatible messages endpoint (exact path unverified this session).
- v0.3.0 reportedly added mlx-audio-backed `/v1/audio/transcriptions` (Whisper,
  Qwen3-ASR, Parakeet, Voxtral). **[search-verified — unconfirmed]**
- v0.5.0 "Lightning MTP" speculative decoding covers Qwen3.6-35B-A3B: 89.6 → 140.4
  tok/s greedy on M3 Ultra with the `-mtp` quants. **[search-verified]**

**Integration decision (D-013):** whosaidwhat speaks only the OpenAI-compatible
surface, via `OpenAiCompatClient` (reqwest + serde, `src/llm/client.rs`):
- narrowest portable contract — swap oMLX for mlx-omni-server / llama.cpp-server /
  LM Studio by changing one base URL;
- the Anthropic path stays unverified, so nothing depends on it;
- server-specific extensions ride in a `#[serde(flatten)] extra` map — typed API
  stays clean, `chat_template_kwargs` still reachable;
- structured-output/JSON-schema mode is **not** relied upon: the "Outlines-based"
  claim in search results risks conflation with the similarly-named
  madroidmaq/mlx-omni-server project. The pipeline's stages consume free-form
  markdown by design, so nothing breaks either way (skeptic-noted).

`InferenceRouter` (`src/llm/router.rs`) owns model resolution at runtime: query
`/v1/models`, prefer the configured model, walk the fallback list, else accept any
served model — and *record the substitution* in the summary's provenance columns
(`model`, `model_was_fallback`). A degraded summary from a smaller model beats a
failed pipeline; the DB never lies about which model wrote what.

## 2. The models

### Qwen3.6-35B-A3B (summarization workhorse) — all **[fetched]** from
https://huggingface.co/Qwen/Qwen3.6-35B-A3B and quant cards:
- MoE: 35 B total / **3 B active** (256 experts, 8 routed + 1 shared); 40 layers in a
  hybrid `10 × (3 × (Gated DeltaNet → MoE) → 1 × (Gated Attention → MoE))` pattern —
  mostly linear-attention layers, so the KV cache stays small at long context.
- Context: 262,144 native (→1,010,000 extended) — a multi-hour meeting fits in one pass.
- The exact model name in the original pipeline is real:
  **Jundot/Qwen3.6-35B-A3B-oQ4e-mtp**, oMLX's own "oQ4e" mixed-precision 4-bit quant
  with MTP heads — **21.1 GB disk / 21.6 GB in MLX** (vs 71.9 GB unquantized).
  **[fetched]** https://huggingface.co/Jundot/Qwen3.6-35B-A3B-oQ4e-mtp
- `preserve_thinking` is a real documented API parameter — for *multi-turn* reasoning
  retention. The original pipeline's use of it on stateless single-shot calls was a
  no-op at best; the rewrite sends `chat_template_kwargs: {enable_thinking: false}`
  instead (thinking off = faster, and Qwen's non-thinking sampling profile applies).
- Sampling → `SamplingProfile::{Prose, Strict}` in the router; extraction/outline
  run Strict, the final rewrite runs Prose. **Provenance, precisely:** the card's
  non-thinking (instruct) profile is **[fetched]** 0.7 / 0.80 / presence 1.5 — that
  is `Prose` exactly. `Strict`'s 0.6 / 0.95 / 0.0 is **[inference]**: the card
  publishes 0.6/0.95/0.0 only as its *thinking-mode "precise coding"* profile, and
  offers no non-thinking precise profile, so I adapted those numbers for
  format-stable extraction with thinking disabled. It is a reasoned adaptation, not
  official non-thinking guidance — do not cargo-cult it as Qwen's non-thinking
  precise setting.

**Memory budget on the 64 GB target (inference from fetched sizes):**
~21.6 GB Qwen weights + KV (small; DeltaNet) + ~1.7 GB whisper large-v3-turbo q5 +
tens of MB sherpa ONNX + app ≈ **< 25 GB total — under 40% of unified memory**, with
oMLX's SSD KV tier giving headroom for long-context sessions.

### Gemma 4 (secondary model + the audio question)
Verified family per Google's HF cards **[fetched]**: E2B, E4B (audio via a ~300 M
USM-style conformer encoder — Gemma 3n lineage), **12B "Unified"**, 26B-A4B, 31B
(vision+text only).
- https://huggingface.co/google/gemma-4-E4B-it, https://huggingface.co/google/gemma-4-12b-it,
  https://huggingface.co/blog/gemma4

**The "encoder-free native audio" claim is true only for Gemma 4 12B Unified**:
"eliminates these encoders entirely, projecting raw image patches and audio
waveforms directly into the LLM's embedding space through lightweight linear
layers" **[fetched]** (12B card). E2B/E4B keep a conformer audio encoder.

**Can it replace Whisper? No — and here is the hard evidence:**
1. **"Audio supports a maximum length of 30 seconds"** per request, on every
   audio-capable Gemma 4 card **[fetched]**. A 60-minute meeting = 120+ chunks with
   boundary loss and no cross-chunk speaker continuity.
2. No documented timestamps and no diarization anywhere on the cards **[fetched]** —
   whosaidwhat's citations and who-said-what rail die without ms offsets.
3. Published audio benchmarks are CoVoST/FLEURS only; **no WER vs Whisper large-v3
   exists to justify the swap** [fetched].
4. Audio training excluded music/non-speech **[fetched]** (HF blog) — meeting audio
   (notification dings, keyboard noise, hold music) is off-distribution.
5. MLX support for Gemma 4 audio is via Python mlx-vlm **[fetched]** (HF blog); the
   mlx-community MLX conversion cards don't even list audio **[fetched]**; Swift-side
   support was an open feature request **[search-verified]**. And oMLX serving audio
   models is unconfirmed (§1).

**Can it augment? Yes (D-014):** router hook `gemma-4-12b-it-4bit` as a fallback
summarization model (multimodal not required for that), and a documented future
"audio-understanding" lane: 30 s clips linked from summary citations could be
re-asked ("what tone was this said in?") via mlx-vlm. Voxtral Mini 3B (dedicated
transcription mode, **30 min** audio per request **[fetched]**
https://huggingface.co/mistralai/Voxtral-Mini-3B-2507) is the stronger
audio-LLM candidate if an LLM-ASR lane is ever wanted — noted, not built.

## 3. Separation of concerns (the answer to "if transcription and diarization
require different inferencing engines than oMLX")

They do, and the native Rust implementations are in the app:

| Stage | Engine | Runtime | Why not oMLX |
|---|---|---|---|
| Summarization / chat | **oMLX** → Qwen3.6-35B-A3B-oQ4e-mtp | HTTP, out-of-process | this *is* oMLX's job; MTP + batching + SSD KV cache are exactly the right infra |
| Transcription | **whisper.cpp** via whisper-rs (`metal`) — `src/asr/whisper.rs` | in-process Rust | needs word/segment timestamps; Gemma 4 caps audio at 30 s; oMLX audio endpoints unconfirmed; whisper-rs is proven (Meetily ships whisper-rs) **[search-verified]** https://github.com/tazz4843/whisper-rs |
| Diarization | **sherpa-onnx** via sherpa-rs — `src/diarize/sherpa.rs` | in-process Rust | not a generation task at all: segmentation + embeddings + clustering; ONNX Runtime, tens of MB |

Process model (inference): oMLX stays a separate user-managed server (menu-bar app,
shared by other tools, models centralized in `~/.omlx/models`) — the app degrades
gracefully when it's down (`healthy()` probe; transcription still completes, summary
retries later). ASR + diarization run in-process because they're bursty post-meeting
batch jobs on files, loaded lazily and dropped after use (`main.rs`).

## 4. Verification limits (honest per guardrail 1)

- oMLX README-level facts (port, model dir, endpoint list, MTP release notes) are
  search-verified only — github.com and omlx.ai were proxy-blocked. The client
  contract is the OpenAI standard regardless; `run.py meetily` + the original
  pipeline's working config corroborate `localhost:8000/v1`.
- sherpa-rs constructor/field names follow its documented examples but couldn't be
  compiled here (crates.io blocked) — flagged in-file; first macOS build is the check.
- An open oMLX issue reports a quality regression in v0.5.0-rc1 oQ4e quants
  **[search-verified, title only]** — pin an oMLX+quant pair and benchmark before
  trusting upgrades.
