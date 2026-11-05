"""Razor dies after a command has done its work and before its result is recorded: on restart the
call's outcome is unknown, its job ends unknown, and the command is never run again."""
import json

from claude import reply
from driver import REPLIES, Reply, flow
import server

# The command's own last act kills the server, its parent, so no result can be recorded. It first
# waits for its job's start to be recorded, as any command that runs a moment does; on a busy
# machine the kill could otherwise land before it, leaving no job to end.
command = ("printf 'ran\\n' >> runs.txt; until grep -qs '\"event\":\"started\"' "
           "\"$HOME\"/.razor/sessions/*/record.jsonl; do sleep 0.02; done; kill -KILL $PPID")
done = Reply((REPLIES / "done.sse").read_bytes())
replies = [reply(("toolu_dies", "Bash", {"command": command})), done, done]
with flow(replies) as f:
    pid = server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    ask = server.post(f, f"/sessions/{session}/input", {"text": "Run the command once."})["entry"]
    server.gone(pid)
    before = list(f.record(session))
    assert [e["kind"] for e in before if e["kind"] in ("call", "result")] == ["call"], before
    assert (f.cwd / "runs.txt").read_text() == "ran\n"
    server.start(f)
    entries = server.until(f, session, lambda es: any(
        e["kind"] == "turn_end" and ask in e["inputs"] for e in es))
    entries = server.until(f, session, lambda es: any(
        e["kind"] == "turn_end" and any(i["id"] in e["inputs"] for i in es if i.get("from") == "razor")
        for e in es))
    notices = [e for e in entries if e.get("from") == "razor"]
    assert len(notices) == 1 and "toolu_dies" in notices[0]["text"], notices
    assert entries[:len(before)] == before, "the restart rewrote the record"
    end = next(e for e in entries if e["kind"] == "turn_end")
    assert end["end"] == "done", end
    calls = [e for e in entries if e["kind"] == "call"]
    outputs = [e for e in entries if e["kind"] == "result"]
    assert [e["call"] for e in calls] == ["toolu_dies"], calls
    assert len(outputs) == 1 and outputs[0]["call"] == "toolu_dies", outputs
    assert outputs[0]["outcome"] == "unknown" and outputs[0]["id"] > len(before), outputs
    jobs = [(e["event"], e.get("end")) for e in entries if e["kind"] == "job"]
    assert jobs == [("started", None), ("ended", "unknown")], jobs
    rebuilds = [e for e in entries if e["kind"] == "rebuild"]
    assert len(rebuilds) == 1 and rebuilds[0]["trigger"] == "restart", rebuilds
    assert (f.cwd / "runs.txt").read_text() == "ran\n", "the command ran again"
    fresh = json.loads((f.requests / "002.body").read_bytes())
    assert "runs.txt" in json.dumps(fresh["messages"]), "the rebuilt context omits the call"
    assert "toolu_dies" not in json.dumps(fresh["messages"]), "the old call was replayed as a tool_use"
