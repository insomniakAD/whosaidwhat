"""Diarization stage: audio file -> speaker segments, fully local.

Engine: pyannote ``speaker-diarization-community-1`` (pyannote.audio 4.x) —
the open-source accuracy reference (DER 17.0% AMI-IHM vs 18.8% for legacy 3.1,
per the official model card). CC-BY-4.0; an ungated mirror exists at
``pyannote-community/speaker-diarization-community-1`` so no HF token is
strictly required.

Apple Silicon notes (evidence in docs/04-diarization-evaluation.md):
- MPS (Apple GPU) works via ``pipeline.to(torch.device("mps"))`` but is
  community-supported, with a history of timestamp bugs — so MPS is opt-in
  here (``WSW_DIARIZE_DEVICE=mps``) and CPU is the default. The segmentation +
  embedding models are small; CPU on M-series is minutes-per-hour-of-audio.
- The "exclusive" (non-overlapping) output is what transcript alignment
  needs; community-1 exposes it directly.

Output shape: [{"start_ms", "end_ms", "speaker"}], chronological,
speakers labeled SPEAKER_00, SPEAKER_01, ... per meeting.
"""

from __future__ import annotations

import os

DEFAULT_PIPELINE = os.environ.get(
    "WSW_DIARIZE_MODEL",
    # Ungated mirror of pyannote/speaker-diarization-community-1 (CC-BY-4.0).
    "pyannote-community/speaker-diarization-community-1",
)


def diarize(
    audio_path: str,
    num_speakers: int | None = None,
    hf_token: str | None = None,
) -> list[dict]:
    """Run local diarization. ``num_speakers`` pins the count when known
    (e.g. a 1:1 call is exactly 2) — clustering with a known count is both
    faster and more accurate."""
    try:
        from pyannote.audio import Pipeline  # type: ignore
    except ImportError as e:
        raise RuntimeError(
            "pyannote.audio not installed. Run: pip install pyannote.audio"
        ) from e

    token = hf_token or os.environ.get("HF_TOKEN")
    pipeline = Pipeline.from_pretrained(DEFAULT_PIPELINE, token=token)

    device = os.environ.get("WSW_DIARIZE_DEVICE", "cpu")
    if device != "cpu":
        import torch  # type: ignore

        pipeline.to(torch.device(device))

    kwargs = {}
    if num_speakers:
        kwargs["num_speakers"] = num_speakers

    output = pipeline(audio_path, **kwargs)

    # pyannote.audio 4.x returns an object whose .speaker_diarization is the
    # annotation; 3.x returned the annotation directly. Support both.
    annotation = getattr(output, "speaker_diarization", output)

    # Prefer the exclusive (overlap-resolved) view when the pipeline provides
    # it — that is the right input for transcript alignment.
    exclusive = getattr(output, "exclusive_speaker_diarization", None)
    if exclusive is not None:
        annotation = exclusive

    segments = [
        {
            "start_ms": int(turn.start * 1000),
            "end_ms": int(turn.end * 1000),
            "speaker": str(label),
        }
        for turn, _track, label in annotation.itertracks(yield_label=True)
    ]
    segments.sort(key=lambda s: s["start_ms"])
    return segments
