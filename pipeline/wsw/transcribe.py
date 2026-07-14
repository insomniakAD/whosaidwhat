"""ASR stage: audio file -> timestamped segments. Apple Silicon-first.

Engine order (rationale in docs/04-diarization-evaluation.md):

1. ``mlx-whisper`` — Whisper on Apple's Metal GPU via MLX. The fastest
   maintained Python path on M-series; faster-whisper/CTranslate2 (what
   WhisperX uses) has no reliable MPS backend and falls back to CPU.
2. ``faster-whisper`` — CPU fallback, still respectable on M-series
   performance cores, and the only option off-Mac.

Both return the same shape: [{"start_ms", "end_ms", "text"}].
"""

from __future__ import annotations

DEFAULT_MODEL_MLX = "mlx-community/whisper-large-v3-turbo"
DEFAULT_MODEL_FW = "large-v3-turbo"


def transcribe(audio_path: str, language: str | None = None) -> list[dict]:
    """Transcribe with the best locally available engine."""
    try:
        return _transcribe_mlx(audio_path, language)
    except ImportError:
        pass
    try:
        return _transcribe_faster_whisper(audio_path, language)
    except ImportError as e:
        raise RuntimeError(
            "No ASR engine installed. Run: pip install mlx-whisper  (Apple Silicon) "
            "or: pip install faster-whisper"
        ) from e


def _transcribe_mlx(audio_path: str, language: str | None) -> list[dict]:
    import mlx_whisper  # type: ignore

    result = mlx_whisper.transcribe(
        audio_path,
        path_or_hf_repo=DEFAULT_MODEL_MLX,
        language=language,
        word_timestamps=True,
    )
    segments = []
    for seg in result.get("segments", []):
        text = (seg.get("text") or "").strip()
        if not text:
            continue
        segments.append(
            {
                "start_ms": int(float(seg["start"]) * 1000),
                "end_ms": int(float(seg["end"]) * 1000),
                "text": text,
            }
        )
    return segments


def _transcribe_faster_whisper(audio_path: str, language: str | None) -> list[dict]:
    from faster_whisper import WhisperModel  # type: ignore

    model = WhisperModel(DEFAULT_MODEL_FW, device="cpu", compute_type="int8")
    raw_segments, _info = model.transcribe(audio_path, language=language, vad_filter=True)
    segments = []
    for seg in raw_segments:
        text = seg.text.strip()
        if not text:
            continue
        segments.append(
            {
                "start_ms": int(seg.start * 1000),
                "end_ms": int(seg.end * 1000),
                "text": text,
            }
        )
    return segments
