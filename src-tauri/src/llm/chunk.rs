//! Transcript chunking for the map stage of summarization.
//!
//! Port of the Python `chunk_transcript` in `pipeline/summarize.py`, with the
//! same semantics (word budget per chunk + turn-overlap carryover) so the Rust
//! and Python paths produce identical chunk boundaries for the same input.
//! Dependency-free and unit-tested on any host.

/// One diarized utterance.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub speaker: String,
    pub text: String,
    /// Milliseconds from recording start; used for citations back into audio.
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Split turns into chunks of at most `max_words`, carrying the last
/// `overlap_turns` turns into the next chunk for context continuity.
///
/// Invariants (matching the Python implementation):
/// - a single turn longer than `max_words` still becomes (part of) a chunk —
///   turns are never split;
/// - overlap turns are re-counted against the next chunk's word budget;
/// - every turn appears in at least one chunk; order is preserved.
pub fn chunk_turns(turns: &[Turn], max_words: usize, overlap_turns: usize) -> Vec<Vec<Turn>> {
    let mut chunks: Vec<Vec<Turn>> = Vec::new();
    let mut current: Vec<Turn> = Vec::new();
    let mut current_words = 0usize;

    for turn in turns {
        let turn_words = turn.text.split_whitespace().count();

        if current_words + turn_words > max_words && !current.is_empty() {
            let overlap = overlap_turns.min(current.len());
            let carry: Vec<Turn> = current[current.len() - overlap..].to_vec();
            chunks.push(std::mem::take(&mut current));
            current_words = carry.iter().map(|t| t.text.split_whitespace().count()).sum();
            current = carry;
        }

        current.push(turn.clone());
        current_words += turn_words;
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Render a chunk as the plain-text block the extraction prompt consumes:
/// `Speaker: text` paragraphs separated by blank lines, with a `[mm:ss]`
/// timestamp so the model can cite moments.
pub fn format_chunk(chunk: &[Turn]) -> String {
    let mut out = String::new();
    for turn in chunk {
        let secs = turn.start_ms / 1000;
        out.push_str(&format!(
            "[{:02}:{:02}] {}: {}\n\n",
            secs / 60,
            secs % 60,
            turn.speaker,
            turn.text
        ));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(speaker: &str, words: usize, start_ms: u64) -> Turn {
        Turn {
            speaker: speaker.to_string(),
            text: vec!["word"; words].join(" "),
            start_ms,
            end_ms: start_ms + 1000,
        }
    }

    #[test]
    fn everything_fits_in_one_chunk() {
        let turns = vec![turn("A", 100, 0), turn("B", 100, 1000)];
        let chunks = chunk_turns(&turns, 1200, 2);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 2);
    }

    #[test]
    fn splits_at_budget_with_overlap() {
        // 5 turns of 400 words, budget 1000, overlap 1.
        let turns: Vec<Turn> = (0..5).map(|i| turn(&format!("S{i}"), 400, i * 1000)).collect();
        let chunks = chunk_turns(&turns, 1000, 1);
        // chunk 1: S0,S1 (800). S2 would make 1200 → split, carry S1.
        // chunk 2: S1,S2 (800). S3 → split, carry S2. etc.
        assert!(chunks.len() >= 3);
        for w in chunks.windows(2) {
            let (prev, next) = (&w[0], &w[1]);
            assert_eq!(prev.last(), next.first(), "overlap turn carried over");
        }
        // Every original turn appears somewhere.
        for t in &turns {
            assert!(chunks.iter().any(|c| c.contains(t)));
        }
    }

    #[test]
    fn oversized_single_turn_is_not_split() {
        let turns = vec![turn("A", 50, 0), turn("B", 5000, 1000), turn("C", 50, 2000)];
        let chunks = chunk_turns(&turns, 1200, 2);
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        assert!(total >= 3);
        assert!(chunks.iter().any(|c| c.iter().any(|t| t.speaker == "B")));
    }

    #[test]
    fn empty_input_empty_output() {
        assert!(chunk_turns(&[], 1200, 2).is_empty());
    }

    #[test]
    fn overlap_zero_has_no_carryover() {
        let turns: Vec<Turn> = (0..4).map(|i| turn(&format!("S{i}"), 600, i * 1000)).collect();
        let chunks = chunk_turns(&turns, 1000, 0);
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(total, 4, "no duplicates when overlap is 0");
    }

    #[test]
    fn format_includes_timestamp_and_speaker() {
        let c = vec![Turn {
            speaker: "Ada".into(),
            text: "hello there".into(),
            start_ms: 65_000,
            end_ms: 66_000,
        }];
        assert_eq!(format_chunk(&c), "[01:05] Ada: hello there");
    }
}
