import sqlite3
import os

# The exact path to your local Meetily database
DB_PATH = "/Users/papp/Library/Application Support/com.meetily.ai/meeting_minutes.sqlite"

def inspect_schema(cursor):
    """Prints the database tables and columns to verify the schema."""
    print("=== Database Schema ===\n")
    cursor.execute("SELECT name FROM sqlite_master WHERE type='table';")
    tables = cursor.fetchall()
    
    for table in tables:
        table_name = table[0]
        print(f"Table: {table_name}")
        cursor.execute(f"PRAGMA table_info({table_name});")
        columns = cursor.fetchall()
        for col in columns:
            print(f"  - {col[1]} ({col[2]})")
        print("-" * 20)
    print("\n=======================\n")

def get_latest_transcript(db_path):
    if not os.path.exists(db_path):
        print(f"Database not found at {db_path}")
        return []

    # Connect to the SQLite database
    conn = sqlite3.connect(db_path, check_same_thread=False)
    conn.row_factory = sqlite3.Row 
    cursor = conn.cursor()

    try:
        # Step 1: Find the most recent meeting ID
        # NOTE: If the script fails here, check the output of inspect_schema() 
        # and update 'meetings' and 'created_at' to match the actual column names.
        cursor.execute("""
            SELECT id, title 
            FROM meetings 
            ORDER BY created_at DESC 
            LIMIT 1
        """)
        latest_meeting = cursor.fetchone()
        
        if not latest_meeting:
            print("No meetings found in the database.")
            return []
            
        meeting_id = latest_meeting['id']
        meeting_title = latest_meeting['title'] if 'title' in latest_meeting.keys() else "Untitled"
        
        # --- NEW LOGIC: Check if already processed ---
        tracking_file = os.path.join(os.path.dirname(os.path.abspath(__file__)), "last_processed.txt")
        if os.path.exists(tracking_file):
            with open(tracking_file, "r") as f:
                last_id = f.read().strip()
                if last_id == meeting_id:
                    print(f"Meeting {meeting_id} already processed. Exiting.")
                    return [] # Return empty to stop pipeline

        # If it's a new meeting, update the tracking file
        with open(tracking_file, "w") as f:
            f.write(meeting_id)
        # -------------------------------------------
        
        print(f"Pulling transcript for: {meeting_title} (ID: {meeting_id})")

        # Step 2: Pull the transcript segments for that meeting
        # NOTE: Updated to use 'transcript' instead of 'text' based on the schema
        cursor.execute("""
            SELECT speaker, transcript 
            FROM transcripts 
            WHERE meeting_id = ? 
            ORDER BY timestamp ASC
        """, (meeting_id,))
        
        raw_rows = cursor.fetchall()
        
        # Step 3: Format the rows into the dictionary list for the pipeline
        turns = []
        for row in raw_rows:
            turns.append({
                "speaker": row['speaker'] if row['speaker'] else "Unknown",
                "text": row['transcript'] # Map 'transcript' from DB to 'text' for the pipeline
            })
            
        return turns

    except sqlite3.Error as e:
        print(f"Database query error: {e}")
        print("\nTIP: The table or column names might be different. Run inspect_schema() to check.")
        return []
    finally:
        conn.close()

if __name__ == "__main__":
    # Quick connection test and schema printout
    if os.path.exists(DB_PATH):
        print("Database found! Connecting...\n")
        test_conn = sqlite3.connect(DB_PATH)
        inspect_schema(test_conn.cursor())
        test_conn.close()
        
        # Attempt to fetch the latest transcript
        print("Attempting to fetch the latest meeting...")
        transcript_turns = get_latest_transcript(DB_PATH)
        
        if transcript_turns:
            print(f"\nSuccess! Extracted {len(transcript_turns)} turns.")
            print("\nSample of first 3 turns:")
            for i, turn in enumerate(transcript_turns[:3]):
                print(f"[{i+1}] {turn['speaker']}: {turn['text'][:60]}...")
    else:
        print(f"Could not find the database at: {DB_PATH}")
        print("Please double-check the path and permissions.")