-- whosaidwhat SQLite schema v1
--
-- Informed by (see docs/03-database-schema.md for the full research):
--   * Meetily CE:  meetings + transcripts tables, LIKE-based search, summaries
--                  overwritten in place  -> we keep the meeting/turn split but fix
--                  search (FTS5) and provenance (versioned summaries).
--   * Screenpipe:  media files OUT of the DB (paths only), FTS5 virtual tables,
--                  dedicated speakers table, sqlx-style migrations table.
--   * Minutes:     SQLite as a rebuildable index over canonical exports; we adopt
--                  the exportable-markdown idea (summaries.content is markdown).
--   * Hyprnote:    numbered SQL migrations applied at startup by a Rust crate.
--
-- Conventions: ms-precision INTEGER timestamps relative to recording start for
-- audio positions; unix epoch seconds for wall-clock times; TEXT UUIDs for ids
-- exposed to the UI; WAL mode set by the opening connection, not in-schema.

BEGIN;

CREATE TABLE IF NOT EXISTS schema_migrations (
    version     INTEGER PRIMARY KEY,
    applied_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

-- One row per meeting/recording session.
CREATE TABLE IF NOT EXISTS meetings (
    id            TEXT PRIMARY KEY,              -- UUID
    title         TEXT NOT NULL DEFAULT 'Untitled meeting',
    app           TEXT,                          -- 'zoom' | 'teams' | 'meet' | 'manual' | NULL
    started_at    INTEGER NOT NULL,              -- unix epoch seconds
    ended_at      INTEGER,
    -- Audio stays on disk (screenpipe pattern): two tracks per recording.
    audio_system_path  TEXT,
    audio_mic_path     TEXT,
    -- Pipeline state machine, one honest column:
    -- 'recorded' -> 'transcribed' -> 'summarized' (or 'failed:<stage>')
    status        TEXT NOT NULL DEFAULT 'recorded',
    created_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_meetings_started_at ON meetings(started_at DESC);

-- Speakers are first-class and global: the same human across many meetings.
-- Diarization emits anonymous per-meeting labels; rows here are created lazily
-- and merged when the user names them (Minutes' alias-merging pattern).
-- NOTE: display_name is deliberately NOT UNIQUE. Anonymous labels (SPEAKER_00)
-- recur across meetings as SEPARATE humans, so each meeting gets its own row
-- (see db.rs resolve_speaker_id / store.py _speaker_id_for). Named speakers and
-- the single is_self='Me' row are deduplicated in code, not by constraint.
CREATE TABLE IF NOT EXISTS speakers (
    id               INTEGER PRIMARY KEY,
    display_name     TEXT NOT NULL,              -- 'SPEAKER_00' until user renames
    is_self          INTEGER NOT NULL DEFAULT 0, -- the mic-track owner
    -- Mean voice embedding for cross-meeting re-identification (opt-in),
    -- stored as raw f32 little-endian bytes.
    embedding        BLOB,
    embedding_model  TEXT,                       -- provenance for the embedding
    created_at       INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Speaker-turn granularity transcript segments. This is THE transcript table;
-- there is no separate blob of full text (renderers join segments in order).
CREATE TABLE IF NOT EXISTS segments (
    id          INTEGER PRIMARY KEY,             -- rowid, used by FTS
    meeting_id  TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    speaker_id  INTEGER REFERENCES speakers(id) ON DELETE SET NULL,
    -- Which capture track the words came from ('mic' | 'system'):
    -- mic-track segments are the local user by construction.
    source      TEXT NOT NULL DEFAULT 'system',
    start_ms    INTEGER NOT NULL,
    end_ms      INTEGER NOT NULL,
    text        TEXT NOT NULL,
    CHECK (end_ms >= start_ms)
);

CREATE INDEX IF NOT EXISTS idx_segments_meeting ON segments(meeting_id, start_ms);
CREATE INDEX IF NOT EXISTS idx_segments_speaker ON segments(speaker_id);

-- Full-text search over transcript text: FTS5 external-content table so the
-- text is stored once (in segments) and indexed, not duplicated.
-- The trigger discipline below is load-bearing: the 'delete' command MUST be
-- fed the OLD row values or FTS5 silently corrupts the index
-- (https://sqlite.org/fts5.html §external content tables).
CREATE VIRTUAL TABLE IF NOT EXISTS segments_fts USING fts5(
    text,
    content='segments',
    content_rowid='id',
    tokenize='porter unicode61'
);

CREATE TRIGGER IF NOT EXISTS segments_ai AFTER INSERT ON segments BEGIN
    INSERT INTO segments_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TRIGGER IF NOT EXISTS segments_ad AFTER DELETE ON segments BEGIN
    INSERT INTO segments_fts(segments_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;

CREATE TRIGGER IF NOT EXISTS segments_au AFTER UPDATE ON segments BEGIN
    INSERT INTO segments_fts(segments_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO segments_fts(rowid, text) VALUES (new.id, new.text);
END;

-- Summaries are versioned rows, never overwrites, with full model provenance
-- (the improvement over Meetily, which keeps model config in a settings table
-- and cannot answer "which model wrote this summary?").
CREATE TABLE IF NOT EXISTS summaries (
    id           INTEGER PRIMARY KEY,
    meeting_id   TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    version      INTEGER NOT NULL,
    kind         TEXT NOT NULL DEFAULT 'notes',  -- 'notes' | 'outline' | 'blog'
    content      TEXT NOT NULL,                  -- markdown (exportable as-is)
    model        TEXT NOT NULL,                  -- e.g. 'Qwen3.6-35B-A3B-oQ4e-mtp'
    engine       TEXT NOT NULL DEFAULT 'omlx',   -- serving engine
    model_was_fallback INTEGER NOT NULL DEFAULT 0,
    prompt_hash  TEXT,                           -- sha256 of the prompt set used
    created_at   INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (meeting_id, kind, version)
);

-- Per-takeaway citations: deep links from summary bullets back into audio
-- (the Notion Nov-2025 pattern; cheap here because segments carry ms offsets).
CREATE TABLE IF NOT EXISTS summary_citations (
    id          INTEGER PRIMARY KEY,
    summary_id  INTEGER NOT NULL REFERENCES summaries(id) ON DELETE CASCADE,
    segment_id  INTEGER NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
    quote       TEXT
);

CREATE INDEX IF NOT EXISTS idx_citations_summary ON summary_citations(summary_id);
-- Cover the child-side FK so cascading a meeting/segment delete does not scan
-- the whole citations table per deleted segment (SQLite FK docs recommendation).
CREATE INDEX IF NOT EXISTS idx_citations_segment ON summary_citations(segment_id);

-- Action items extracted by stage 1/2, queryable across meetings.
CREATE TABLE IF NOT EXISTS action_items (
    id          INTEGER PRIMARY KEY,
    meeting_id  TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    summary_id  INTEGER REFERENCES summaries(id) ON DELETE SET NULL,
    speaker_id  INTEGER REFERENCES speakers(id) ON DELETE SET NULL,
    text        TEXT NOT NULL,
    done        INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_action_items_open ON action_items(done, meeting_id);
-- Cover the FK columns for cascade/SET NULL enforcement (the composite index
-- above cannot serve a summary_id-only lookup, and meeting_id benefits from a
-- dedicated index on delete).
CREATE INDEX IF NOT EXISTS idx_action_items_meeting ON action_items(meeting_id);
CREATE INDEX IF NOT EXISTS idx_action_items_summary ON action_items(summary_id);

INSERT OR IGNORE INTO schema_migrations(version) VALUES (1);

COMMIT;
