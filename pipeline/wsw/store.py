"""Persist pipeline output into the whosaidwhat SQLite database.

Uses the exact schema in /schema.sql (single source of truth shared with the
Rust app). Stdlib-only, so it is fully testable in any sandbox.
"""

from __future__ import annotations

import os
import sqlite3
import time
import uuid

from .chunking import Turn

SCHEMA_PATH = os.path.join(os.path.dirname(__file__), "..", "..", "schema.sql")


def open_store(db_path: str) -> sqlite3.Connection:
    conn = sqlite3.connect(db_path, isolation_level=None)
    conn.execute("PRAGMA foreign_keys=ON")
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA busy_timeout=5000")
    with open(SCHEMA_PATH) as f:
        conn.executescript(f.read())
    return conn


SELF_LABEL = "Me"


def _speaker_id_for(
    conn: sqlite3.Connection, meeting_id: str, label: str, self_row_id: dict
) -> int:
    """Resolve a per-turn speaker label to a speakers.id.

    Speaker identity rules (matching schema.sql's design and the Rust path):
    - the local user ("Me", the mic track) is a single global row, is_self=1,
      reused across meetings and within this one;
    - anonymous diarization labels (SPEAKER_00, ...) are per-meeting: a FRESH
      row per meeting, so meeting A's SPEAKER_00 is never conflated with
      meeting B's. Deduping these by display_name (the naive get-or-create)
      is the cross-meeting-collision bug; we deliberately insert a new row.
      The user renaming one later touches only that row.
    - any other label (a real name the user already assigned) dedups globally.
    """
    if label == SELF_LABEL:
        if self_row_id.get("id") is None:
            row = conn.execute(
                "SELECT id FROM speakers WHERE display_name = ? AND is_self = 1", (label,)
            ).fetchone()
            if row is None:
                conn.execute(
                    "INSERT INTO speakers (display_name, is_self) VALUES (?, 1)", (label,)
                )
                self_row_id["id"] = conn.execute("SELECT last_insert_rowid()").fetchone()[0]
            else:
                self_row_id["id"] = row[0]
        return self_row_id["id"]

    if label.startswith("SPEAKER_"):
        # Fresh per-meeting row; cache within this call so repeated turns of the
        # same anonymous speaker in THIS meeting share one row.
        cache_key = f"__anon__{label}"
        if self_row_id.get(cache_key) is None:
            conn.execute("INSERT INTO speakers (display_name) VALUES (?)", (label,))
            self_row_id[cache_key] = conn.execute("SELECT last_insert_rowid()").fetchone()[0]
        return self_row_id[cache_key]

    # Named speaker: global dedup.
    conn.execute(
        "INSERT INTO speakers (display_name) SELECT ? WHERE NOT EXISTS"
        " (SELECT 1 FROM speakers WHERE display_name = ?)",
        (label, label),
    )
    return conn.execute("SELECT id FROM speakers WHERE display_name = ?", (label,)).fetchone()[0]


def save_meeting(
    conn: sqlite3.Connection,
    title: str,
    turns: list[Turn],
    outline: str,
    notes: str,
    model: str,
    app: str | None = None,
    audio_system_path: str | None = None,
    audio_mic_path: str | None = None,
    started_at: int | None = None,
) -> str:
    """Write meeting + segments + versioned summaries in one transaction.
    Returns the meeting id."""
    meeting_id = str(uuid.uuid4())
    now = int(started_at if started_at is not None else time.time())

    conn.execute("BEGIN")
    try:
        conn.execute(
            "INSERT INTO meetings (id, title, app, started_at, audio_system_path,"
            " audio_mic_path, status) VALUES (?, ?, ?, ?, ?, ?, 'transcribed')",
            (meeting_id, title, app, now, audio_system_path, audio_mic_path),
        )

        speaker_cache: dict = {}
        for turn in turns:
            label = turn.get("speaker") or "Unknown"
            speaker_id = _speaker_id_for(conn, meeting_id, label, speaker_cache)
            # The mic track ("Me") is the local user by construction; everything
            # else came off the system track. Provenance must survive storage
            # (schema.sql segments.source; docs/00 §2).
            source = "mic" if label == SELF_LABEL else "system"
            conn.execute(
                "INSERT INTO segments (meeting_id, speaker_id, source, start_ms, end_ms, text)"
                " VALUES (?, ?, ?, ?, ?, ?)",
                (
                    meeting_id,
                    speaker_id,
                    source,
                    turn.get("start_ms", 0),
                    turn.get("end_ms", turn.get("start_ms", 0)),
                    turn.get("text", ""),
                ),
            )

        for kind, content in (("outline", outline), ("notes", notes)):
            (next_version,) = conn.execute(
                "SELECT COALESCE(MAX(version), 0) + 1 FROM summaries"
                " WHERE meeting_id = ? AND kind = ?",
                (meeting_id, kind),
            ).fetchone()
            conn.execute(
                "INSERT INTO summaries (meeting_id, version, kind, content, model)"
                " VALUES (?, ?, ?, ?, ?)",
                (meeting_id, next_version, kind, content, model),
            )

        conn.execute(
            "UPDATE meetings SET status = 'summarized' WHERE id = ?", (meeting_id,)
        )
        conn.execute("COMMIT")
    except BaseException:
        conn.execute("ROLLBACK")
        raise
    return meeting_id


def fts5_sanitize(query: str) -> str:
    """Turn arbitrary user input into a safe FTS5 MATCH expression.

    Mirrors db::fts5_sanitize in the Rust path: each whitespace token becomes a
    quoted phrase (embedded quotes doubled), so punctuation like `don't` or
    `covid-19` is treated literally instead of raising `fts5: syntax error`.
    Empty input returns "" and the caller short-circuits to no results.
    """
    tokens = query.split()
    return " ".join('"' + t.replace('"', '""') + '"' for t in tokens)


def search(conn: sqlite3.Connection, query: str, limit: int = 20) -> list[dict]:
    """FTS5 transcript search across all stored meetings."""
    match = fts5_sanitize(query)
    if not match:
        return []
    rows = conn.execute(
        "SELECT s.meeting_id, sp.display_name, s.start_ms,"
        " snippet(segments_fts, 0, '[', ']', '…', 12)"
        " FROM segments_fts"
        " JOIN segments s ON s.id = segments_fts.rowid"
        " LEFT JOIN speakers sp ON sp.id = s.speaker_id"
        " WHERE segments_fts MATCH ? ORDER BY rank LIMIT ?",
        (match, limit),
    ).fetchall()
    return [
        {"meeting_id": m, "speaker": sp, "start_ms": ms, "snippet": snip}
        for (m, sp, ms, snip) in rows
    ]
