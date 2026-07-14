import os
from openai import OpenAI

# NOTE: If you saved your previous script as 'meetily_db_extractor.py', 
# change the import below to: from meetily_db_extractor import ...
from db_extractor import get_latest_transcript, DB_PATH

# 1. Configuration for local oMLX server
OMLX_BASE_URL = "http://localhost:8000/v1"
MODEL_NAME = "Qwen3.6-35B-A3B-oQ4e-mtp"

# Initialize local client
client = OpenAI(base_url=OMLX_BASE_URL, api_key="not-needed")

# 2. System Prompts
STAGE_1_PROMPT = """
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
* [Speaker]: [Detail]
"""

STAGE_2_PROMPT = """
You are an expert synthesizer organizing raw data points into a cohesive outline.
Review the extraction reports. Merge related points, eliminate redundancies, and group them by theme.
Organize the final output into a standard markdown outline with Main Topics and Subtopics.
Do not include introductory or concluding conversational text.
"""

STAGE_3_PROMPT = """
You are an expert writer translating an outline into a polished blog post.
Write in a direct, calm, and human tone.
Avoid all corporate filler and AI-speak. Do not use em dashes anywhere.
Structure the post using clear H2 and H3 headings.
Do not include generic opening remarks or email signatures.
"""

# 3. Chunking Logic
def chunk_transcript(turns, max_words=1200, overlap_turns=2):
    """
    Splits the transcript into semantic chunks based on word count, 
    injecting an overlap of previous turns to maintain context.
    """
    chunks = []
    current_chunk = []
    current_word_count = 0
    
    for turn in turns:
        text = turn.get("text", "")
        turn_words = len(text.split())
        
        # If adding this turn exceeds the limit, finalize the current chunk
        if current_word_count + turn_words > max_words and current_chunk:
            chunks.append(current_chunk)
            
            # Start a new chunk with the overlap from the previous turns
            safe_overlap = min(overlap_turns, len(current_chunk))
            current_chunk = current_chunk[-safe_overlap:] if safe_overlap > 0 else []
            current_word_count = sum(len(t.get("text", "").split()) for t in current_chunk)
        
        current_chunk.append(turn)
        current_word_count += turn_words
        
    # Append any remaining turns as the final chunk
    if current_chunk:
        chunks.append(current_chunk)
        
    return chunks

def format_chunks(chunks):
    """Formats the list of turn dictionaries into plain text blocks for the LLM."""
    formatted = []
    for chunk in chunks:
        text = "".join(f"{turn['speaker']}: {turn['text']}\n\n" for turn in chunk)
        formatted.append(text.strip())
    return formatted

# 4. Pipeline Execution Steps
def run_stage(system_prompt, user_content, temp=0.1):
    """Wrapper function to execute a prompt against the local oMLX server."""
    try:
        response = client.chat.completions.create(
            model=MODEL_NAME,
            messages=[
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_content}
            ],
            temperature=temp,
            # Qwen specific parameter to retain reasoning traces
            extra_body={"preserve_thinking": True} 
        )
        return response.choices[0].message.content
    except Exception as e:
        print(f"\n[!] Error communicating with oMLX: {e}")
        print("Please ensure your oMLX server is running on localhost:8000")
        return f"ERROR: {e}"

def process_transcript(turns):
    """Runs the full MapReduce pipeline: Chunk -> Extract -> Outline -> Rewrite."""
    print("\n[+] Step 1: Chunking transcript...")
    raw_chunks = chunk_transcript(turns)
    formatted_chunks = format_chunks(raw_chunks)
    
    print(f"[+] Created {len(formatted_chunks)} chunks. Starting Stage 1 (Map/Extraction)...")
    extracted_reports = []
    for i, chunk in enumerate(formatted_chunks):
        print(f"  -> Processing chunk {i+1} of {len(formatted_chunks)}...")
        report = run_stage(STAGE_1_PROMPT, chunk, temp=0.1)
        extracted_reports.append(report)
        
    print("\n[+] Stage 1 Complete. Starting Stage 2 (Reduce/Outline)...")
    # Merge all chunk reports into one giant string for the reduction phase
    combined_reports = "\n\n=== NEXT CHUNK ===\n\n".join(extracted_reports)
    outline = run_stage(STAGE_2_PROMPT, combined_reports, temp=0.2)
    
    print("\n[+] Stage 2 Complete. Starting Stage 3 (Final Rewrite)...")
    final_post = run_stage(STAGE_3_PROMPT, outline, temp=0.6)
    
    return final_post

# 5. Main Entry Point
if __name__ == "__main__":
    print("Fetching latest meeting from Meetily database...")
    
    # Use the function from our db_extractor script
    transcript_turns = get_latest_transcript(DB_PATH)
    
    if not transcript_turns:
        print("Pipeline aborted. No transcript found or database error.")
    else:
        print(f"Found {len(transcript_turns)} turns. Initiating MapReduce Pipeline...\n")
        
        # Run the pipeline
        final_result = process_transcript(transcript_turns)
        
        # Save the result
        output_filename = "meeting_blog_post.md"
        with open(output_filename, "w") as f:
            f.write(final_result)
            
        print(f"\nSUCCESS! Fully polished blog post saved to {output_filename}")

        # Notification trigger
        os.system(f'osascript -e \'display notification "Summary saved: {output_filename}" with title "Meetily Summarizer" sound name "Glass"\'')