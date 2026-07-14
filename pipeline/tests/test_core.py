"""Stdlib-only tests for the pipeline's pure-logic modules.

Run with:  python3 -m unittest discover pipeline/tests
No third-party packages required, so these run in any sandbox.
"""

from __future__ import annotations

import os
import sqlite3
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from wsw.chunking import chunk_transcript, format_chunk  # noqa: E402
from wsw import meetily_source  # noqa: E402


def words(n: int) -> str:
    return " ".join(["word"] * n)


class ChunkingTests(unittest.TestCase):
    def test_single_chunk(self):
        turns = [{"speaker": "A", "text": words(100)}, {"speaker": "B", "text": words(100)}]
        self.assertEqual(len(chunk_transcript(turns, max_words=1200)), 1)

    def test_split_with_overlap_carryover(self):
        turns = [{"speaker": f"S{i}", "text": words(400)} for i in range(5)]
        chunks = chunk_transcript(turns, max_words=1000, overlap_turns=1)
        self.assertGreaterEqual(len(chunks), 3)
        for prev, nxt in zip(chunks, chunks[1:]):
            self.assertEqual(prev[-1], nxt[0], "overlap turn must carry over")
        flat = [t["speaker"] for c in chunks for t in c]
        for i in range(5):
            self.assertIn(f"S{i}", flat)

    def test_oversized_turn_not_split(self):
        turns = [
            {"speaker": "A", "text": words(50)},
            {"speaker": "B", "text": words(5000)},
            {"speaker": "C", "text": words(50)},
        ]
        chunks = chunk_transcript(turns, max_words=1200)
        flat = [t["speaker"] for c in chunks for t in c]
        self.assertIn("B", flat)

    def test_empty(self):
        self.assertEqual(chunk_transcript([]), [])

    def test_format_with_and_without_timestamps(self):
        with_ts = format_chunk([{"speaker": "Ada", "text": "hi", "start_ms": 65000}])
        self.assertEqual(with_ts, "[01:05] Ada: hi")
        without_ts = format_chunk([{"speaker": "Ada", "text": "hi"}])
        self.assertEqual(without_ts, "Ada: hi")


class MeetilySourceTests(unittest.TestCase):
    def setUp(self):
        fd, self.db_path = tempfile.mkstemp(suffix=".sqlite")
        os.close(fd)
        conn = sqlite3.connect(self.db_path)
        conn.executescript(
            """
            CREATE TABLE meetings (id TEXT PRIMARY KEY, title TEXT, created_at TEXT);
            CREATE TABLE transcripts (
                id INTEGER PRIMARY KEY,
                meeting_id TEXT,
                speaker TEXT,
                transcript TEXT,
                timestamp TEXT
            );
            INSERT INTO meetings VALUES
                ('m1', 'Old meeting', '2026-07-01T10:00:00'),
                ('m2', 'New meeting', '2026-07-14T09:00:00');
            INSERT INTO transcripts (meeting_id, speaker, transcript, timestamp) VALUES
                ('m2', 'Alice', 'first turn', '2026-07-14T09:00:01'),
                ('m2', NULL,    'second turn', '2026-07-14T09:00:05'),
                ('m1', 'Bob',   'old turn', '2026-07-01T10:00:01');
            """
        )
        conn.commit()
        conn.close()

    def tearDown(self):
        os.unlink(self.db_path)

    def test_latest_meeting_and_turn_order(self):
        meeting, turns = meetily_source.get_latest_transcript(self.db_path)
        self.assertEqual(meeting["id"], "m2")
        self.assertEqual(meeting["title"], "New meeting")
        self.assertEqual([t["text"] for t in turns], ["first turn", "second turn"])
        self.assertEqual(turns[1]["speaker"], "Unknown", "NULL speaker mapped")

    def test_readonly_connection(self):
        conn = meetily_source._connect_readonly(self.db_path)
        with self.assertRaises(sqlite3.OperationalError):
            conn.execute("INSERT INTO meetings VALUES ('x', 'x', 'x')")
        conn.close()

    def test_schema_mismatch_is_explicit(self):
        conn = sqlite3.connect(self.db_path)
        conn.executescript("ALTER TABLE transcripts RENAME COLUMN transcript TO body;")
        conn.commit()
        conn.close()
        with self.assertRaises(meetily_source.SchemaMismatch) as ctx:
            meetily_source.get_transcript("m2", self.db_path)
        self.assertIn("transcript", str(ctx.exception))

    def test_missing_db_raises_filenotfound(self):
        with self.assertRaises(FileNotFoundError):
            meetily_source.get_latest_meeting("/nonexistent/path.sqlite")

    def test_empty_db_returns_none(self):
        conn = sqlite3.connect(self.db_path)
        conn.execute("DELETE FROM meetings")
        conn.commit()
        conn.close()
        meeting, turns = meetily_source.get_latest_transcript(self.db_path)
        self.assertIsNone(meeting)
        self.assertEqual(turns, [])


if __name__ == "__main__":
    unittest.main()
