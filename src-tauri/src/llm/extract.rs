//! Structured extraction: `[mm:ss]` citation markers and action items.
//!
//! The summarizer's stage-2/3 prompts preserve `[mm:ss]` timestamp markers
//! inline (see `summarize.rs`). This module turns those markers, plus a
//! dedicated stage-4 extraction call, into rows for the schema's
//! `summary_citations` and `action_items` tables (schema.sql; the Notion-style
//! per-takeaway deep-link pattern from docs/01 §2.1).
//!
//! Design constraints:
//! - **No JSON mode.** Per D-013 we only rely on the plain OpenAI-compatible
//!   chat surface, so the action-item stage uses the same strict line format
//!   as stage 1 and is parsed with a hand-rolled scanner, never `serde_json`.
//! - **Dependency-free and pure** so every function here runs under the
//!   bare-rustc harness (BUILD_LOG D-006). The LLM call and the DB writes
//!   live in `pipeline::process_recording` / `db.rs`, not here.
//! - **Honest resolution.** A marker that cannot be matched to a real segment
//!   (model hallucinated or rounded beyond tolerance) is dropped, not
//!   force-linked to the nearest row.

/// Stage-4 prompt: structured action items from the stage-2 outline (the
/// outline always exists — short meetings skip stage 1, never stage 2 — and
/// it is the surface the stage-2 prompt keeps timestamps in).
pub const ACTION_ITEMS_PROMPT: &str = r#"You are a strict data extraction engine. From the meeting outline below, extract every action item: a concrete task somebody committed to do.
Output one line per action item, exactly in this format, with no other text:
* Owner: task description [mm:ss]

Rules:
- Owner is the speaker's name exactly as it appears in the outline. If no owner is stated, write Unassigned.
- Keep the [mm:ss] timestamp of the moment the task was discussed if the outline shows one; omit it if none is shown.
- Do not invent tasks, owners, or timestamps not present in the outline.
- If there are no action items, output exactly: None"#;

/// A `[mm:ss]` / `[h:mm:ss]` marker found in summary text, as milliseconds
/// from recording start.
///
/// The formatter (`llm::chunk::format_chunk`) emits `[{:02}:{:02}]` where the
/// minute field is unbounded (a 90-minute meeting cites `[92:11]`), so the
/// parser accepts 1–4 minute digits; models sometimes normalize long offsets
/// to `[h:mm:ss]`, so that form is accepted too.
pub fn parse_timestamps(text: &str) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some((ms, consumed)) = parse_marker(&bytes[i..]) {
                if !out.contains(&ms) {
                    out.push(ms);
                }
                i += consumed;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Parse one marker at the start of `bytes` (which begins with `[`).
/// Returns (milliseconds, bytes consumed including both brackets).
fn parse_marker(bytes: &[u8]) -> Option<(u64, usize)> {
    debug_assert_eq!(bytes.first(), Some(&b'['));
    let inner_start = 1;
    let close = bytes.iter().position(|&b| b == b']')?;
    let inner = std::str::from_utf8(&bytes[inner_start..close]).ok()?;
    // 2 or 3 colon-separated all-digit fields: mm:ss or h:mm:ss.
    let parts: Vec<&str> = inner.split(':').collect();
    let all_digits =
        |s: &str| !s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit());
    let secs = match parts.as_slice() {
        [m, s] if all_digits(m) && all_digits(s) && s.len() == 2 => {
            let (m, s): (u64, u64) = (m.parse().ok()?, s.parse().ok()?);
            if s >= 60 {
                return None;
            }
            m * 60 + s
        }
        [h, m, s]
            if all_digits(h) && all_digits(m) && all_digits(s) && m.len() == 2 && s.len() == 2 =>
        {
            let (h, m, s): (u64, u64, u64) = (h.parse().ok()?, m.parse().ok()?, s.parse().ok()?);
            if m >= 60 || s >= 60 {
                return None;
            }
            (h * 60 + m) * 60 + s
        }
        _ => return None,
    };
    Some((secs * 1000, close + 1))
}

/// Minimal segment view for marker resolution (mirror of db::SegmentRow's
/// timing fields; kept separate so this module stays dependency-free).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentSpan {
    /// segments.id
    pub id: i64,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// How far (ms) a marker may fall outside every segment and still snap to the
/// nearest one. Markers are formatter-truncated to whole seconds and models
/// occasionally round, so a small tolerance is honest; beyond it the marker is
/// treated as hallucinated and dropped.
pub const RESOLVE_TOLERANCE_MS: u64 = 10_000;

