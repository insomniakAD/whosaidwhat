"""Tests for wsw.extract (marker parsing, resolution, action items) and the
store.save_structured_extraction round trip — stdlib only, in-memory SQLite.

These mirror the Rust unit tests in src-tauri/src/llm/extract.rs so a semantic
drift between the two implementations shows up as a failing test on whichever
side changed.
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from wsw import extract, store  # noqa: E402
from wsw.chunking import Turn  # noqa: E402


class TestParseTimestamps(unittest.TestCase):
    def test_mm_ss_and_h_mm_ss(self):
        text = "Decision at [01:05], revisited [92:11] and again at [1:32:07]."
        self.assertEqual(
            extract.parse_timestamps(text),
            [65_000, (92 * 60 + 11) * 1000, ((1 * 60 + 32) * 60 + 7) * 1000],
        )

    def test_ignores_non_markers_and_dedups(self):
        text = "[TODO] fix [01:05] later; see [01:05] and [notes] and [12:99]."
        self.assertEqual(extract.parse_timestamps(text), [65_000])

    def test_boundaries_and_empty(self):
        self.assertEqual(extract.parse_timestamps("[00:00]"), [0])
        self.assertEqual(extract.parse_timestamps(""), [])
        self.assertEqual(extract.parse_timestamps("broken [12:3"), [])


class TestResolveSegment(unittest.TestCase):
    SEGS = [
        {"id": 1, "start_ms": 0, "end_ms": 4_000},
        {"id": 2, "start_ms": 5_000, "end_ms": 9_000},
    ]

    def test_containment(self):
        self.assertEqual(extract.resolve_segment(6_000, self.SEGS), 2)
        self.assertEqual(extract.resolve_segment(0, self.SEGS), 1)
        self.assertEqual(extract.resolve_segment(4_000, self.SEGS), 1)

    def test_snaps_within_tolerance(self):
        segs = [
            {"id": 1, "start_ms": 0, "end_ms": 60_000},
            {"id": 2, "start_ms": 65_400, "end_ms": 80_000},
        ]
        self.assertEqual(extract.resolve_segment(65_000, segs), 2)

    def test_drops_beyond_tolerance(self):
        segs = [{"id": 1, "start_ms": 0, "end_ms": 4_000}]
        self.assertEqual(extract.resolve_segment(8_000, segs), 1)
        self.assertIsNone(extract.resolve_segment(120_000, segs))
        self.assertIsNone(extract.resolve_segment(0, []))


class TestParseActionItems(unittest.TestCase):
    def test_owner_timestamp_variants(self):
        response = (
            "* Sarah: send the revised budget to finance [12:41]\n"
            "- Me: file the DER benchmark issue\n"
            "• Unassigned: book a room for the offsite [01:05]\n"
            "Some stray narration the model added.\n"
            "* : broken line skipped\n"
            "* NoTask:"
        )
        items = extract.parse_action_items(response)
        self.assertEqual(len(items), 3)
        self.assertEqual(
            items[0],
            {
                "owner": "Sarah",
                "text": "send the revised budget to finance",
                "ts_ms": (12 * 60 + 41) * 1000,
            },
        )
        self.assertEqual(items[1]["owner"], "Me")
        self.assertIsNone(items[1]["ts_ms"])
        self.assertIsNone(items[2]["owner"])
        self.assertEqual(items[2]["text"], "book a room for the offsite")

    def test_none_and_empty(self):
        self.assertEqual(extract.parse_action_items("None"), [])
        self.assertEqual(extract.parse_action_items(""), [])

    def test_only_a_truly_trailing_marker_is_stripped(self):
        # Twin parity with llm::extract.rs: a mid-text marker before a stray
        # ']' is NOT trailing, so the text is kept verbatim.
        items = extract.parse_action_items("* Owner: do the thing [12:41] more]")
        self.assertEqual(items[0]["text"], "do the thing [12:41] more]")
        self.assertEqual(items[0]["ts_ms"], (12 * 60 + 41) * 1000)
        t = extract.parse_action_items("* Owner: ship it [02:00]")
        self.assertEqual(t[0]["text"], "ship it")
        two = extract.parse_action_items("* Owner: foo [01:00] bar [02:00]")
        self.assertEqual(two[0]["text"], "foo [01:00] bar")
        self.assertEqual(two[0]["ts_ms"], 120_000)

    def test_bracketed_owner(self):
        items = extract.parse_action_items("* [Kim]: draft the rollout plan [03:00]")
        self.assertEqual(items[0]["owner"], "Kim")
        self.assertEqual(items[0]["text"], "draft the rollout plan")
        self.assertEqual(items[0]["ts_ms"], 180_000)

    def test_quote_snippet_bounds(self):
        self.assertEqual(extract.quote_snippet("short"), "short")
        long = "x" * 400
        q = extract.quote_snippet(long)
        self.assertTrue(q.endswith("…"))
        self.assertEqual(len(q), 241)


class TestSaveStructuredExtraction(unittest.TestCase):
    def _meeting(self, conn):
        turns = [
            Turn(speaker="Me", text="I will send the budget", start_ms=0, end_ms=4_000),
            Turn(
                speaker="SPEAKER_00",
                text="shipping moves to friday",
                start_ms=65_000,
                end_ms=70_000,
            ),
        ]
        return store.save_meeting(
            conn,
            title="Standup",
            turns=turns,
            outline="- shipping moves [01:05]\n- budget [00:00]",
            notes="## Notes\nShipping moved to Friday [01:05]. Hallucinated [59:59].",
            model="qwen",
        )

    def test_citations_and_action_items(self):
        conn = store.open_store(":memory:")
        meeting_id = self._meeting(conn)
        response = "* Me: send the budget [00:00]\n* Unassigned: confirm friday date"
        counts = store.save_structured_extraction(conn, meeting_id, response)

        # notes: [01:05] resolves, [59:59] dropped; outline: both resolve.
        self.assertEqual(counts["citations"], 3)
        self.assertEqual(counts["action_items"], 2)

        rows = conn.execute(
            "SELECT c.segment_id, c.quote FROM summary_citations c"
            " JOIN summaries s ON s.id = c.summary_id WHERE s.kind = 'notes'"
        ).fetchall()
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0][1], "shipping moves to friday")

        items = conn.execute(
            "SELECT a.text, sp.display_name FROM action_items a"
            " LEFT JOIN speakers sp ON sp.id = a.speaker_id ORDER BY a.id"
        ).fetchall()
        self.assertEqual(items[0], ("send the budget", "Me"))
        self.assertEqual(items[1], ("confirm friday date", None))

    def test_owner_resolution_is_meeting_scoped(self):
        conn = store.open_store(":memory:")
        m1 = self._meeting(conn)
        self.assertIsNotNone(store.speaker_in_meeting(conn, m1, "Me"))
        self.assertIsNone(store.speaker_in_meeting(conn, "no-such-meeting", "Me"))
        # An owner name with no speaker row in this meeting stores NULL.
        counts = store.save_structured_extraction(conn, m1, "* Zoe: unknowable owner")
        self.assertEqual(counts["action_items"], 1)
        row = conn.execute(
            "SELECT speaker_id FROM action_items WHERE text = 'unknowable owner'"
        ).fetchone()
        self.assertIsNone(row[0])

    def test_no_response_writes_citations_only(self):
        conn = store.open_store(":memory:")
        meeting_id = self._meeting(conn)
        counts = store.save_structured_extraction(conn, meeting_id, None)
        self.assertEqual(counts["action_items"], 0)
        self.assertEqual(counts["citations"], 3)


if __name__ == "__main__":
    unittest.main()
