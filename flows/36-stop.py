"""Real app: Ctrl-C detaches; resumed Escape stops the foreground group, not the app, and the
next inputs continue the same native conversation, with no rebuild. An input sent to the model
path before Escape runs as the next turn there: the session stays working, a second Escape stops
it and its job, and the session then answers what comes next. News a code-mode cell's calls
take reaches the model through the next Result that reads the cell, or after a stop as an input."""
import os
import signal
import subprocess
import time

from claude import reply
from driver import Reply, flow
from responses import PLAIN, call, done, response
import json
import server
from terminal import Terminal


def path(pid, sig):
    """Signals the model path's processes: those of the server's process group but the server."""
    rows = [row.split() for row in subprocess.check_output(["ps", "-eo", "pid=,pgid="], text=True).splitlines()]
    for p in [int(p) for p, g in rows if int(g) == pid and int(p) != pid]:
        os.kill(p, sig)


for model in ("opus", "gpt-6.1-sol"):
    command = "sleep 600 & printf ready > ready; wait"
    replies = ([reply(("ui_stop", "Bash", {"command": command})), reply(text="flow done")] if model == "opus" else
               [response([call("bash", json.dumps({"command": command}), "ui_stop")]), done()])
    with flow(replies, model=model, config=PLAIN if model != "opus" else "") as f:
        server.start(f)
        session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        with Terminal(f, "--resume", session) as t:
            t.shown("Message the agent")
            t.send("Run the foreground job.\r")
            entries = server.until(f, session, lambda es: (f.cwd / "ready").exists()
                                   and any(e["kind"] == "job" for e in es))
            job = next(e for e in entries if e["kind"] == "job")
            members = [row.split() for row in subprocess.check_output(
                ["ps", "-eo", "pid=,pgid=,stat="], text=True).splitlines()
                if int(row.split()[1]) == job["pid"] and not row.split()[2].startswith("Z")]
            assert len(members) >= 2, members
        os.kill(job["pid"], 0)
        assert not any(e["kind"] == "turn_end" for e in f.record(session))
        with Terminal(f, "--resume", session) as t:
            t.shown("Esc stops")
            # A first Ctrl-C only asks for a second; Esc then stays without stopping the job.
            t.send(b"\x03")
            t.shown("PRESS CTRL-C AGAIN TO QUIT")
            os.kill(t.pid, 0)
            t.send(b"\x1b")
            t.until(lambda t: "PRESS CTRL-C AGAIN TO QUIT" not in t.text())
            t.pump(1)
            os.kill(job["pid"], 0)
            assert not any(e["kind"] == "turn_end" for e in f.record(session))
            t.send(b"\x1b")
            server.until(f, session, lambda es: any(e["kind"] == "turn_end" and e["end"] == "stopped"
                         for e in es) and any(e["kind"] == "job" and e["event"] == "ended" for e in es))
            server.gone(job["pid"])
            os.kill(t.pid, 0)
            t.send("Continue after Escape.\r")
            t.shown("flow done")
            server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 2)
            entries = list(f.record(session))
            assert [e["text"] for e in entries if e["kind"] == "input" and
                    e["from"] == "person"] == ["Run the foreground job.", "Continue after Escape."]
            # The stopped turn keeps its native conversation: the next inputs continue it.
            assert not any(e["kind"] == "rebuild" for e in entries), entries
            assert [e["call"] for e in entries if e["kind"] == "result"] == ["ui_stop"], entries
            if model == "opus":
                ids = {e["native"]["claude"]["conversation"] for e in entries if e.get("native")}
                assert len(ids) == 1, ids
            else:
                last = sorted(f.requests.glob("[0-9]*.body"))[-1].read_text()
                assert "ui_stop" in last and "Run the foreground job." in last, last

# A handover input sent while a cell runs is not taken by the stopped turn; the path runs it, and
# on Claude its cells run too. The first cell starts no job, so no end notice opens a turn.
# Escape stops a cell: its timer's call is never made, even while the path, its processes held on
# Claude, has not ended the turn.
patch = json.dumps("*** Begin Patch\n*** Add File: first_ready\n+ready\n*** End Patch")
first = f'// @exec: {{"yield_time_ms": 60000}}\nawait tools.apply_patch({patch}); await new Promise(r => setTimeout(r, 600000));'
second = ('// @exec: {"yield_time_ms": 60000}\n'
          'text(await tools.exec_command({cmd: "printf ready > ready; sleep 600", yield_time_ms: 60000}));')
