"""At the minute's check, a working session whose record has sat past `stuck.after` with its input
unanswered shows as stuck, and one inside an `agent wait` before its deadline stays working."""
from dataclasses import replace
import time
from driver import flow
from model import agent, reply
import server

# The slow reply, about 9 s, spans the check a minute after the server starts; the stand-in
# answers one request at a time, so it starts late to hold the stand-in only briefly.
slow = replace(reply("SLOW66", text="slow66" + "." * 400), cuts=tuple(range(1, 300)))
with flow([agent("WAIT66", "wait", timeout_ms=600000), slow], config='[stuck]\nafter = "1s"') as f:
    server.start(f)
    started, made = time.monotonic(), {}
    for text in ("WAIT66", "SLOW66"):
        time.sleep(max(0, started + 55 - time.monotonic()) * (text == "SLOW66"))
        made[text] = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        server.post(f, f"/sessions/{made[text]}/input", {"text": text})
        server.until(f, made["WAIT66"], lambda es: any(e["kind"] == "call" for e in es))
    while (rows := server.agents(f))[made["SLOW66"]]["state"] != "stuck":
        assert rows[made["SLOW66"]]["state"] == "working", rows
        time.sleep(0.2)
    assert rows[made["WAIT66"]]["state"] == "working", rows
    server.post(f, f"/sessions/{made['WAIT66']}/stop", {})
    server.until(f, made["SLOW66"], lambda es: any(e["kind"] == "turn_end" for e in es))
