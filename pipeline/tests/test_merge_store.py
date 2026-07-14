"""Stdlib-only tests for merge.py and store.py (mirrors the Rust merge tests)."""

from __future__ import annotations

import os
import sqlite3
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from wsw import merge, store  # noqa: E402
from wsw.chunking import Turn  # noqa: E402


def asr(start, end, text):
    return {"start_ms": start, "end_ms": end, "text": text}


def spk(start, end, who):
    return {"start_ms": start, "end_ms": end, "speaker": who}


class MergeTests(unittest.TestCase):
    def test_majority_overlap_wins(self):
        turns = merge.attribute_speakers(
            [asr(0, 1000, "hello there")],
            [spk(0, 300, "SPEAKER_00"), spk(300, 1000, "SPEAKER_01")],
        )
        self.assertEqual(turns[0]["speaker"], "SPEAKER_01")

    def test_no_overlap_falls_back(self):
        turns = merge.attribute_speakers([asr(5000, 6000, "hm")], [spk(0, 1000, "A")])
        self.assertEqual(turns[0]["speaker"], "SPEAKER_XX")

    def test_tie_breaks_to_earlier(self):
        turns = merge.attribute_speakers(
            [asr(0, 1000, "even split")],
            [spk(0, 500, "SPEAKER_00"), spk(500, 1000, "SPEAKER_01")],
        )
        self.assertEqual(turns[0]["speaker"], "SPEAKER_00")

    def test_coalesce_and_gap(self):
        turns = [
            Turn(speaker="A", text="one", start_ms=0, end_ms=900),
            Turn(speaker="A", text="two", start_ms=1100, end_ms=2000),
            Turn(speaker="B", text="reply", start_ms=2100, end_ms=3000),
            Turn(speaker="A", text="later", start_ms=300000, end_ms=301000),
        ]
        merged = merge.coalesce_turns(turns, max_gap_ms=1000)
        self.assertEqual(len(merged), 3)
        self.assertEqual(merged[0]["text"], "one two")
        # input list untouched
        self.assertEqual(turns[0]["text"], "one")

    def test_interleave_chronological_local_first_on_tie(self):
        local = [Turn(speaker="Me", text="q", start_ms=0, end_ms=1000)]
        remote = [Turn(speaker="S0", text="a", start_ms=0, end_ms=1000)]
        out = merge.interleave(local, remote)
        self.assertEqual([t["text"] for t in out], ["q", "a"])


class StoreTests(unittest.TestCase):
    def setUp(self):
        fd, self.db_path = tempfile.mkstemp(suffix=".sqlite")
        os.close(fd)
        os.unlink(self.db_path)  # open_store creates it

    def tearDown(self):
        for suffix in ("", "-wal", "-shm"):
            path = self.db_path + suffix
            if os.path.exists(path):
                os.unlink(path)

    def test_save_and_search_roundtrip(self):
        conn = store.open_store(self.db_path)
        turns = [
            Turn(speaker="Me", text="we approved the budget", start_ms=0, end_ms=2000),
            Turn(speaker="SPEAKER_00", text="deadline moves to friday", start_ms=2000, end_ms=5000),
        ]
        meeting_id = store.save_meeting(
            conn, "Standup", turns, "## outline", "## notes", "test-model", app="zoom"
        )
        hits = store.search(conn, "budget")
        self.assertEqual(len(hits), 1)
        self.assertEqual(hits[0]["meeting_id"], meeting_id)
        self.assertEqual(hits[0]["speaker"], "Me")

        (status,) = conn.execute(
            "SELECT status FROM meetings WHERE id = ?", (meeting_id,)
        ).fetchone()
        self.assertEqual(status, "summarized")

        kinds = {
            k for (k,) in conn.execute(
                "SELECT kind FROM summaries WHERE meeting_id = ?", (meeting_id,)
            )
        }
        self.assertEqual(kinds, {"outline", "notes"})
        conn.close()

    def test_transaction_rolls_back_on_failure(self):
        conn = store.open_store(self.db_path)
        bad_turns = [Turn(speaker="A", text="ok", start_ms=100, end_ms=50)]  # violates CHECK
        with self.assertRaises(sqlite3.IntegrityError):
            store.save_meeting(conn, "Broken", bad_turns, "o", "n", "m")
        (count,) = conn.execute("SELECT COUNT(*) FROM meetings").fetchone()
        self.assertEqual(count, 0, "failed save must leave no partial meeting")
        conn.close()

    def test_two_meetings_version_independently(self):
        conn = store.open_store(self.db_path)
        t = [Turn(speaker="A", text="x", start_ms=0, end_ms=10)]
        m1 = store.save_meeting(conn, "One", t, "o", "n", "m")
        m2 = store.save_meeting(conn, "Two", t, "o", "n", "m")
        for m in (m1, m2):
            (v,) = conn.execute(
                "SELECT MAX(version) FROM summaries WHERE meeting_id=? AND kind='notes'",
                (m,),
            ).fetchone()
            self.assertEqual(v, 1)
        conn.close()


if __name__ == "__main__":
    unittest.main()