timer = second.replace("text(", 'setTimeout(() => tools.exec_command({cmd: "true"}), 3000); text(')
for model in ("opus", "gpt-6.1-sol"):
    if model == "opus":
        run = lambda id, code: reply((id, "mcp__razor__exec", {"code": code}))
    else:
        run = lambda id, code: response([{"type": "custom_tool_call", "id": f"ct_{id}", "call_id": id,
                                          "name": "exec", "input": code}])
    replies = [run("queued_first", first), run("queued_job", second)]
    replies.append(reply(text="flow done") if model == "opus" else done())
    with flow(replies, model=model, config="[profiles.claude]\ncode_mode = true\n") as f:
        server.start(f)
        session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        server.post(f, f"/sessions/{session}/input", {"text": "Hold this cell open."})
        server.until(f, session, lambda es: (f.cwd / "first_ready").exists())
        server.post(f, f"/sessions/{session}/handover", {})
        server.post(f, f"/sessions/{session}/stop", {})
        entries = server.until(f, session, lambda es: (f.cwd / "ready").exists()
                               and any(e["kind"] == "job" for e in es))
        agents = server.agents(f)
        assert agents[session]["state"] == "working", agents
        server.post(f, f"/sessions/{session}/stop", {})
        job = next(e for e in entries if e["kind"] == "job")
        server.gone(job["pid"])
        later = server.post(f, f"/sessions/{session}/input", {"text": "After both stops."})["entry"]
        entries = server.until(f, session, lambda es: any(e["kind"] == "turn_end" and later in e["inputs"]
                                                          for e in es))
        ends = [e["end"] for e in entries if e["kind"] == "turn_end"]
        assert ends == ["stopped", "stopped", "done"], ends
    with flow([run("timer", timer)], model=model, config="[profiles.claude]\ncode_mode = true\n") as f:
        pid = server.start(f)
        session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        server.post(f, f"/sessions/{session}/input", {"text": "Run the cell."})
        entries = server.until(f, session, lambda es: any(e["kind"] == "job" for e in es))
        path(pid, signal.SIGSTOP)
        server.post(f, f"/sessions/{session}/stop", {})
        server.gone(next(e for e in entries if e["kind"] == "job")["pid"])
        time.sleep(3.5)
        path(pid, signal.SIGCONT)
        entries = server.until(f, session, lambda es: any(e["kind"] == "turn_end" for e in es))
        assert [e["call"] for e in entries if e["kind"] == "call"] == ["timer", "timer:1"], entries

# News a yielded cell's calls take is answered by the next Result that showed the cell, which the
# model reads: never by the call's own Result, which only the cell's code reads, nor by a refused
# wait. A stop before such a Result (on Claude, the CLI's own result for the stopped wait) puts the
# news back.
cell = ('// @exec: {"yield_time_ms": 300}\nconst wait = () => tools.agent({action: "wait", timeout_ms: 30000});\n'
        'text(await wait()); yield_control(); const later = await wait(); await wait(); text(later);')
