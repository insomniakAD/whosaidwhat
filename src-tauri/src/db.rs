//! SQLite store. Schema lives in /schema.sql (single source of truth, shared
//! with the Python pipeline and verified by executable tests there and here).
//!
//! Connection discipline: WAL journal + NORMAL sync (durable enough on a
//! local disk, far fewer fsyncs), foreign keys ON per connection (SQLite
//! default is OFF), busy_timeout so the Tauri UI reader never sees SQLITE_BUSY
//! from the pipeline writer.

use rusqlite::{params, Connection};
use std::path::Path;

use crate::llm::chunk::Turn;

pub const SCHEMA_SQL: &str = include_str!("../../schema.sql");

/// Turn arbitrary user input into a safe FTS5 MATCH expression.
///
/// FTS5's MATCH argument is a query language: bare `'`, `"`, `-`, `*`, `(`,
/// `:`, and keywords like `NEAR`/`AND`/`OR` are syntax, so a user typing
/// `don't` or `covid-19` triggers `fts5: syntax error` and the query fails.
/// We split on whitespace and wrap each token as a quoted phrase (doubling
/// embedded quotes), which makes every token a literal term ANDed together —
/// the intuitive "results containing all these words" behavior. Empty input
/// yields a query that matches nothing rather than erroring.
pub fn fts5_sanitize(input: &str) -> String {
    let tokens: Vec<String> = input
        .split_whitespace()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    tokens.join(" ")
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub struct Store {
    conn: Connection,
}

/// A transcript search hit (FTS5 snippet + jump-to-audio position).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SearchHit {
    pub meeting_id: String,
    pub speaker: Option<String>,
    pub start_ms: u64,
    pub snippet: String,
}

