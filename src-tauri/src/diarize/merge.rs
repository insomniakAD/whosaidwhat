//! Merge ASR segments with diarization segments into speaker-attributed turns.
//!
//! Engine-agnostic and dependency-free (unit-tested on any host). The
//! two-track capture design also flows in here: mic-track ASR arrives
//! pre-attributed to the local user, system-track ASR gets speakers from
//! diarization, and `interleave` zips both into one chronological transcript.

use crate::asr::AsrSegment;
use crate::diarize::SpeakerSegment;
use crate::llm::chunk::Turn;

/// Assign each ASR segment the speaker whose diarization span overlaps it
/// most. Ties break to the earlier speaker segment; ASR spans with no overlap
/// at all get `fallback_speaker` (silence-adjacent fragments, jingles).
pub fn attribute_speakers(
    asr: &[AsrSegment],
    speakers: &[SpeakerSegment],
    fallback_speaker: &str,
) -> Vec<Turn> {
    asr.iter()
        .map(|seg| {
            let mut best: Option<(&SpeakerSegment, u64)> = None;
            for sp in speakers {
                let overlap = overlap_ms(seg.start_ms, seg.end_ms, sp.start_ms, sp.end_ms);
                if overlap > 0 {
                    let better = match best {
                        None => true,
                        Some((_, best_overlap)) => overlap > best_overlap,
                    };
                    if better {
                        best = Some((sp, overlap));
                    }
                }
            }
            Turn {
                speaker: best
                    .map(|(sp, _)| sp.speaker.clone())
                    .unwrap_or_else(|| fallback_speaker.to_string()),
                text: seg.text.clone(),
                start_ms: seg.start_ms,
                end_ms: seg.end_ms,
            }
        })
        .collect()
}

/// Merge consecutive turns from the same speaker into one turn (diarization
/// often splits one utterance across ASR segment boundaries). `max_gap_ms`
/// keeps distinct utterances apart (a speaker answering their own question
/// two minutes later is a new turn).
pub fn coalesce_turns(turns: Vec<Turn>, max_gap_ms: u64) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::with_capacity(turns.len());
    for turn in turns {
        match out.last_mut() {
            Some(prev)
                if prev.speaker == turn.speaker
                    && turn.start_ms.saturating_sub(prev.end_ms) <= max_gap_ms =>
            {
                prev.text.push(' ');
                prev.text.push_str(&turn.text);
                prev.end_ms = turn.end_ms;
            }
            _ => out.push(turn),
        }
    }
    out
}

/// Zip the local-user track (already attributed) with the remote track into
/// one chronological transcript. Stable on ties (local first, matching how
/// meeting audio interleaves in practice: you hear yourself before the reply).
pub fn interleave(local: Vec<Turn>, remote: Vec<Turn>) -> Vec<Turn> {
    let mut all = Vec::with_capacity(local.len() + remote.len());
    let (mut i, mut j) = (0, 0);
    while i < local.len() && j < remote.len() {
        if local[i].start_ms <= remote[j].start_ms {
            all.push(local[i].clone());
            i += 1;
        } else {
            all.push(remote[j].clone());
            j += 1;
        }
    }
    all.extend_from_slice(&local[i..]);
    all.extend_from_slice(&remote[j..]);
    all
}

fn overlap_ms(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> u64 {
    let start = a_start.max(b_start);
    let end = a_end.min(b_end);
    end.saturating_sub(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asr(start: u64, end: u64, text: &str) -> AsrSegment {
        AsrSegment { start_ms: start, end_ms: end, text: text.to_string() }
    }

    fn spk(start: u64, end: u64, who: &str) -> SpeakerSegment {
        SpeakerSegment { start_ms: start, end_ms: end, speaker: who.to_string() }
    }

    #[test]
    fn majority_overlap_wins() {
        let turns = attribute_speakers(
            &[asr(0, 1000, "hello there everyone")],
            &[spk(0, 300, "SPEAKER_00"), spk(300, 1000, "SPEAKER_01")],
            "UNKNOWN",
        );
        assert_eq!(turns[0].speaker, "SPEAKER_01");
    }

    #[test]
    fn no_overlap_falls_back() {
        let turns =
            attribute_speakers(&[asr(5000, 6000, "hm")], &[spk(0, 1000, "SPEAKER_00")], "UNKNOWN");
        assert_eq!(turns[0].speaker, "UNKNOWN");
    }

    #[test]
    fn tie_breaks_to_earlier_segment() {
        let turns = attribute_speakers(
            &[asr(0, 1000, "split evenly")],
            &[spk(0, 500, "SPEAKER_00"), spk(500, 1000, "SPEAKER_01")],
            "UNKNOWN",
        );
        assert_eq!(turns[0].speaker, "SPEAKER_00");
    }

    #[test]
    fn coalesce_merges_same_speaker_within_gap() {
        let turns = vec![
            Turn { speaker: "A".into(), text: "first part".into(), start_ms: 0, end_ms: 900 },
            Turn { speaker: "A".into(), text: "second part".into(), start_ms: 1100, end_ms: 2000 },
            Turn { speaker: "B".into(), text: "reply".into(), start_ms: 2100, end_ms: 3000 },
            Turn { speaker: "A".into(), text: "much later".into(), start_ms: 300_000, end_ms: 301_000 },
        ];
        let merged = coalesce_turns(turns, 1000);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].text, "first part second part");
        assert_eq!(merged[0].end_ms, 2000);
        assert_eq!(merged[2].text, "much later");
    }

    #[test]
    fn interleave_is_chronological_and_stable() {
        let local = vec![
            Turn { speaker: "Me".into(), text: "question".into(), start_ms: 0, end_ms: 1000 },
            Turn { speaker: "Me".into(), text: "thanks".into(), start_ms: 5000, end_ms: 6000 },
        ];
        let remote = vec![Turn {
            speaker: "SPEAKER_00".into(),
            text: "answer".into(),
            start_ms: 1500,
            end_ms: 4000,
        }];
        let all = interleave(local, remote);
        let texts: Vec<&str> = all.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, vec!["question", "answer", "thanks"]);
    }

    #[test]
    fn empty_inputs() {
        assert!(attribute_speakers(&[], &[], "U").is_empty());
        assert!(coalesce_turns(vec![], 1000).is_empty());
        assert!(interleave(vec![], vec![]).is_empty());
    }
}
