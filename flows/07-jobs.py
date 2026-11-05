"""Claude Code's job shapes over Razor's shell: a background Bash, `terminal` waiting on it and
looking at it, TaskStop, and a second job stopped at its own background timeout, whose end wakes
the session (docs/design/claude.md). Then a GPT turn, in which a job's end joins the running turn
(docs/design/controller.md)."""
import json
import os
import time
from driver import REPLIES, Reply, flow
from responses import PLAIN, call, done, response
from server import until

names = ("jobs-start", "jobs-wait", "jobs-peek", "jobs-kill", "jobs-second", "done", "done")
jobs = [call("bash", json.dumps({"command": f"sleep {seconds}; printf retained-output", "wait_ms": 0}), id)
        for seconds, id in ((1.5, "call_one"), (0.5, "call_two"))]
gpt = [response(jobs), response([call("wait", json.dumps({"job": 1, "timeout_ms": 10000}), "call_wait")]),
       response([call("kill", json.dumps({"job": 1}), "call_kill")]),
       done()]
with flow([Reply((REPLIES / f"{name}.sse").read_bytes()) for name in names] + gpt, config=PLAIN) as f:
    result = f.run("--print=json", "Start the background command; wait, look, stop it; start another.")
    assert result.returncode == 0, result.stderr
    session = json.loads(result.stdout)["session"]
    entries = list(f.record(session))
    calls = [e for e in entries if e["kind"] == "call"]
    assert [e["name"] for e in calls] == [
        "Bash", "mcp__razor__terminal", "mcp__razor__terminal", "TaskStop", "Bash"], calls
    outputs = [e for e in entries if e["kind"] == "result"]
    assert [e["call"] for e in outputs] == [e["call"] for e in calls], outputs
    assert all(e["outcome"] == "success" for e in outputs), outputs
    assert "job-1" in outputs[0]["output"], outputs[0]
    assert "ready" in outputs[1]["output"] and "ready" in outputs[2]["output"], outputs[1:3]
    assert "job-1" in outputs[3]["output"], outputs[3]
    assert "job-2" in outputs[4]["output"], outputs[4]
    # The second job is stopped at its one-second timeout, not the 30-minute default, and no
    # result shows its end, so a notice wakes the session.
    def answered(entries):
        notices = [e for e in entries if e["kind"] == "input" and e["from"] == "razor"]
        last = entries[-1]
        return notices and last["kind"] == "turn_end" and notices[0]["id"] in last["inputs"]

    entries = until(f, session, answered, timeout=20)
    notice = next(e["text"] for e in entries if e["kind"] == "input" and e["from"] == "razor")
    assert "<task-notification>" in notice and "job-2" in notice and "Second job" in notice, notice
    assert "<status>failed</status>" in notice and "Terminated by signal" in notice, notice
    request = json.loads((f.requests / "007.body").read_bytes())
    assert "<task-notification>" in json.dumps(request["messages"]), "the notice missed the model"
    jobs = [e for e in entries if e["kind"] == "job"]
    started = {e["job"]: e for e in jobs if e["event"] == "started"}
    ended = {e["job"]: e["end"] for e in jobs if e["event"] == "ended"}
    assert sorted(started) == sorted(ended) == [1, 2], jobs
    assert "signal" in ended[1] and "signal" in ended[2], ended
    try:
        os.kill(started[1]["pid"], 0)
    except ProcessLookupError:
        pass
    else:
        raise AssertionError(f"job-1's leader {started[1]['pid']} survived TaskStop")

    # Job-2 ends while the turn waits on job-1: its notice joins the turn at that call and reaches
    # the next request, not the turn's end (Trey, 2026-10-30). Job-1's end is the wait's own
    # result, so no notice repeats it.
    result = f.run("--model", "gpt-6.1-sol", "--print=json", "Start two jobs; wait on the first.")
    assert result.returncode == 0, result.stderr
    entries = list(f.record(json.loads(result.stdout)["session"]))
    notices = [e for e in entries if e["kind"] == "input" and e["from"] == "razor"]
    assert len(notices) == 1 and notices[0]["text"].startswith("Job job-2 "), notices
    ends = [e for e in entries if e["kind"] == "turn_end"]
    assert len(ends) == 1 and notices[0]["id"] in ends[0]["inputs"], ends
    waited = json.loads(next(e["output"] for e in entries if e["kind"] == "result" and e["call"] == "call_wait"))
    assert waited["job"] == 1 and waited["end"], waited
    killed = json.loads(next(e["output"] for e in entries if e["kind"] == "result" and e["call"] == "call_kill"))
    assert killed["job"] == 1 and killed["end"] == waited["end"] and killed["output"] == "retained-output", killed
    last = json.loads((f.requests / "010.body").read_bytes())["input"][-1]
    assert "Job job-2 " in json.dumps(last), last

