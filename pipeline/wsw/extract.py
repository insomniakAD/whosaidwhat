"""Structured extraction: ``[mm:ss]`` citation markers and action items.

Mirror of src-tauri/src/llm/extract.rs — same prompt, same parsing semantics,
same resolution tolerance — so the Rust app and this pipeline populate
``summary_citations`` / ``action_items`` identically for identical inputs.
Stdlib-only and pure (the LLM call lives in summarize.py, the DB writes in
store.py), so everything here is testable in any sandbox.

Design notes (rationale in the Rust twin):
- no JSON mode — the stage-4 prompt asks for a strict line format and this
  module parses it defensively; worst case is zero action items, never an
  exception fed back into a prompt;
- markers that resolve to no real segment (hallucinated or rounded beyond
  RESOLVE_TOLERANCE_MS) are dropped, not force-linked.
"""

from __future__ import annotations

import re

ACTION_ITEMS_PROMPT = """\
You are a strict data extraction engine. From the meeting outline below, extract every action item: a concrete task somebody committed to do.
Output one line per action item, exactly in this format, with no other text:
* Owner: task description [mm:ss]

Rules:
- Owner is the speaker's name exactly as it appears in the outline. If no owner is stated, write Unassigned.
- Keep the [mm:ss] timestamp of the moment the task was discussed if the outline shows one; omit it if none is shown.
- Do not invent tasks, owners, or timestamps not present in the outline.
- If there are no action items, output exactly: None"""

# [mm:ss] with an unbounded minute field (the chunk formatter emits [92:11]
# for a 90-minute meeting), or [h:mm:ss] when a model normalizes long offsets.
_MARKER_RE = re.compile(r"\[(\d{1,4}):(\d{2})(?::(\d{2}))?\]")

# How far (ms) a marker may fall outside every segment and still snap to the
# nearest one (formatter truncates to whole seconds; models round).
RESOLVE_TOLERANCE_MS = 10_000


def _marker_ms(match: re.Match) -> int | None:
    """Convert one marker match to milliseconds; None if fields are invalid."""
    a, b, c = match.group(1), match.group(2), match.group(3)
    if c is not None:  # h:mm:ss
        h, m, s = int(a), int(b), int(c)
        if m >= 60 or s >= 60:
            return None
        return ((h * 60 + m) * 60 + s) * 1000
    m, s = int(a), int(b)
    if s >= 60:
        return None
    return (m * 60 + s) * 1000


def parse_timestamps(text: str) -> list[int]:
    """All valid markers in ``text`` as ms offsets, deduplicated in order."""
    out: list[int] = []
    for match in _MARKER_RE.finditer(text):
        ms = _marker_ms(match)
        if ms is not None and ms not in out:
            out.append(ms)
    return out


def resolve_segment(ms: int, segments: list[dict]) -> int | None:
    """Resolve a marker to a segment id.

    ``segments`` rows need ``id``/``start_ms``/``end_ms`` and must be sorted by
    start_ms (the DB query's order). Containment wins; otherwise the nearest
    start within RESOLVE_TOLERANCE_MS; otherwise None (marker dropped).
    """
    for seg in segments:
        if seg["start_ms"] <= ms <= seg["end_ms"]:
            return seg["id"]
    best = None
    for seg in segments:
        d = abs(seg["start_ms"] - ms)
        if best is None or d < best[0]:
            best = (d, seg["id"])
    if best is not None and best[0] <= RESOLVE_TOLERANCE_MS:
        return best[1]
    return None


def parse_action_items(response: str) -> list[dict]:
    """Parse the strict stage-4 line format into
    ``{"owner": str|None, "text": str, "ts_ms": int|None}`` dicts.

    Tolerates the bullet variants models actually produce (``*``/``-``/``•``),
    stage-1-style bracketed owners (``* [Kim]: ...``), "None" responses, and
    stray prose (skipped). Never raises.
    """
    items: list[dict] = []
    for raw in response.splitlines():
        line = raw.strip()
        rest = None
        for bullet in ("* ", "- ", "• "):
            if line.startswith(bullet):
                rest = line[len(bullet):]
                break
        if rest is None:
            continue
        owner_part, sep, task_part = rest.partition(":")
        if not sep:
            continue
        owner_raw = owner_part.strip().strip("[]").strip()
        text = task_part.strip()
        if not owner_raw or not text:
            continue
        ts_list = parse_timestamps(text)
        ts_ms = ts_list[-1] if ts_list else None
        # Strip a trailing marker from the display text (keep it as data).
        trailing = _MARKER_RE.search(text)
        while trailing is not None:
            nxt = _MARKER_RE.search(text, trailing.end())
            if nxt is None:
                break
            trailing = nxt
        if trailing is not None and trailing.end() == len(text) and _marker_ms(trailing) is not None:
            text = text[: trailing.start()].rstrip()
        if not text:
            continue
        owner = None if owner_raw.lower() == "unassigned" else owner_raw
        items.append({"owner": owner, "text": text, "ts_ms": ts_ms})
    return items


def quote_snippet(text: str, max_chars: int = 240) -> str:
    """Citation quote bound (mirror of pipeline::quote_snippet in Rust)."""
    if len(text) <= max_chars:
        return text
    return text[:max_chars].rstrip() + "…"
