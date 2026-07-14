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

        for turn in turns:
            speaker = turn.get("speaker") or "Unknown"
            conn.execute(
                "INSERT INTO speakers (display_name) SELECT ? WHERE NOT EXISTS"
                " (SELECT 1 FROM speakers WHERE display_name = ?)",
                (speaker, speaker),
            )
            (speaker_id,) = conn.execute(
                "SELECT id FROM speakers WHERE display_name = ?", (speaker,)
            ).fetchone()
            conn.execute(
                "INSERT INTO segments (meeting_id, speaker_id, start_ms, end_ms, text)"
                " VALUES (?, ?, ?, ?, ?)",
                (
                    meeting_id,
                    speaker_id,
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


def search(conn: sqlite3.Connection, query: str, limit: int = 20) -> list[dict]:
    """FTS5 transcript search across all stored meetings."""
    rows = conn.execute(
        "SELECT s.meeting_id, sp.display_name, s.start_ms,"
        " snippet(segments_fts, 0, '[', ']', '…', 12)"
        " FROM segments_fts"
        " JOIN segments s ON s.id = segments_fts.rowid"
        " LEFT JOIN speakers sp ON sp.id = s.speaker_id"
        " WHERE segments_fts MATCH ? ORDER BY rank LIMIT ?",
        (query, limit),
    ).fetchall()
    return [
        {"meeting_id": m, "speaker": sp, "start_ms": ms, "snippet": snip}
        for (m, sp, ms, snip) in rows
    ]