/// One transcript segment with DB identity (citation resolution + UI).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SegmentRow {
    pub id: i64,
    pub speaker_id: Option<i64>,
    pub speaker: String,
    pub source: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MeetingRow {
    pub id: String,
    pub title: String,
    pub app: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CitationRow {
    pub segment_id: i64,
    pub quote: Option<String>,
    pub start_ms: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SummaryRow {
    pub id: i64,
    pub version: i64,
    pub kind: String,
    pub content: String,
    pub model: String,
    pub model_was_fallback: bool,
    pub citations: Vec<CitationRow>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ActionItemRow {
    pub id: i64,
    pub text: String,
    pub done: bool,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SpeakerStat {
    pub speaker_id: i64,
    pub display_name: String,
    pub is_self: bool,
    pub talk_ms: u64,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(Store { conn })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(Store { conn })
    }

    pub fn create_meeting(
        &mut self,
        id: &str,
        title: &str,
        app: Option<&str>,
        started_at_epoch_s: i64,
    ) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO meetings (id, title, app, started_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, title, app, started_at_epoch_s],
        )?;
        Ok(())
    }

    pub fn set_meeting_audio(
        &mut self,
        id: &str,
        system_path: Option<&str>,
        mic_path: Option<&str>,
        ended_at_epoch_s: i64,
    ) -> Result<(), DbError> {
        self.conn.execute(
            "UPDATE meetings SET audio_system_path = ?2, audio_mic_path = ?3, ended_at = ?4 \
             WHERE id = ?1",
            params![id, system_path, mic_path, ended_at_epoch_s],
        )?;
        Ok(())
    }

    pub fn set_meeting_status(&mut self, id: &str, status: &str) -> Result<(), DbError> {
        self.conn
            .execute("UPDATE meetings SET status = ?2 WHERE id = ?1", params![id, status])?;
        Ok(())
    }

    /// Get-or-create a speaker row by display name. Diarization labels are
    /// per-meeting ("SPEAKER_00"), so callers namespace them before storage
    /// when cross-meeting identity is unknown.
    pub fn ensure_speaker(&mut self, display_name: &str, is_self: bool) -> Result<i64, DbError> {
        if let Some(id) = self
            .conn
            .query_row(
                "SELECT id FROM speakers WHERE display_name = ?1",
                params![display_name],
                |r| r.get::<_, i64>(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?
        {
            return Ok(id);
        }
        self.conn.execute(
            "INSERT INTO speakers (display_name, is_self) VALUES (?1, ?2)",
            params![display_name, is_self as i64],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Store a full transcript in one transaction (all-or-nothing).
    ///
    /// Speaker-identity rules (mirroring pipeline/wsw/store.py so both writers
    /// agree): "Me" (the mic track) is one global `is_self=1` row reused across
    /// meetings; anonymous `SPEAKER_*` labels get a FRESH row per meeting (so
    /// meeting A's SPEAKER_00 is never conflated with meeting B's — deduping
    /// them by name is the cross-meeting-collision bug); any other (already
    /// user-named) label dedups globally.
    pub fn insert_transcript(
        &mut self,
        meeting_id: &str,
        turns: &[(Turn, /*source:*/ &str)],
    ) -> Result<(), DbError> {
        use std::collections::HashMap;
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO segments (meeting_id, speaker_id, source, start_ms, end_ms, text) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            // Per-call cache of label -> speaker_id (keeps anonymous labels
            // consistent WITHIN this meeting without leaking across meetings).
            let mut cache: HashMap<String, i64> = HashMap::new();
            for (turn, source) in turns {
                let speaker_id = if let Some(id) = cache.get(&turn.speaker) {
                    *id
                } else {
                    let id = Self::resolve_speaker_id(&tx, &turn.speaker)?;
                    cache.insert(turn.speaker.clone(), id);
                    id
                };
                insert.execute(params![
                    meeting_id,
                    speaker_id,
                    source,
                    turn.start_ms as i64,
                    turn.end_ms as i64,
                    turn.text,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn resolve_speaker_id(conn: &rusqlite::Connection, label: &str) -> Result<i64, DbError> {
        if label == "Me" {
            if let Ok(id) = conn.query_row(
                "SELECT id FROM speakers WHERE display_name = 'Me' AND is_self = 1",
                [],
                |r| r.get::<_, i64>(0),
            ) {
                return Ok(id);
            }
            conn.execute("INSERT INTO speakers (display_name, is_self) VALUES ('Me', 1)", [])?;
            return Ok(conn.last_insert_rowid());
        }
        if label.starts_with("SPEAKER_") {
            // Fresh per-meeting row (caller caches within the meeting).
            conn.execute("INSERT INTO speakers (display_name) VALUES (?1)", params![label])?;
            return Ok(conn.last_insert_rowid());
        }
        // Named speaker: global dedup.
        conn.execute(
            "INSERT INTO speakers (display_name) \
             SELECT ?1 WHERE NOT EXISTS (SELECT 1 FROM speakers WHERE display_name = ?1)",
            params![label],
        )?;
        Ok(conn.query_row(
            "SELECT id FROM speakers WHERE display_name = ?1",
            params![label],
            |r| r.get(0),
        )?)
    }

    /// Store a summary as the next version for (meeting, kind).
    pub fn insert_summary(
        &mut self,
        meeting_id: &str,
        kind: &str,
        content: &str,
        model: &str,
        model_was_fallback: bool,
    ) -> Result<i64, DbError> {
        let next_version: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM summaries \
             WHERE meeting_id = ?1 AND kind = ?2",
            params![meeting_id, kind],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO summaries (meeting_id, version, kind, content, model, model_was_fallback) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![meeting_id, next_version, kind, content, model, model_was_fallback as i64],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// FTS5 transcript search across all meetings, ranked best-first.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>, DbError> {
        let sanitized = fts5_sanitize(query);
        if sanitized.is_empty() {
            return Ok(Vec::new());
        }
        let query = &sanitized;
        let mut stmt = self.conn.prepare(
            "SELECT s.meeting_id, sp.display_name, s.start_ms, \
                    snippet(segments_fts, 0, '[', ']', '…', 12) \
             FROM segments_fts \
             JOIN segments s ON s.id = segments_fts.rowid \
             LEFT JOIN speakers sp ON sp.id = s.speaker_id \
             WHERE segments_fts MATCH ?1 \
             ORDER BY rank LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query, limit], |r| {
            Ok(SearchHit {
                meeting_id: r.get(0)?,
                speaker: r.get(1)?,
                start_ms: r.get::<_, i64>(2)? as u64,
                snippet: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Transcript rows with their DB identities (for citation resolution and
    /// the UI's transcript pane; `transcript()` below stays the LLM-facing
    /// view that needs no ids).
    pub fn segments_for_meeting(&self, meeting_id: &str) -> Result<Vec<SegmentRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.speaker_id, COALESCE(sp.display_name, 'Unknown'), s.source, \
                    s.start_ms, s.end_ms, s.text \
             FROM segments s LEFT JOIN speakers sp ON sp.id = s.speaker_id \
             WHERE s.meeting_id = ?1 ORDER BY s.start_ms ASC",
        )?;
        let rows = stmt.query_map(params![meeting_id], |r| {
            Ok(SegmentRow {
                id: r.get(0)?,
                speaker_id: r.get(1)?,
                speaker: r.get(2)?,
                source: r.get(3)?,
                start_ms: r.get::<_, i64>(4)? as u64,
                end_ms: r.get::<_, i64>(5)? as u64,
                text: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Store resolved citation links for a summary (idempotent per call site:
    /// the pipeline writes them once, right after inserting the summary).
    pub fn insert_citations(
        &mut self,
        summary_id: i64,
        citations: &[(i64, Option<String>)],
    ) -> Result<usize, DbError> {
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO summary_citations (summary_id, segment_id, quote) \
                 VALUES (?1, ?2, ?3)",
            )?;
            for (segment_id, quote) in citations {
                insert.execute(params![summary_id, segment_id, quote])?;
            }
        }
        tx.commit()?;
        Ok(citations.len())
    }

    /// Store extracted action items. `items` pairs an optional speaker row id
    /// (resolved by the caller against this meeting's speakers) with the text.
    pub fn insert_action_items(
        &mut self,
        meeting_id: &str,
        summary_id: Option<i64>,
        items: &[(Option<i64>, String)],
    ) -> Result<usize, DbError> {
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO action_items (meeting_id, summary_id, speaker_id, text) \
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (speaker_id, text) in items {
                insert.execute(params![meeting_id, summary_id, speaker_id, text])?;
            }
        }
        tx.commit()?;
        Ok(items.len())
    }

    /// Find a speaker id by display name among the speakers who actually have
    /// segments in this meeting (never a global name lookup: meeting A's
    /// "SPEAKER_00" must not resolve against meeting B's row — the same
    /// namespacing rule as `resolve_speaker_id`).
    pub fn speaker_in_meeting(
        &self,
        meeting_id: &str,
        display_name: &str,
    ) -> Result<Option<i64>, DbError> {
        let result = self.conn.query_row(
            "SELECT sp.id FROM speakers sp \
             WHERE sp.display_name = ?2 AND EXISTS \
                (SELECT 1 FROM segments s \
                 WHERE s.meeting_id = ?1 AND s.speaker_id = sp.id) \
             LIMIT 1",
            params![meeting_id, display_name],
            |r| r.get::<_, i64>(0),
        );
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // ---- UI-facing reads (Tauri shell commands; serde-serializable rows) ----

    /// Meetings, newest first.
    pub fn list_meetings(&self, limit: u32) -> Result<Vec<MeetingRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, app, started_at, ended_at, status FROM meetings \
             ORDER BY started_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(MeetingRow {
                id: r.get(0)?,
                title: r.get(1)?,
                app: r.get(2)?,
                started_at: r.get(3)?,
                ended_at: r.get(4)?,
                status: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Latest version of a summary kind for a meeting, with its citations'
    /// audio offsets joined in (the UI renders chips without a second query).
    pub fn latest_summary(
        &self,
        meeting_id: &str,
        kind: &str,
    ) -> Result<Option<SummaryRow>, DbError> {
        let result = self.conn.query_row(
            "SELECT id, version, content, model, model_was_fallback FROM summaries \
             WHERE meeting_id = ?1 AND kind = ?2 ORDER BY version DESC LIMIT 1",
            params![meeting_id, kind],
            |r| {
                Ok(SummaryRow {
                    id: r.get(0)?,
                    version: r.get(1)?,
                    kind: kind.to_string(),
                    content: r.get(2)?,
                    model: r.get(3)?,
                    model_was_fallback: r.get::<_, i64>(4)? != 0,
                    citations: Vec::new(),
                })
            },
        );
        let mut summary = match result {
            Ok(s) => s,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut stmt = self.conn.prepare(
            "SELECT c.segment_id, c.quote, s.start_ms \
             FROM summary_citations c JOIN segments s ON s.id = c.segment_id \
             WHERE c.summary_id = ?1 ORDER BY s.start_ms ASC",
        )?;
        let rows = stmt.query_map(params![summary.id], |r| {
            Ok(CitationRow {
                segment_id: r.get(0)?,
                quote: r.get(1)?,
                start_ms: r.get::<_, i64>(2)? as u64,
            })
        })?;
        summary.citations = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(Some(summary))
    }

    pub fn action_items_for_meeting(
        &self,
        meeting_id: &str,
    ) -> Result<Vec<ActionItemRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.text, a.done, sp.display_name FROM action_items a \
             LEFT JOIN speakers sp ON sp.id = a.speaker_id \
             WHERE a.meeting_id = ?1 ORDER BY a.id ASC",
        )?;
        let rows = stmt.query_map(params![meeting_id], |r| {
            Ok(ActionItemRow {
                id: r.get(0)?,
                text: r.get(1)?,
                done: r.get::<_, i64>(2)? != 0,
                owner: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn set_action_item_done(&mut self, id: i64, done: bool) -> Result<(), DbError> {
        self.conn.execute(
            "UPDATE action_items SET done = ?2 WHERE id = ?1",
            params![id, done as i64],
        )?;
        Ok(())
    }

    /// Rename a speaker row (the who-said-what rail's click-to-rename; writes
    /// speakers.display_name exactly as docs/01 §2.1 specifies).
    pub fn rename_speaker(&mut self, speaker_id: i64, new_name: &str) -> Result<(), DbError> {
        self.conn.execute(
            "UPDATE speakers SET display_name = ?2 WHERE id = ?1",
            params![speaker_id, new_name],
        )?;
        Ok(())
    }

    /// Per-speaker talk time for one meeting (the rail's talk-time bars).
    pub fn speaker_stats(&self, meeting_id: &str) -> Result<Vec<SpeakerStat>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT sp.id, sp.display_name, sp.is_self, \
                    SUM(s.end_ms - s.start_ms) AS talk_ms \
             FROM segments s JOIN speakers sp ON sp.id = s.speaker_id \
             WHERE s.meeting_id = ?1 \
             GROUP BY sp.id ORDER BY talk_ms DESC",
        )?;
        let rows = stmt.query_map(params![meeting_id], |r| {
            Ok(SpeakerStat {
                speaker_id: r.get(0)?,
                display_name: r.get(1)?,
                is_self: r.get::<_, i64>(2)? != 0,
                talk_ms: r.get::<_, i64>(3)? as u64,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn transcript(&self, meeting_id: &str) -> Result<Vec<Turn>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(sp.display_name, 'Unknown'), s.text, s.start_ms, s.end_ms \
             FROM segments s LEFT JOIN speakers sp ON sp.id = s.speaker_id \
             WHERE s.meeting_id = ?1 ORDER BY s.start_ms ASC",
        )?;
        let rows = stmt.query_map(params![meeting_id], |r| {
            Ok(Turn {
                speaker: r.get(0)?,
                text: r.get(1)?,
                start_ms: r.get::<_, i64>(2)? as u64,
                end_ms: r.get::<_, i64>(3)? as u64,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(speaker: &str, text: &str, start: u64) -> Turn {
        Turn { speaker: speaker.into(), text: text.into(), start_ms: start, end_ms: start + 1000 }
    }

    #[test]
    fn end_to_end_meeting_flow() {
        let mut store = Store::open_in_memory().unwrap();
        store.create_meeting("m1", "Standup", Some("zoom"), 1_752_451_200).unwrap();
        store
            .insert_transcript(
                "m1",
                &[
                    (turn("Me", "we approved the budget", 0), "mic"),
                    (turn("SPEAKER_00", "shipping moves to friday", 2000), "system"),
                ],
            )
            .unwrap();

        let hits = store.search("budget", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].speaker.as_deref(), Some("Me"));
        assert_eq!(hits[0].start_ms, 0);

        let v1 = store.insert_summary("m1", "notes", "# Notes v1", "qwen", false).unwrap();
        let _v2 = store.insert_summary("m1", "notes", "# Notes v2", "qwen", true).unwrap();
        assert!(v1 > 0);
        let versions: i64 = store
            .conn
            .query_row("SELECT MAX(version) FROM summaries WHERE meeting_id='m1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(versions, 2);

        let transcript = store.transcript("m1").unwrap();
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].speaker, "Me");
    }

    #[test]
    fn speakers_are_deduplicated() {
        let mut store = Store::open_in_memory().unwrap();
        let a = store.ensure_speaker("Alice", false).unwrap();
        let b = store.ensure_speaker("Alice", false).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn anonymous_speakers_do_not_collide_across_meetings() {
        let mut store = Store::open_in_memory().unwrap();
        store.create_meeting("m1", "A", None, 1).unwrap();
        store.create_meeting("m2", "B", None, 2).unwrap();
        store.insert_transcript("m1", &[(turn("SPEAKER_00", "hi from m1", 0), "system")]).unwrap();
        store.insert_transcript("m2", &[(turn("SPEAKER_00", "hi from m2", 0), "system")]).unwrap();
        // Two distinct speaker rows despite the same display_name.
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM speakers WHERE display_name='SPEAKER_00'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 2, "each meeting's SPEAKER_00 must be its own row");
    }

    #[test]
    fn self_speaker_is_single_and_marked() {
        let mut store = Store::open_in_memory().unwrap();
        store.create_meeting("m1", "A", None, 1).unwrap();
        store.create_meeting("m2", "B", None, 2).unwrap();
        store.insert_transcript("m1", &[(turn("Me", "hi", 0), "mic")]).unwrap();
        store.insert_transcript("m2", &[(turn("Me", "again", 0), "mic")]).unwrap();
        let (count, is_self): (i64, i64) = store
            .conn
            .query_row(
                "SELECT COUNT(*), MAX(is_self) FROM speakers WHERE display_name='Me'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "one global self speaker across meetings");
        assert_eq!(is_self, 1);
    }

    #[test]
    fn citations_and_action_items_round_trip() {
        let mut store = Store::open_in_memory().unwrap();
        store.create_meeting("m1", "Standup", Some("zoom"), 1).unwrap();
        store
            .insert_transcript(
                "m1",
                &[
                    (turn("Me", "I will send the budget", 0), "mic"),
                    (turn("SPEAKER_00", "shipping moves to friday", 65_000), "system"),
                ],
            )
            .unwrap();
        let segments = store.segments_for_meeting("m1").unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1].start_ms, 65_000);

        let summary_id = store.insert_summary("m1", "notes", "notes [01:05]", "qwen", false).unwrap();
        store
            .insert_citations(summary_id, &[(segments[1].id, Some("shipping moves".into()))])
            .unwrap();
        let summary = store.latest_summary("m1", "notes").unwrap().unwrap();
        assert_eq!(summary.id, summary_id);
        assert_eq!(summary.citations.len(), 1);
        assert_eq!(summary.citations[0].start_ms, 65_000);
        assert!(store.latest_summary("m1", "outline").unwrap().is_none());

        // Speaker resolution is meeting-scoped.
        let me = store.speaker_in_meeting("m1", "Me").unwrap();
        assert!(me.is_some());
        assert!(store.speaker_in_meeting("m2", "Me").unwrap().is_none());

        store
            .insert_action_items("m1", Some(summary_id), &[(me, "send the budget".into())])
            .unwrap();
        let items = store.action_items_for_meeting("m1").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].owner.as_deref(), Some("Me"));
        assert!(!items[0].done);
        store.set_action_item_done(items[0].id, true).unwrap();
        assert!(store.action_items_for_meeting("m1").unwrap()[0].done);
    }

    #[test]
    fn ui_reads_meetings_stats_and_rename() {
        let mut store = Store::open_in_memory().unwrap();
        store.create_meeting("m1", "A", Some("zoom"), 100).unwrap();
        store.create_meeting("m2", "B", None, 200).unwrap();
        let meetings = store.list_meetings(10).unwrap();
        assert_eq!(meetings.len(), 2);
        assert_eq!(meetings[0].id, "m2", "newest first");

        store
            .insert_transcript(
                "m1",
                &[
                    (turn("Me", "short", 0), "mic"),
                    (turn("SPEAKER_00", "a much longer remark", 2_000), "system"),
                    (turn("SPEAKER_00", "and another one", 4_000), "system"),
                ],
            )
            .unwrap();
        let stats = store.speaker_stats("m1").unwrap();
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].display_name, "SPEAKER_00", "most talk time first");
        assert_eq!(stats[0].talk_ms, 2_000);
        assert!(stats.iter().any(|s| s.is_self));

        let anon = stats[0].speaker_id;
        store.rename_speaker(anon, "Sarah").unwrap();
        let segs = store.segments_for_meeting("m1").unwrap();
        assert!(segs.iter().any(|s| s.speaker == "Sarah"));
    }

    #[test]
    fn search_tolerates_punctuation() {
        let mut store = Store::open_in_memory().unwrap();
        store.create_meeting("m1", "A", None, 1).unwrap();
        store
            .insert_transcript("m1", &[(turn("Me", "we shipped covid-19 dashboards", 0), "mic")])
            .unwrap();
        // These would be FTS5 syntax errors unsanitized.
        assert_eq!(store.search("covid-19", 10).unwrap().len(), 1);
        assert!(store.search("don't", 10).unwrap().is_empty());
        assert!(store.search("", 10).unwrap().is_empty());
        assert!(store.search("   ", 10).unwrap().is_empty());
    }
}
