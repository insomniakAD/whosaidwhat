"""Read transcripts out of Meetily CE's local SQLite database.

This is the repaired and hardened successor of the original
``meetily_db_extractor.py`` (which crashed in production because
``pipeline.py`` imported it under the wrong module name — see
reference/meetily_pipeline/summarizer_error.log). Changes:

- importable under a stable name (``wsw.meetily_source``);
- DB path is a parameter/env var, not a hardcoded user path;
- read-only connection URI so we can never corrupt Meetily's DB, even if
  Meetily is writing at the same time;
- the "already processed" bookkeeping moved out of the extractor into the
  caller (a data reader silently refusing to return data based on a side-file
  was a debugging trap);
- schema drift (Meetily renaming columns between releases) raises a clear
  error listing the actual schema instead of a bare sqlite3 exception.
"""

from __future__ import annotations

import os
import sqlite3
from .chunking import Turn

DEFAULT_DB_PATH = os.environ.get(
    "MEETILY_DB",
    os.path.expanduser(
        "~/Library/Application Support/com.meetily.ai/meeting_minutes.sqlite"
    ),
)


class SchemaMismatch(RuntimeError):
    """Meetily's schema does not match what this reader expects."""


def _connect_readonly(db_path: str) -> sqlite3.Connection:
    if not os.path.exists(db_path):
        raise FileNotFoundError(f"Meetily database not found at {db_path}")
    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row
    return conn


def describe_schema(db_path: str = DEFAULT_DB_PATH) -> dict[str, list[str]]:
    """Return {table: [column, ...]} for diagnostics and drift detection."""
    conn = _connect_readonly(db_path)
    try:
        tables = [
            r[0]
            for r in conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"
            )
        ]
        return {
            t: [c[1] for c in conn.execute(f"PRAGMA table_info({t})")] for t in tables
        }
    finally:
        conn.close()


def _require(schema: dict[str, list[str]], table: str, columns: list[str]) -> None:
    if table not in schema:
        raise SchemaMismatch(
            f"expected table '{table}' not found; actual schema: {schema}"
        )
    missing = [c for c in columns if c not in schema[table]]
    if missing:
        raise SchemaMismatch(
            f"table '{table}' is missing columns {missing}; has {schema[table]}"
        )


def get_latest_meeting(db_path: str = DEFAULT_DB_PATH) -> dict | None:
    """Return {'id', 'title'} of the most recent meeting, or None if empty."""
    schema = describe_schema(db_path)
    _require(schema, "meetings", ["id", "title", "created_at"])
    conn = _connect_readonly(db_path)
    try:
        row = conn.execute(
            "SELECT id, title FROM meetings ORDER BY created_at DESC LIMIT 1"
        ).fetchone()
        if row is None:
            return None
        return {"id": row["id"], "title": row["title"] or "Untitled"}
    finally:
        conn.close()


def get_transcript(meeting_id: str, db_path: str = DEFAULT_DB_PATH) -> list[Turn]:
    """Return the ordered speaker turns for one meeting."""
    schema = describe_schema(db_path)
    _require(schema, "transcripts", ["meeting_id", "transcript", "timestamp"])
    has_speaker = "speaker" in schema["transcripts"]

    conn = _connect_readonly(db_path)
    try:
        speaker_col = "speaker" if has_speaker else "NULL AS speaker"
        rows = conn.execute(
            f"SELECT {speaker_col}, transcript FROM transcripts "
            "WHERE meeting_id = ? ORDER BY timestamp ASC",
            (meeting_id,),
        ).fetchall()
        # Meetily stores no per-turn millisecond offsets, so we synthesize a
        # monotonically increasing sequence (1 s apart) purely to PRESERVE ORDER
        # in whosaidwhat's segments table (which renders ORDER BY start_ms). These
        # are ordering keys, NOT real audio positions — audio deep-links are
        # therefore unavailable for Meetily-imported meetings (documented in
        # docs/04). A future improvement is parsing Meetily's timestamp column
        # into real offsets, but its format is not guaranteed across releases.
        return [
            Turn(
                speaker=(row["speaker"] or "Unknown"),
                text=row["transcript"] or "",
                start_ms=i * 1000,
                end_ms=i * 1000,
            )
            for i, row in enumerate(rows)
        ]
    finally:
        conn.close()


def get_latest_transcript(db_path: str = DEFAULT_DB_PATH) -> tuple[dict | None, list[Turn]]:
    """Convenience: (meeting, turns) for the newest meeting; (None, []) if empty."""
    meeting = get_latest_meeting(db_path)
    if meeting is None:
        return None, []
    return meeting, get_transcript(meeting["id"], db_path)
