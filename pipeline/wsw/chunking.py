"""Transcript chunking for the map stage of summarization.

Same semantics as the original pipeline.py chunker (word budget + turn overlap),
kept in lockstep with the Rust port in src-tauri/src/llm/chunk.rs: identical
inputs must produce identical chunk boundaries in both implementations.
"""

from __future__ import annotations

from typing import TypedDict


class Turn(TypedDict, total=False):
    speaker: str
    text: str
    start_ms: int
    end_ms: int


def chunk_transcript(
    turns: list[Turn], max_words: int = 1200, overlap_turns: int = 2
) -> list[list[Turn]]:
    """Split turns into chunks of at most ``max_words``, carrying the last
    ``overlap_turns`` turns into the next chunk for context continuity.

    Turns are never split; every turn appears in at least one chunk.
    """
    chunks: list[list[Turn]] = []
    current: list[Turn] = []
    current_words = 0

    for turn in turns:
        turn_words = len(turn.get("text", "").split())

        if current_words + turn_words > max_words and current:
            chunks.append(current)
            overlap = min(overlap_turns, len(current))
            current = current[-overlap:] if overlap > 0 else []
            current_words = sum(len(t.get("text", "").split()) for t in current)

        current.append(turn)
        current_words += turn_words

    if current:
        chunks.append(current)

    return chunks


def format_chunk(chunk: list[Turn]) -> str:
    """Render a chunk as ``[mm:ss] Speaker: text`` paragraphs for the LLM.

    Timestamps let the model cite moments; they are omitted when a turn has
    no timing (e.g. transcripts imported from Meetily CE, which stores none
    at turn level).
    """
    lines = []
    for turn in chunk:
        prefix = ""
        if "start_ms" in turn:
            secs = int(turn["start_ms"]) // 1000
            prefix = f"[{secs // 60:02d}:{secs % 60:02d}] "
        lines.append(f"{prefix}{turn.get('speaker', 'Unknown')}: {turn.get('text', '')}")
    return "\n\n".join(lines).strip()