gate = '// @exec: {"yield_time_ms": 60000}\nawait tools.exec_command({cmd: "while [ ! -e go ]; do sleep 0.05; done"});'
read = {"cell_id": "cell", "yield_time_ms": 30000}
waits = [("bad", {"cell_id": "cell", "terminate": "no"}), ("stale", {"cell_id": "read"}), ("again", read)]
for model in ("opus", "gpt-6.1-sol"):
    if model == "opus":
        run = lambda id, code: reply((id, "mcp__razor__exec", {"code": code}))
        wait = lambda id, input: reply((id, "mcp__razor__wait", input))
    else:
        run = lambda id, code: response([{"type": "custom_tool_call", "id": f"ct_{id}", "call_id": id,
                                          "name": "exec", "input": code}])
        wait = lambda id, input: response([call("wait", json.dumps(input), id)])
    replies = [run("cell", cell), wait("read", read), run("gate", gate), *(wait(*w) for w in waits),
               reply(text="flow done") if model == "opus" else done()]
    config = "[profiles.claude]\ncode_mode = true\n" if model == "opus" else ""
    with flow(replies, model=model, config=config) as f:
        server.start(f)
        session, sender = (server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"] for _ in range(2))
        server.post(f, f"/sessions/{session}/input", {"text": "Read news through a cell."})
        news = {}
        # The gate holds the refused waits back until the cell's second wait has taken NEWS_B.
        for calls, then in ((["read"], "NEWS_A"), (["cell:2", "gate:1"], "NEWS_B"), (["cell:3"], "go"),
                            (["again"], None)):
            server.until(f, session, lambda es: all(any(e.get("call") == c for e in es) for c in calls))
            if then == "go":
                (f.cwd / "go").touch()
            elif then:
                news[then] = server.post(f, "/mail", {"to": session, "id": [sender, len(news) + 1], "kind": "note",
                                                      "from": "flow", "project": "cwd", "text": then})["entry"]
        server.post(f, f"/sessions/{session}/stop", {})
        entries = server.until(f, session, lambda es: any(e["kind"] == "turn_end" and news["NEWS_B"] in e["inputs"]
                                                          for e in es))
        named = {e["call"]: e.get("inputs", []) for e in entries if e["kind"] == "result"}
        assert named["read"] == [news["NEWS_A"]] and named["bad"] == named["stale"] == named["again"] == [], named
        assert not any(inputs for call, inputs in named.items() if call.startswith("cell:")), named
        ends = [e for e in entries if e["kind"] == "turn_end"]
        assert ends[0]["end"] == "stopped" and news["NEWS_B"] not in ends[0]["inputs"], ends
        last = sorted(f.requests.glob("[0-9]*.body"))[-1].read_text()
        assert last.count("NEWS_A") == 1 and last.count("NEWS_B") == 1, "news reached the model twice or not at all"

# A second Escape before the path ends the turn drops the conversation: an input sent to it and not
# yet read is not answered by the stopped turn, but opens the next one. Stopping the model path's
# processes holds the first Escape back, so the path never starts on that input before the second.
gate = lambda id, file: (id, "Bash", {"command": f"until [ -e {file} ]; do sleep 0.05; done"})
with flow([reply(gate("unread_a", "a_go"), gate("unread_b", "b_go")), reply(text="flow done")]) as f:
    pid = server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    server.post(f, f"/sessions/{session}/input", {"text": "Run both gates."})
    server.until(f, session, lambda es: any(e["kind"] == "job" for e in es))
    later = server.post(f, f"/sessions/{session}/input", {"text": "UNREAD_WORDS"})["entry"]
    (f.cwd / "a_go").touch()
    server.until(f, session, lambda es: any(e["kind"] == "result" and e["call"] == "unread_a" for e in es))
    path(pid, signal.SIGSTOP)
    for _ in range(2):
        server.post(f, f"/sessions/{session}/stop", {})
    path(pid, signal.SIGCONT)
    entries = server.until(f, session, lambda es: any(e["kind"] == "turn_end" and later in e["inputs"] for e in es))
    assert [e["end"] for e in entries if e["kind"] == "turn_end"] == ["stopped", "done"], entries[-3:]
    assert "UNREAD_WORDS" in sorted(f.requests.glob("[0-9]*.body"))[-1].read_text()

# An Escape that reaches the model path after the turn's end it has not yet recorded stops
# nothing: the next turn's cells run. The race is narrow, so each round tries it once: before the
# fix, twelve parallel copies of this case failed 7 runs in 36, so a red here is rare, and real.
replies = [r for i in range(20) for r in (reply(text=f"end {i}"), reply((f"next_{i}", "mcp__razor__exec", {
    "code": f'text(await tools.exec_command({{cmd: "touch ok-{i}"}}));'})), reply(text="done"))]
with flow(replies, config="[profiles.claude]\ncode_mode = true\n") as f:
    server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    for i in range(20):
        stream = server.send(f, "GET", f"/sessions/{session}/events?after=0")
        server.post(f, f"/sessions/{session}/input", {"text": f"First {i}"})
        next(line for line in stream if json.loads(line).get("text") == f"end {i}")
        server.post(f, f"/sessions/{session}/stop", {})
        stream.close()
        server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 2 * i + 1)
        later = server.post(f, f"/sessions/{session}/input", {"text": f"Run the next cell {i}."})["entry"]
        server.until(f, session, lambda es: any(e["kind"] == "turn_end" and later in e["inputs"] for e in es))
        assert (f.cwd / f"ok-{i}").exists(), f"round {i}: the cell after a late Escape was refused"

# A second Escape in the first request after a rebuild answers the rebuild, which no input is: a
# restart then asks nothing. Stopping the model path's processes holds the first Escape back.
slow = reply(text="Rebuilt reply.")
with flow([reply(("h", "mcp__razor__handover", {"note": "Continue."})), Reply(slow.body, cuts=(1,) * 50)]) as f:
    pid = server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    server.post(f, f"/sessions/{session}/input", {"text": "Keep this task."})
    server.until(f, session, lambda es: (f.requests / "002.body").exists())
    path(pid, signal.SIGSTOP)
    for _ in range(2):
        server.post(f, f"/sessions/{session}/stop", {})
    entries = server.until(f, session, lambda es: es[-1].get("end") == "stopped")
    assert next(e["id"] for e in entries if e["kind"] == "rebuild") in entries[-1]["inputs"], entries[-3:]
    # The reply is all written before the path resumes, so dropping the request cuts nothing.
    server.until(f, session, lambda _: "sent" in json.loads((f.requests / "002.json").read_text()))
    path(pid, signal.SIGCONT)
    server.stop(f, signal.SIGKILL)
    server.start(f)
    time.sleep(1)
    assert len(list(f.requests.glob("[0-9]*.body"))) == 2, "the restart resumed the stopped work"
