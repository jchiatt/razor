"""Resume by id, continue by directory, and all three headless formats."""
import json
import os
import signal
import server
from driver import REPLIES, Reply, flow

replies = [Reply((REPLIES / f"{name}.sse").read_bytes())
           for name in ("hello", "resumed", "continued", "stream")]
with flow(replies) as f:
    first = f.run("--print=json", "First ask.")
    assert first.returncode == 0, first.stderr
    session = json.loads(first.stdout)["session"]
    os.kill(server.start(f), signal.SIGTERM)
    server.gone(json.loads((f.home / ".razor/server.json").read_text())["pid"])
    before = list(f.record(session))
    resumed = f.run("--resume", session, "--print=json", "Second ask.")
    assert resumed.returncode == 0, resumed.stderr
    assert json.loads(resumed.stdout) == {"session": session, "message": "resumed reply"}
    assert list(f.record(session))[:len(before)] == before, "resume rewrote the record"
    continued = f.run("--continue", "--print=text", input="Third ask.")
    assert continued.returncode == 0, continued.stderr
    assert continued.stdout == "continued reply\n", continued.stdout
    prior = list(f.record(session))
    streamed = f.run("--resume", session, "--print=stream", "Fourth ask.")
    assert streamed.returncode == 0, streamed.stderr
    events = [json.loads(line) for line in streamed.stdout.splitlines()]
    assert events[0]["kind"] == "attached" and events[0]["session"] == session, events
    entries = list(f.record(session))
    # Entries reach the stream without `native`, the model path's replay data.
    assert [e for e in events if "id" in e] == [{k: v for k, v in e.items() if k != "native"} for e in entries[len(prior):]], events
    assert events[-1]["kind"] == "turn_end", events[-1]
    assert [e["text"] for e in entries if e["kind"] == "input"] == [
        "First ask.", "Second ask.", "Third ask.", "Fourth ask."]
    assert len([e for e in entries if e["kind"] == "start"]) == 1, entries
    requests = [json.loads((f.requests / f"{i:03}.body").read_bytes()) for i in range(1, 5)]
    assert all("First ask." in json.dumps(request["messages"]) for request in requests), requests
