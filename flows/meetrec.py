#!/usr/bin/env python3
"""A recorder process for flow 71; the flow controls its transcript and batch outcome."""
import json
import os
from pathlib import Path
import select
import sys
import time
import uuid
import wave

args = sys.argv[1:]
root = Path.home() / "Recordings/meetings"
root.mkdir(parents=True, exist_ok=True)
with (root / "commands.jsonl").open("a") as log:
    log.write(json.dumps({"pid": os.getpid(), "args": args}) + "\n")

def answer(**fields):
    print(json.dumps({"schema_version": 1, "ok": True, "error": None, **fields}), flush=True)

if args[0] == "capabilities":
    answer(commands={"ingest": {}})
elif args[0] == "start":
    key = args[args.index("--request") + 1] if "--request" in args else str(uuid.uuid4())
    folder = Path(args[args.index("--dir") + 1]) / key if "--dir" in args else root / key
    if not folder.exists():
        (folder / "pieces").mkdir(parents=True)
        with wave.open(str(folder / "room.live.wav"), "wb") as wav:
            wav.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
        (folder / "session.json").write_text(json.dumps({"phase": "recording", "request": key}))
        rate = int(args[args.index("--rate") + 1])
        (folder / "clock.json").write_text(json.dumps({"tracks": {"room": {"sample_rate": rate}}}))
    lost = root / "lose-start-reply"
    if lost.exists():
        lost.unlink()
        sys.exit(1)
    answer(session={"id": folder.name, "path": str(folder)}, ingest={
        "live_file": "room.live.wav", "format": {"rate": 16000, "channels": 1, "sample_width": 2, "encoding": "pcm_s16le"},
        "pieces_dir": "pieces", "complete_file": "pieces/complete.json", "pauses_file": "pauses.json",
        "speakers_file": "speakers.json", "final_file": "final.json"})
elif args[0] == "live":
    folder = Path(args[1])
    cursor = folder / "flow-cursor"
    live = folder / "live.jsonl"
    while True:
        rows = live.read_text().splitlines() if live.exists() else []
        at = int(cursor.read_text()) if cursor.exists() else 0
        for row in rows[at:]:
            print(row, flush=True)
            at += 1
            cursor.write_text(str(at))
        print(json.dumps({"kind": "status", "live": {"alive": True, "lag_s": 1.5}}), flush=True)
        if "--follow" not in args or select.select([sys.stdin], [], [], .2)[0]:
            break
elif args[0] in ("ingest", "stop", "cancel"):
    state = {"stop": "stopped", "cancel": "canceled"}.get(args[0], "recording")
    if args[0] in ("stop", "cancel"):
        (Path(args[1]) / "session.json").write_text(json.dumps({"phase": state}))
    if "--retry" in args:
        (Path(args[1]) / "final.json").unlink(missing_ok=True)  # as meetrec does before it returns
    answer(state=state)
else:
    sys.exit(2)
