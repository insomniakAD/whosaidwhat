//! Three-stage MapReduce summarization (Rust port of pipeline/wsw/summarize.py,
//! same prompts, same stage structure, same chunk boundaries via llm::chunk).
//!
//! Stage 1 (map): per-chunk structured extraction — decisions, arguments,
//!   data points, action items, with speakers.
//! Stage 2 (reduce): merge all extraction reports into a deduplicated outline.
//! Stage 3 (rewrite): outline → polished prose.
//!
//! Why chunk at all when Qwen3.6 has 262K native context? Two reasons,
//! recorded after measurement is possible: (1) map-stage extraction with a
//! strict format catches per-chunk specifics that single-pass summaries of
//! multi-hour meetings blur; (2) chunks bound worst-case prompt-processing
//! latency and keep the UI's progress bar honest. `single_pass` exists for
//! short meetings (< max_words) where MapReduce is pure overhead.

use super::chunk::{chunk_turns, format_chunk, Turn};
use super::client::LlmError;
use super::router::{InferenceRouter, RoutedModel, SamplingProfile};

pub const STAGE_1_PROMPT: &str = r#"You are a strict data extraction engine analyzing a transcript chunk.
Extract all substantive decisions, arguments, data points, and action items.
Use concise, complete sentences. Include the speaker's name.
Do not write a narrative summary. If a category has no relevant information, write "None".

Required Output Format:
Decisions Made:
* [Speaker]: [Detail]

Key Arguments & Discussion:
* [Speaker]: [Detail]

Data & Metrics Discussed:
* [Speaker]: [Detail]

Action Items:
* [Speaker]: [Detail]"#;

pub const STAGE_2_PROMPT: &str = r#"You are an expert synthesizer organizing raw data points into a cohesive outline.
Review the extraction reports. Merge related points, eliminate redundancies, and group them by theme.
Organize the final output into a standard markdown outline with Main Topics and Subtopics.
Preserve the [mm:ss] timestamps of the most important points so they remain citable.
Do not include introductory or concluding conversational text."#;

pub const STAGE_3_PROMPT: &str = r#"You are an expert writer translating an outline into polished meeting notes.
Write in a direct, calm, and human tone.
Avoid all corporate filler and AI-speak. Do not use em dashes anywhere.
Structure the notes using clear H2 and H3 headings, ending with an "Action Items" section listing owner and task.
Keep the [mm:ss] timestamp markers from the outline attached to the points they belong to; do not invent new ones.
Do not include generic opening remarks or signatures."#;

#[derive(Debug, Clone)]
pub struct SummarizeConfig {
    pub max_words_per_chunk: usize,
    pub overlap_turns: usize,
}

impl Default for SummarizeConfig {
    fn default() -> Self {
        SummarizeConfig { max_words_per_chunk: 1200, overlap_turns: 2 }
    }
}

/// Progress callback: (stage, done, total).
pub type Progress<'a> = &'a mut (dyn FnMut(&str, usize, usize) + Send);

pub struct SummaryOutput {
    pub outline: String,
    pub notes: String,
    /// Which model produced it (for the DB provenance columns).
    pub model: RoutedModel,
}

pub async fn summarize(
    router: &InferenceRouter,
    turns: &[Turn],
    config: &SummarizeConfig,
    progress: Progress<'_>,
) -> Result<SummaryOutput, LlmError> {
    let routed = router.resolve_summarize_model().await?;

    let total_words: usize = turns.iter().map(|t| t.text.split_whitespace().count()).sum();
    if total_words <= config.max_words_per_chunk {
        // Short meeting: single pass, skip the map stage entirely.
        progress("extract", 0, 1);
        let text = format_chunk(turns);
        let outline =
            router.summarize_stage(&routed, STAGE_2_PROMPT, &text, SamplingProfile::Strict).await?;
        progress("extract", 1, 1);
        progress("rewrite", 0, 1);
        let notes = router
            .summarize_stage(&routed, STAGE_3_PROMPT, &outline, SamplingProfile::Prose)
            .await?;
        progress("rewrite", 1, 1);
        return Ok(SummaryOutput { outline, notes, model: routed });
    }

    // Stage 1: map.
    let chunks = chunk_turns(turns, config.max_words_per_chunk, config.overlap_turns);
    let total = chunks.len();
    let mut reports = Vec::with_capacity(total);
    for (i, chunk) in chunks.iter().enumerate() {
        progress("extract", i, total);
        let text = format_chunk(chunk);
        let report =
            router.summarize_stage(&routed, STAGE_1_PROMPT, &text, SamplingProfile::Strict).await?;
        reports.push(report);
    }
    progress("extract", total, total);

    // Stage 2: reduce.
    progress("outline", 0, 1);
    let combined = reports.join("\n\n=== NEXT CHUNK ===\n\n");
    let outline =
        router.summarize_stage(&routed, STAGE_2_PROMPT, &combined, SamplingProfile::Strict).await?;
    progress("outline", 1, 1);

    // Stage 3: rewrite.
    progress("rewrite", 0, 1);
    let notes =
        router.summarize_stage(&routed, STAGE_3_PROMPT, &outline, SamplingProfile::Prose).await?;
    progress("rewrite", 1, 1);

    Ok(SummaryOutput { outline, notes, model: routed })
}
