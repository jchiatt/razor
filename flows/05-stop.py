"""Escape through the API stops a foreground job on both model paths; disconnecting a client
leaves its session running and a second client replays from its cursor; a stopped job that
outlives the stop still tells the model of the stop. Ctrl-C quits the terminal app, not a
session; that check is left to flow 36 through the real terminal app."""
import json
import os
import signal
import subprocess
import time

from claude import reply
from driver import flow
from responses import PLAIN, call, done, response
import server



for model in ("opus", "gpt-6.1-sol"):
    command = "sleep 600 & printf ready > ready; while [ ! -e detached ]; do sleep 0.05; done; printf detached; wait"
    following = "printf working > next.txt"
    if model == "opus":
        replies = [reply(("call_stop", "Bash", {"command": command})),
                   reply(("call_next", "Bash", {"command": following})),
                   reply(text="flow done")]
    else:
        replies = [response([call("bash", json.dumps({"command": command}), "call_stop")]),
                   response([call("bash", json.dumps({"command": following}), "call_next")]),
                   done()]
    with flow(replies, model=model, config=PLAIN if model != "opus" else "") as f:
        pid = server.start(f)
        session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        path = f"/sessions/{session}"
        state = f.home / ".razor"
        stream = server.send(f, "GET", f"{path}/events?after=0")
        assert stream.status == 200, stream.status
        assert json.loads(stream.readline())["kind"] == "attached"
        ask = server.post(f, path + "/input", {"text": "Run the foreground job."})["entry"]
        seen = []
        while not seen or seen[-1]["kind"] != "call":
            line = json.loads(stream.readline())
            if "id" in line:
                seen.append(line)
        cursor = seen[-1]["id"]
        # The client read the Call, but not its job's Started entry: reattach must replay it.
        stream.close()
        started = server.until(f, session, lambda es: (f.cwd / "ready").exists()
                               and any(e["kind"] == "job" for e in es))
        job = next(e for e in started if e["kind"] == "job")
        assert job["event"] == "started" and job["call"] == cursor > ask, job
        os.kill(pid, 0)
        processes = subprocess.check_output(["ps", "-eo", "pid=,pgid=,stat="], text=True)
        members = [row.split() for row in processes.splitlines() if int(row.split()[1]) == job["pid"]]
        assert len([row for row in members if not row[2].startswith("Z")]) >= 2, members
        (f.cwd / "detached").touch()
        output = state / "sessions" / session / f"job-{job['job']}"
        before = server.until(f, session, lambda _: output.read_bytes() == b"detached")
        assert not [e for e in before if e["kind"] in ("result", "turn_end")], before
        # A different connection attaches after the exact durable cursor the first client read.
        stream = server.send(f, "GET", f"{path}/events?after={cursor}")
        assert stream.status == 200, stream.status
        attached = json.loads(stream.readline())
        assert attached == {"kind": "attached", "session": session, "last": before[-1]["id"], "pending": []}, attached
        replay = []
        while not replay or replay[-1]["id"] != before[-1]["id"]:
            line = json.loads(stream.readline())
            if "id" in line:
                replay.append(line)
        assert replay == [{k: v for k, v in e.items() if k != "native"} for e in before if e["id"] > cursor] and job in replay, replay
        server.post(f, path + "/stop", {})
        entries = server.until(f, session, lambda es: any(
            e["kind"] == "turn_end" and ask in e["inputs"] for e in es))
        stopped = next(e for e in entries if e["kind"] == "turn_end" and ask in e["inputs"])
        assert stopped["end"] == "stopped", stopped
        # One result for the stopped call, an unknown outcome: on Claude the CLI's own interrupt
        # result carries no answer of Razor's, so it is not mirrored.
        results = [e for e in entries if e["kind"] == "result" and e["call"] == "call_stop"]
        assert len(results) == 1 and results[0]["outcome"] == "unknown", results
        server.gone(job["pid"])
        # The killed job's end notice starts no turn of its own: after Escape the agent stays quiet.
        entries = server.until(f, session, lambda es: any(e["kind"] == "job" and e["event"] == "ended" for e in es))
        time.sleep(1)
        assert len(list(f.requests.glob("[0-9]*.body"))) == 1, "the stopped job's notice started a turn"
        ends = [e for e in entries if e["kind"] == "job" and e["event"] == "ended"]
        assert len(ends) == 1 and ends[0]["job"] == job["job"] and "signal" in ends[0]["end"], ends
        later = server.post(f, path + "/input", {"text": "Run the next command."})["entry"]
        entries = server.until(f, session, lambda es: any(
            e["kind"] == "turn_end" and later in e["inputs"] for e in es))
        # The next message's request tells the model it was the person's stop and no refusal, after
        # how long, and the job's last output; on GPT the stopped call's own output says so too.
        told = (f.requests / "002.body").read_text().split("Nothing refused this command: the person stopped the turn while it was running, ")
        assert len(told) == 2 and told[1].split(" s after")[0].isdigit() and "output:\\ndetached" in told[1] and (model == "opus" or "before this call had its result" in told[0]), told[-1][-900:]
        assert entries[-1]["end"] == "done" and (f.cwd / "next.txt").read_text() == "working", entries[-3:]
        calls = [e["call"] for e in entries if e["kind"] == "call"]
        assert calls == ["call_stop", "call_next"], calls
        while replay[-1]["id"] != entries[-1]["id"]:
            line = json.loads(stream.readline())
            if "id" in line:
                replay.append(line)
        assert replay == [{k: v for k, v in e.items() if k != "native"} for e in entries if e["id"] > cursor], replay
        stream.close()

# A job that ignores TERM outlives the stop until KILL. Through a restart, its end opens no turn
# and the next input carries the stop; ending during the next turn, it joins that turn at a call.
stubborn = "trap '' TERM; printf ready > ready; until [ -e release ]; do sleep 0.05; done"
for restart in (True, False):
    later = [] if restart else [response([call("bash", json.dumps({"command": "sleep 2"}), "later")])]
    replies = [response([call("bash", json.dumps({"command": stubborn}), "stubborn")]), *later, done()]
    with flow(replies, model="gpt-6.1-sol", config=PLAIN) as f:
        pid = server.start(f)
        session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        server.post(f, f"/sessions/{session}/input", {"text": "Start it."})
        server.until(f, session, lambda es: (f.cwd / "ready").exists())
        server.post(f, f"/sessions/{session}/stop", {})
        server.until(f, session, lambda es: any(e.get("end") == "stopped" for e in es))
        if restart:  # before the stop's KILL, so the job is taken back
            os.kill(pid, signal.SIGKILL)
            server.gone(pid)
            server.start(f)
            (f.cwd / "release").touch()
            server.until(f, session, lambda es: any(e.get("event") == "ended" for e in es))
            time.sleep(1)
            assert len(list(f.requests.glob("[0-9]*.body"))) == 1, "after a restart the stopped job's end started a turn"
        server.post(f, f"/sessions/{session}/input", {"text": "Go on."})
        entries = server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 2)
        told = [e["text"] for e in entries if e["kind"] == "input" and e["from"] == "razor"]
        assert len(told) == 1 and "the person stopped the turn while it was running" in told[0], (restart, told)
