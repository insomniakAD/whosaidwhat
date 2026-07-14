"""Three-stage MapReduce summarization against a local oMLX server.

The repaired and hardened successor of the original ``pipeline.py``:

- the broken ``from db_extractor import ...`` (ModuleNotFoundError in the
  shipped logs) is gone — sources are explicit modules;
- sampling parameters follow the official Qwen3.6 model card instead of
  ad-hoc temperatures: strict stages use temperature=0.6/top_p=0.95/
  presence_penalty=0.0, the prose stage 0.7/0.8/1.5;
- thinking is disabled per request (``chat_template_kwargs``): single-shot
  summarization gains nothing from reasoning traces and they cost latency.
  The original's ``extra_body={"preserve_thinking": True}`` was misplaced —
  per the Qwen card that parameter retains reasoning across *multi-turn*
  conversations, which a stateless pipeline never has;
- short meetings skip the map stage entirely (single pass);
- a failed chunk raises instead of embedding "ERROR: ..." strings into the
  next stage's prompt (the original fed its own error messages to stage 2).
"""

from __future__ import annotations

import os

from .chunking import Turn, chunk_transcript, format_chunk

OMLX_BASE_URL = os.environ.get("OMLX_BASE_URL", "http://localhost:8000/v1")
MODEL_NAME = os.environ.get("WSW_SUMMARIZE_MODEL", "Qwen3.6-35B-A3B-oQ4e-mtp")

STAGE_1_PROMPT = """\
You are a strict data extraction engine analyzing a transcript chunk.
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
* [Speaker]: [Detail]"""

STAGE_2_PROMPT = """\
You are an expert synthesizer organizing raw data points into a cohesive outline.
Review the extraction reports. Merge related points, eliminate redundancies, and group them by theme.
Organize the final output into a standard markdown outline with Main Topics and Subtopics.
Preserve the [mm:ss] timestamps of the most important points so they remain citable.
Do not include introductory or concluding conversational text."""

STAGE_3_PROMPT = """\
You are an expert writer translating an outline into polished meeting notes.
Write in a direct, calm, and human tone.
Avoid all corporate filler and AI-speak. Do not use em dashes anywhere.
Structure the notes using clear H2 and H3 headings, ending with an "Action Items" section listing owner and task.
Do not include generic opening remarks or signatures."""

# Official Qwen3.6-35B-A3B card recommendations.
STRICT = {"temperature": 0.6, "top_p": 0.95, "presence_penalty": 0.0}
PROSE = {"temperature": 0.7, "top_p": 0.8, "presence_penalty": 1.5}


class SummarizeError(RuntimeError):
    pass


def _client():
    try:
        from openai import OpenAI  # type: ignore
    except ImportError as e:
        raise SummarizeError("openai package not installed: pip install openai") from e
    return OpenAI(base_url=OMLX_BASE_URL, api_key="not-needed")


def _run_stage(client, system_prompt: str, user_content: str, sampling: dict) -> str:
    try:
        response = client.chat.completions.create(
            model=MODEL_NAME,
            messages=[
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_content},
            ],
            extra_body={"chat_template_kwargs": {"enable_thinking": False}},
            **sampling,
        )
    except Exception as e:
        raise SummarizeError(
            f"oMLX request failed ({OMLX_BASE_URL}, model {MODEL_NAME}): {e}. "
            "Is the oMLX server running?"
        ) from e
    content = response.choices[0].message.content
    if not content:
        raise SummarizeError("oMLX returned an empty completion")
    return content


def summarize(
    turns: list[Turn],
    max_words: int = 1200,
    overlap_turns: int = 2,
    progress=lambda stage, done, total: None,
) -> dict:
    """Full pipeline: turns -> {'outline': str, 'notes': str, 'model': str}."""
    if not turns:
        raise SummarizeError("empty transcript")
    client = _client()

    total_words = sum(len(t.get("text", "").split()) for t in turns)
    if total_words <= max_words:
        # Short meeting: outline directly from the transcript, skip the map.
        progress("outline", 0, 1)
        outline = _run_stage(client, STAGE_2_PROMPT, format_chunk(turns), STRICT)
        progress("outline", 1, 1)
        progress("rewrite", 0, 1)
        notes = _run_stage(client, STAGE_3_PROMPT, outline, PROSE)
        progress("rewrite", 1, 1)
        return {"outline": outline, "notes": notes, "model": MODEL_NAME}

    chunks = chunk_transcript(turns, max_words=max_words, overlap_turns=overlap_turns)
    reports = []
    for i, chunk in enumerate(chunks):
        progress("extract", i, len(chunks))
        reports.append(_run_stage(client, STAGE_1_PROMPT, format_chunk(chunk), STRICT))
    progress("extract", len(chunks), len(chunks))

    progress("outline", 0, 1)
    combined = "\n\n=== NEXT CHUNK ===\n\n".join(reports)
    outline = _run_stage(client, STAGE_2_PROMPT, combined, STRICT)
    progress("outline", 1, 1)

    progress("rewrite", 0, 1)
    notes = _run_stage(client, STAGE_3_PROMPT, outline, PROSE)
    progress("rewrite", 1, 1)

    return {"outline": outline, "notes": notes, "model": MODEL_NAME}
