#!/usr/bin/env python3
"""whosaidwhat pipeline CLI — audio in, diarized transcript + summary out.

Two entry modes:

  From a recording (the whosaidwhat/Granola-style path):
      python3 run.py audio path/to/meeting.system.wav \
          [--mic path/to/meeting.mic.wav] [--speakers 3] [--title "Weekly sync"]

  From an existing Meetily CE database (the original pipeline's path,
  kept working — speaker labels come from Meetily, no diarization run):
      python3 run.py meetily [--db /path/to/meeting_minutes.sqlite]

Output: transcript + summaries stored in the whosaidwhat SQLite DB
(default ~/whosaidwhat.sqlite, override with --out-db) and a markdown file
written next to the audio (exportable, the DB is the index — see
docs/03-database-schema.md).

Env knobs: OMLX_BASE_URL, WSW_SUMMARIZE_MODEL, WSW_DIARIZE_MODEL,
WSW_DIARIZE_DEVICE (cpu|mps), HF_TOKEN (only if using the gated pyannote repo).
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import time

from wsw import merge, store
from wsw.chunking import Turn


def notify(message: str) -> None:
    """Best-effort macOS notification; silent no-op elsewhere.

    The message is user-controlled (meeting title / filename), so it must be
    escaped before it is embedded in AppleScript source. Passing argv as a list
    stops *shell* injection but not *AppleScript* injection: a title like
    `x" & (do shell script "…") & "` would still execute. Escaping backslashes
    and double quotes closes that (the original pipeline's os.system f-string
    had exactly this hole).
    """
    if sys.platform != "darwin":
        return
    safe = message.replace("\\", "\\\\").replace('"', '\\"')
    script = f'display notification "{safe}" with title "whosaidwhat" sound name "Glass"'
    subprocess.run(["osascript", "-e", script], check=False, capture_output=True)


def cmd_audio(args: argparse.Namespace) -> int:
    from wsw.diarize import diarize
    from wsw.summarize import summarize
    from wsw.transcribe import transcribe

    t0 = time.time()
    print(f"[1/4] Transcribing {args.audio} ...")
    system_asr = transcribe(args.audio, language=args.language)
    print(f"      {len(system_asr)} segments in {time.time() - t0:.0f}s")

    print("[2/4] Diarizing (local pyannote community-1) ...")
    t1 = time.time()
    speakers = diarize(args.audio, num_speakers=args.speakers or None)
    n_speakers = len({s["speaker"] for s in speakers})
    print(f"      {n_speakers} speakers, {len(speakers)} turns in {time.time() - t1:.0f}s")

    remote = merge.coalesce_turns(merge.attribute_speakers(system_asr, speakers))

    local: list[Turn] = []
    if args.mic:
        print(f"      Transcribing mic track {args.mic} (speaker = Me) ...")
        mic_asr = transcribe(args.mic, language=args.language)
        local = merge.coalesce_turns(
            [
                Turn(speaker="Me", text=s["text"], start_ms=s["start_ms"], end_ms=s["end_ms"])
                for s in mic_asr
            ]
        )

    turns = merge.interleave(local, remote)
    print(f"[3/4] Summarizing {len(turns)} turns via oMLX ...")

    def progress(stage: str, done: int, total: int) -> None:
        print(f"      {stage}: {done}/{total}", end="\r", flush=True)

    result = summarize(turns, progress=progress)
    print()

    title = args.title or os.path.splitext(os.path.basename(args.audio))[0]
    conn = store.open_store(args.out_db)
    meeting_id = store.save_meeting(
        conn,
        title=title,
        turns=turns,
        outline=result["outline"],
        notes=result["notes"],
        model=result["model"],
        app=args.app,
        audio_system_path=os.path.abspath(args.audio),
        audio_mic_path=os.path.abspath(args.mic) if args.mic else None,
    )

    md_path = os.path.splitext(args.audio)[0] + ".notes.md"
    with open(md_path, "w") as f:
        f.write(f"# {title}\n\n{result['notes']}\n")

    print(f"[4/4] Done. Meeting {meeting_id} stored in {args.out_db}; notes at {md_path}")
    notify(f"Summary ready: {title}")
    return 0


def cmd_meetily(args: argparse.Namespace) -> int:
    from wsw.meetily_source import DEFAULT_DB_PATH, get_latest_transcript
    from wsw.summarize import summarize

    db_path = args.db or DEFAULT_DB_PATH
    print(f"[1/3] Reading latest meeting from Meetily DB: {db_path}")
    meeting, turns = get_latest_transcript(db_path)
    if meeting is None or not turns:
        print("No transcript found. Nothing to do.")
        return 1
    print(f"      '{meeting['title']}' — {len(turns)} turns (speakers from Meetily)")

    print("[2/3] Summarizing via oMLX ...")

    def progress(stage: str, done: int, total: int) -> None:
        print(f"      {stage}: {done}/{total}", end="\r", flush=True)

    result = summarize(turns, progress=progress)
    print()

    conn = store.open_store(args.out_db)
    meeting_id = store.save_meeting(
        conn,
        title=meeting["title"],
        turns=turns,
        outline=result["outline"],
        notes=result["notes"],
        model=result["model"],
        app="meetily-import",
    )
    print(f"[3/3] Done. Stored as meeting {meeting_id} in {args.out_db}")
    notify(f"Summary ready: {meeting['title']}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)

    # --out-db is added to each subparser (not the parent) so it can appear
    # after the subcommand, matching the documented usage
    # `run.py audio file.wav --out-db X`. A parent-parser option would have to
    # precede the subcommand, which the examples don't.
    def add_common(p: argparse.ArgumentParser) -> None:
        p.add_argument(
            "--out-db",
            default=os.path.expanduser("~/whosaidwhat.sqlite"),
            help="whosaidwhat SQLite DB to write into",
        )

    sub = parser.add_subparsers(dest="command", required=True)

    p_audio = sub.add_parser("audio", help="diarize + summarize a recording")
    add_common(p_audio)
    p_audio.add_argument("audio", help="system-audio WAV (remote participants)")
    p_audio.add_argument("--mic", help="optional mic-track WAV (local user)")
    p_audio.add_argument("--speakers", type=int, default=0, help="known speaker count (0 = auto)")
    p_audio.add_argument("--language", default=None, help="language hint, e.g. en")
    p_audio.add_argument("--title", default=None)
    p_audio.add_argument("--app", default=None, help="zoom|teams|meet|manual")
    p_audio.set_defaults(func=cmd_audio)

    p_meetily = sub.add_parser("meetily", help="summarize the latest Meetily CE meeting")
    add_common(p_meetily)
    p_meetily.add_argument("--db", default=None, help="path to Meetily's meeting_minutes.sqlite")
    p_meetily.set_defaults(func=cmd_meetily)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