# Monitor's trained command shape starts an ordinary job; its exit wakes the session once.
from claude import reply
from server import post, start
with flow([reply(("monitor", "mcp__razor__monitor", {"description": "Wait for release", "timeout_ms": 10000,
                                        "command": "while [ ! -e release ]; do sleep 0.05; done; printf monitor-ready"})),
           reply(text="armed"), reply(text="flow done")]) as f:
    result = f.run("--print=json", "Monitor release.")
    assert result.returncode == 0, result.stderr
    session = json.loads(result.stdout)["session"]
    entries = list(f.record(session))
    output = next(e for e in entries if e["kind"] == "result" and e["call"] == "monitor")
    assert output["outcome"] == "success" and "job-1" in output["output"], output
    tools = {t["name"] for t in json.loads((f.requests / "001.body").read_bytes())["tools"]}
    assert "mcp__razor__monitor" in tools and "Monitor" not in tools, tools
    (f.cwd / "release").touch()
    entries = until(f, session, answered)
    notice = next(e["text"] for e in entries if e["kind"] == "input" and e["from"] == "razor")
    assert "<status>completed</status>" in notice and "Wait for release" in notice, notice
    assert (f.home / ".razor/sessions" / session / "job-1").read_text() == "monitor-ready"

# A job ending during agent wait is consumed by that call, without a timeout or duplicate input;
# `/done` is not news, so a later wait leaves it and the closing turn runs after that wait's turn.
with flow([response([call("bash", json.dumps({"command": "printf MARKER; while [ ! -e cursor_release ]; do sleep 0.05; done; printf MARKER; while [ ! -e release ]; do sleep 0.05; done; printf released",
                                              "wait_ms": 10000, "pattern": "MARKER", "deadline_ms": 30000}), "background")]),
           response([call("bash", json.dumps({"command": "true", "wait_ms": 10000}), f"churn_{n}") for n in range(64)]),
           response([call("wait", json.dumps({"job": 1, "pattern": "MARKER", "timeout_ms": 5000}), "cursor_wait")]),
           response([call("agent", json.dumps({"action": "wait", "timeout_ms": 3000}), "news")]), done(),
           response([call("agent", json.dumps({"action": "wait", "timeout_ms": 3000}), "idle")]), done(), done()],
          model="gpt-6.1-sol", config=PLAIN) as f:
    start(f)
    session = post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    post(f, f"/sessions/{session}/input", {"text": "Wait for job news."})
    entries = until(f, session, lambda es: any(e.get("call") == "cursor_wait" and e["kind"] == "call" for e in es))
    results = {e["call"]: json.loads(e["output"]) for e in entries if e["kind"] == "result"}
    assert results["background"]["output"] == "MARKER" and results["background"]["end"] is None, results["background"]
    churn = [results[f"churn_{n}"] for n in range(64)]
    assert len({r["job"] for r in churn}) == 64 and all(r["job"] > 1 and r["end"] == {"exit": 0} for r in churn), churn
    time.sleep(0.5)
    early = [e for e in f.record(session) if e["kind"] == "result" and e.get("call") == "cursor_wait"]
    assert not early, f"pattern wait returned before new output: {early}"
    (f.cwd / "cursor_release").touch()
    entries = until(f, session, lambda es: any(e.get("call") == "news" and e["kind"] == "call" for e in es))
    waited = json.loads(next(e["output"] for e in entries if e["kind"] == "result" and e.get("call") == "cursor_wait"))
    assert waited["output"] == "MARKERMARKER" and waited["end"] is None, waited
    (f.cwd / "release").touch()
    entries = until(f, session, lambda es: any(e["kind"] == "turn_end" for e in es))
    result = next(e for e in entries if e["kind"] == "result" and e["call"] == "news")
    notice = json.loads(result["output"])
    assert notice and notice["from"] == "razor" and "Job job-1 Exit code 0" in notice["text"], result
    assert result["inputs"] == [notice["id"]], result
    assert len([e for e in entries if e["kind"] == "input" and e["from"] == "razor"]) == 1, entries
    post(f, f"/sessions/{session}/input", {"text": "Wait again."})
    until(f, session, lambda es: any(e.get("call") == "idle" and e["kind"] == "call" for e in es))
    closing = post(f, f"/sessions/{session}/done", {})["entry"]
    entries = until(f, session, lambda es: es[-1].get("closed"))
    waited = next(e for e in entries if e["kind"] == "result" and e["call"] == "idle")
    ends = [e for e in entries if e["kind"] == "turn_end"]
    assert closing not in waited.get("inputs", []) and ends[-1]["inputs"] == [closing], (waited, ends[-2:])