/// Resolve a marker to the segment it points into: the segment containing the
/// timestamp, else the nearest segment within [`RESOLVE_TOLERANCE_MS`], else
/// `None`. `segments` must be sorted by `start_ms` (the DB query's order).
pub fn resolve_segment(ms: u64, segments: &[SegmentSpan]) -> Option<i64> {
    // Containment first: truncation means a marker for a segment starting at
    // 65_400 ms reads [01:05] = 65_000 ms, which its own span may not cover —
    // so containment alone is not enough, but when it hits it is exact.
    if let Some(seg) = segments.iter().find(|s| s.start_ms <= ms && ms <= s.end_ms) {
        return Some(seg.id);
    }
    segments
        .iter()
        .map(|s| {
            let d = if s.start_ms > ms { s.start_ms - ms } else { ms - s.start_ms };
            (d, s.id)
        })
        .min()
        .filter(|(d, _)| *d <= RESOLVE_TOLERANCE_MS)
        .map(|(_, id)| id)
}

/// Citation quote: the cited segment's words, bounded so a monologue segment
/// doesn't bloat the citations table (char-boundary-safe truncation; mirrored
/// by wsw.extract.quote_snippet).
pub fn quote_snippet(text: &str) -> String {
    const MAX_CHARS: usize = 240;
    if text.chars().count() <= MAX_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(MAX_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// One parsed action item from the stage-4 response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionItem {
    /// Owner name as written; `None` when the model wrote "Unassigned".
    pub owner: Option<String>,
    pub text: String,
    /// Timestamp of the moment, if the model kept one.
    pub ts_ms: Option<u64>,
}

/// Parse the strict stage-4 line format. Tolerant of the bullet variants
/// models actually produce (`*`, `-`, `•`), of "None" responses, and of stray
/// prose lines (skipped: no `Owner:` prefix). Never errors — worst case is an
/// empty list, which the caller records as "no action items".
pub fn parse_action_items(response: &str) -> Vec<ActionItem> {
    let mut items = Vec::new();
    for raw in response.lines() {
        let line = raw.trim();
        let Some(rest) = line
            .strip_prefix("* ")
            .or_else(|| line.strip_prefix("- "))
            .or_else(|| line.strip_prefix("• "))
        else {
            continue;
        };
        // Owner is everything before the first ": " — names with colons don't
        // survive the transcript formatter, so the first colon is the split.
        let Some((owner_part, task_part)) = rest.split_once(':') else {
            continue;
        };
        let owner_raw = owner_part.trim().trim_matches(|c| c == '[' || c == ']').trim();
        if owner_raw.is_empty() {
            continue;
        }
        let mut text = task_part.trim().to_string();
        if text.is_empty() {
            continue;
        }
        // Trailing timestamp: strip it from the display text, keep it as data.
        let ts_list = parse_timestamps(&text);
        let ts_ms = ts_list.last().copied();
        // Strip ONLY a marker that ends at the end of the (trimmed) text —
        // never a mid-text marker that merely happens to sit before a stray
        // `]`. Mirrors wsw.extract's `trailing.end() == len(text)` check so
        // both twins store identical text for identical input.
        let trimmed_len = text.trim_end().len();
        if let Some(open) = text[..trimmed_len].rfind('[') {
            if let Some((_, consumed)) = parse_marker(text[open..].as_bytes()) {
                if open + consumed == trimmed_len {
                    text.truncate(open);
                    text.truncate(text.trim_end().len());
                }
            }
        }
        if text.is_empty() {
            continue;
        }
        let owner = if owner_raw.eq_ignore_ascii_case("unassigned") {
            None
        } else {
            Some(owner_raw.to_string())
        };
        items.push(ActionItem { owner, text, ts_ms });
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mm_ss_and_h_mm_ss_markers() {
        let text = "Decision at [01:05], revisited [92:11] and again at [1:32:07].";
        assert_eq!(
            parse_timestamps(text),
            vec![65_000, (92 * 60 + 11) * 1000, ((1 * 60 + 32) * 60 + 7) * 1000]
        );
    }

    #[test]
    fn ignores_non_marker_brackets_and_dedups() {
        let text = "[TODO] fix [01:05] later; see [01:05] and [notes] and [12:99].";
        assert_eq!(parse_timestamps(text), vec![65_000]);
    }

    #[test]
    fn marker_at_boundaries_and_empty() {
        assert_eq!(parse_timestamps("[00:00]"), vec![0]);
        assert!(parse_timestamps("").is_empty());
        assert!(parse_timestamps("no markers here").is_empty());
        // Unclosed bracket must not panic or loop.
        assert!(parse_timestamps("broken [12:3").is_empty());
    }

    fn seg(id: i64, start: u64, end: u64) -> SegmentSpan {
        SegmentSpan { id, start_ms: start, end_ms: end }
    }

    #[test]
    fn resolves_containment_exactly() {
        let segs = vec![seg(1, 0, 4_000), seg(2, 5_000, 9_000)];
        assert_eq!(resolve_segment(6_000, &segs), Some(2));
        assert_eq!(resolve_segment(0, &segs), Some(1));
        assert_eq!(resolve_segment(4_000, &segs), Some(1)); // inclusive end
    }

    #[test]
    fn snaps_truncated_marker_to_nearest_start_within_tolerance() {
        // Segment starts at 65_400; formatter emits [01:05] = 65_000.
        let segs = vec![seg(1, 0, 60_000), seg(2, 65_400, 80_000)];
        assert_eq!(resolve_segment(65_000, &segs), Some(2));
    }

    #[test]
    fn drops_marker_beyond_tolerance() {
        let segs = vec![seg(1, 0, 4_000)];
        // 4s past the segment end but 20s past its start: nearest-start
        // distance is what gates, so this still resolves...
        assert_eq!(resolve_segment(8_000, &segs), Some(1));
        // ...while a marker far beyond everything is dropped.
        assert_eq!(resolve_segment(120_000, &segs), None);
        assert_eq!(resolve_segment(0, &[]), None);
    }

    #[test]
    fn parses_action_items_with_owner_timestamp_variants() {
        let response = "\
* Sarah: send the revised budget to finance [12:41]
- Me: file the DER benchmark issue
• Unassigned: book a room for the offsite [01:05]
Some stray narration the model added.
* : broken line skipped
* NoTask:";
        let items = parse_action_items(response);
        assert_eq!(items.len(), 3);
        assert_eq!(
            items[0],
            ActionItem {
                owner: Some("Sarah".into()),
                text: "send the revised budget to finance".into(),
                ts_ms: Some((12 * 60 + 41) * 1000),
            }
        );
        assert_eq!(items[1].owner.as_deref(), Some("Me"));
        assert_eq!(items[1].ts_ms, None);
        assert_eq!(items[2].owner, None, "Unassigned maps to no owner");
        assert_eq!(items[2].text, "book a room for the offsite");
    }

    #[test]
    fn quote_snippet_truncates_on_char_boundaries() {
        assert_eq!(quote_snippet("short"), "short");
        let long = "é".repeat(400); // multi-byte chars: byte-index truncation would panic
        let q = quote_snippet(&long);
        assert!(q.ends_with('…'));
        assert_eq!(q.chars().count(), 241);
    }

    #[test]
    fn none_response_yields_no_items() {
        assert!(parse_action_items("None").is_empty());
        assert!(parse_action_items("").is_empty());
    }

    #[test]
    fn only_a_truly_trailing_marker_is_stripped() {
        // Mid-text marker followed by more text + a stray `]`: the marker is
        // NOT trailing, so the text is kept verbatim (twin parity with
        // wsw.extract — the earlier rfind('[')+ends_with(']') logic wrongly
        // truncated to "do the thing").
        let items = parse_action_items("* Owner: do the thing [12:41] more]");
        assert_eq!(items[0].text, "do the thing [12:41] more]");
        assert_eq!(items[0].ts_ms, Some((12 * 60 + 41) * 1000));
        // A genuinely trailing marker still strips.
        let t = parse_action_items("* Owner: ship it [02:00]");
        assert_eq!(t[0].text, "ship it");
        // Two markers, last one trailing: strip only the trailing one.
        let two = parse_action_items("* Owner: foo [01:00] bar [02:00]");
        assert_eq!(two[0].text, "foo [01:00] bar");
        assert_eq!(two[0].ts_ms, Some(120_000));
    }

    #[test]
    fn bracketed_owner_format_is_tolerated() {
        // Stage-1 style "* [Speaker]: [Detail]" leakage.
        let items = parse_action_items("* [Kim]: draft the rollout plan [03:00]");
        assert_eq!(items[0].owner.as_deref(), Some("Kim"));
        assert_eq!(items[0].text, "draft the rollout plan");
        assert_eq!(items[0].ts_ms, Some(180_000));
    }
}
