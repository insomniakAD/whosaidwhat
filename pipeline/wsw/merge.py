"""Merge ASR segments with diarization segments into speaker-attributed turns.

Pure stdlib — mirror of the Rust implementation in
src-tauri/src/diarize/merge.rs; both are covered by equivalent tests so the
two paths cannot silently diverge.
"""

from __future__ import annotations

from .chunking import Turn


def _overlap_ms(a_start: int, a_end: int, b_start: int, b_end: int) -> int:
    return max(0, min(a_end, b_end) - max(a_start, b_start))


def attribute_speakers(
    asr_segments: list[dict],
    speaker_segments: list[dict],
    fallback_speaker: str = "SPEAKER_XX",
) -> list[Turn]:
    """Assign each ASR segment the speaker with the largest time overlap.
    Ties break to the earlier speaker segment; zero overlap -> fallback."""
    turns: list[Turn] = []
    for seg in asr_segments:
        best_speaker = None
        best_overlap = 0
        for sp in speaker_segments:
            ov = _overlap_ms(seg["start_ms"], seg["end_ms"], sp["start_ms"], sp["end_ms"])
            if ov > best_overlap:
                best_overlap = ov
                best_speaker = sp["speaker"]
        turns.append(
            Turn(
                speaker=best_speaker or fallback_speaker,
                text=seg["text"],
                start_ms=seg["start_ms"],
                end_ms=seg["end_ms"],
            )
        )
    return turns


def coalesce_turns(turns: list[Turn], max_gap_ms: int = 1500) -> list[Turn]:
    """Merge consecutive same-speaker turns separated by at most max_gap_ms."""
    out: list[Turn] = []
    for turn in turns:
        if (
            out
            and out[-1]["speaker"] == turn["speaker"]
            and turn["start_ms"] - out[-1]["end_ms"] <= max_gap_ms
        ):
            out[-1]["text"] = f"{out[-1]['text']} {turn['text']}"
            out[-1]["end_ms"] = turn["end_ms"]
        else:
            out.append(dict(turn))  # copy so callers' lists stay untouched
    return out


def interleave(local: list[Turn], remote: list[Turn]) -> list[Turn]:
    """Zip two chronologically sorted turn lists; ties go to local first."""
    out: list[Turn] = []
    i = j = 0
    while i < len(local) and j < len(remote):
        if local[i]["start_ms"] <= remote[j]["start_ms"]:
            out.append(local[i])
            i += 1
        else:
            out.append(remote[j])
            j += 1
    out.extend(local[i:])
    out.extend(remote[j:])
    return out
