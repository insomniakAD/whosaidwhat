# Task 3 — Database Schema Research & the whosaidwhat Schema

Deliverable: [`/schema.sql`](../schema.sql) — executed and verified in this sandbox
(FTS5 trigger sync, snippet/rank search, summary versioning, cascade deletes,
idempotent re-run; SQLite 3.45.1). Used by both the Rust app (`src-tauri/src/db.rs`,
`include_str!`) and the Python pipeline (`pipeline/wsw/store.py`) — one source of truth.

Evidence tiers: **[fetched]** / **[search-verified]** / **[verified-by-execution]**
(pattern proven by running it locally) / **[inference]**.

## 1. How the reference projects structure their databases

### Meetily (Zackriya-Solutions/meetily — the pipeline's current upstream)
Single SQLite file, ~6 tables (`meetings`, `transcripts`, `transcript_chunks`, AI
model config + API keys, transcription provider config, settings) managed by a
`DatabaseManager` over aiosqlite; search is case-insensitive **LIKE**, not FTS.
**[search-verified]** https://deepwiki.com/Zackriya-Solutions/meeting-minutes
The user's own extractor confirms the working column set: `meetings.id/title/
created_at`, `transcripts.meeting_id/speaker/transcript/timestamp` (verified by
reading `reference/meetily_pipeline/meetily_db_extractor.py`, which ran successfully
against the live DB per its tracking-file logic). Upstream `db.py` was not fetchable
from this sandbox — flagged, not assumed. Note: Meetily markets diarization as a
**Pro** feature, so CE's `transcripts.speaker` may be sparse — directly motivating
Task 4. **[search-verified]** https://github.com/Zackriya-Solutions/meetily

Weaknesses to fix (inference): LIKE search doesn't scale and can't rank; summaries
lack per-row model provenance (model config lives in a separate settings table);
transcript text at arbitrary-chunk granularity rather than speaker turns.

### OpenWhispr (OpenWhispr/openwhispr)
Electron (not Tauri), better-sqlite3 for transcription history, SQLite + **FTS5**
for notes. Concrete DDL unreachable this session. **[search-verified]**
https://github.com/OpenWhispr/openwhispr
Takeaway adopted: FTS5 for text search is the category standard even in small apps.

### Minutes (silverstein/minutes — the meeting-notes "Minutes")
Tauri menu-bar app; pipeline Transcribe → Diarize → Summarize → structured
**Markdown as the canonical store**, with SQLite as a derived index rebuilt from
markdown in <50 ms; tracks people/commitments/topics with alias merging.
**[search-verified]** https://github.com/silverstein/minutes + https://useminutes.app
Takeaways adopted: summaries stored as export-ready markdown; global `speakers`
table with rename/merge semantics (their alias pattern). The full
"DB-is-disposable" inversion was **not** adopted (D-011 below).

### Screenpipe (screenpipe/screenpipe)
`~/.screenpipe/db.sqlite`: metadata, OCR text, `audio_transcriptions`, `speakers`,
tags; **media files on disk, not in the DB**; FTS5 virtual tables incl. `audio_fts`;
sqlx migrations table. **[search-verified]** https://docs.screenpi.pe/architecture
Takeaways adopted: audio paths not blobs; dedicated speakers table; migrations table.

### Hyprnote (fastrepl/hyprnote)
`db.sqlite` managed by a dedicated Rust crate with numbered SQL migration scripts
applied at startup. **[search-verified]** https://hyprnote.com/docs/developers/storage
Takeaway adopted: schema_migrations + idempotent DDL applied at connection open.

## 2. Patterns verified by execution (not just reading)

- **FTS5 external-content table** over `segments.text` with AFTER INSERT/DELETE/
  UPDATE triggers; the delete half **must** use `old.*` values or the index silently
  corrupts (SQLite forum, corroborated). Ran the full DDL + triggers + `snippet()`/
  `rank` queries + `integrity-check` locally: update flips match results correctly.
  **[verified-by-execution]**; refs https://sqlite.org/fts5.html,
  https://sqlite.org/forum/forumpost/acdc2aa30a
- **WAL mode**: readers don't block the writer; per-connection `PRAGMA
  journal_mode=WAL` returns `wal`. Caveat carried into docs: same-host only, no
  network volumes. **[verified-by-execution]**; ref https://sqlite.org/wal.html
- **Embeddings as BLOBs** next to segment rows (f32 LE bytes) insert/read fine —
  kept on the `speakers` table (mean voiceprint) rather than per-segment to avoid
  8 KB × thousands of rows for a feature (cross-meeting re-identification) that
  reads rarely. **[verified-by-execution]** + inference.

## 3. The schema (what and why)

See `/schema.sql` for full DDL with inline rationale. Shape:

```
meetings 1─* segments *─1 speakers          segments 1─1 segments_fts (external content)
meetings 1─* summaries 1─* summary_citations *─1 segments
meetings 1─* action_items
schema_migrations
```

Key decisions:

| Decision | Over | Why |
|---|---|---|
| Speaker-turn granularity `segments` (`start_ms`/`end_ms` INTEGER) | Meetily's chunk rows / one big text blob | diarization is turn-native; ms offsets make audio deep-links and citation chips free (Notion pattern, docs/01) |
| Global `speakers` + lazy anonymous rows | per-meeting labels only | rename-once semantics; Minutes' alias merging; is_self marks the mic-track owner |
| `source` column ('mic'/'system') on segments | inferring later | the two-track capture design is the diarization shortcut (docs/00 §2); provenance must survive storage |
| Versioned `summaries` with `model`, `engine`, `model_was_fallback`, `prompt_hash` | Meetily's overwrite + settings-table config | answers "which model wrote this?" forever; regenerating with a better model never destroys history |
| FTS5 external-content + triggers | Meetily's LIKE | ranked, snippeted, scales; text stored once |
| Audio as paths | blobs in DB | screenpipe pattern; keeps DB small and Time-Machine-friendly |
| `action_items` table | parsing markdown at query time | cross-meeting "what do I owe people" queries |

**Implementation status (honest):** `meetings`, `speakers`, `segments`,
`segments_fts`, and `summaries` are written by both the Rust (`db.rs`) and Python
(`store.py`) paths today. `summary_citations` and `action_items` are schema-defined
and indexed but **not yet populated** — the summarizer currently preserves `[mm:ss]`
markers inline in the notes rather than extracting structured citation/action rows.
Populating them is the next pipeline step, not a schema change.

**D-011 (canonical store)** — Markdown-canonical (Minutes) was considered and
rejected for v1: whosaidwhat's segments carry ms timestamps + speaker FKs + FTS,
which lossy markdown round-trips poorly. Instead: SQLite is canonical, `summaries.content`
is export-ready markdown, and a one-file-per-meeting exporter stays trivial. Logged
as the reversible call.

## 4. Migration posture

`schema_migrations(version)` + fully idempotent DDL (`IF NOT EXISTS` everywhere) =
v1 bootstraps and re-runs safely (verified by double-execution). v2+ follows the
Hyprnote/screenpipe numbered-script pattern; rusqlite applies scripts inside one
transaction at open.
