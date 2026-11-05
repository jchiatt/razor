"""The two command guards refuse before execution; Razor's own keys (the web providers' and
the Claude sign-in's) never reach shell jobs."""
import json

from driver import flow
from responses import PLAIN, call, done, response

keys = {name: "flow-secret" for name in
        ("EXA_API_KEY", "FIRECRAWL_API_KEY", "PARALLEL_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN",
         "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN")}
commands = [f"printf ran > ran-{n}; ./{name} {arg}" for n, (name, arg) in enumerate(
            [("pkill", "no-process"), ("killall", "no-process"), ("kill", "-1")])]
commands += ["printf ran > delete-ran; rm old", "printf made > own; cat own", "rm own", "env"]
items = [call("bash", json.dumps({"command": command}), f"call_{n}")
         for n, command in enumerate(commands)]
with flow([response(items), done()], model="gpt-6.1-sol",
          config=PLAIN + "[guards]\ndelete = true\n",
          env={**keys, "FLOW_ORDINARY": "passed-through"}) as f:
    (f.cwd / "old").write_text("before session")
    # Harmless executables keep a broken kill guard safe, including `kill -1`.
    for name in ("pkill", "killall", "kill"):
        (f.cwd / name).write_text("#!/bin/sh\nexit 0\n")
        (f.cwd / name).chmod(0o755)
    result = f.run("--print=json", "Check guards and the job environment.")
    assert result.returncode == 0, result.stderr
    entries = list(f.record(json.loads(result.stdout)["session"]))
    outputs = [e for e in entries if e["kind"] == "result"]
    assert len(outputs) == len(items), outputs
    for n, entry in enumerate(outputs[:3]):
        assert entry["outcome"] == "error" and "kill" in entry["output"], entry
        assert not (f.cwd / f"ran-{n}").exists(), "a refused command ran"
    assert outputs[3]["outcome"] == "error" and "old" in outputs[3]["output"], outputs[3]
    assert (f.cwd / "old").read_text() == "before session", "old file was deleted"
    assert not (f.cwd / "delete-ran").exists(), "a refused deletion command ran"
    assert all(e["outcome"] == "success" for e in outputs[4:]), outputs[4:]
    assert json.loads(outputs[4]["output"])["output"] == "made", outputs[4]
    assert not (f.cwd / "own").exists(), "the session's own file was not deleted"
    jobs = [e for e in entries if e["kind"] == "job" and e["event"] == "started"]
    assert len(jobs) == 3, jobs
    environment = json.loads(outputs[-1]["output"])["output"]
    assert "FLOW_ORDINARY=passed-through" in environment, environment
    assert not any(name + "=" in environment for name in keys), environment

# Full-command patterns select every matching owned job, including a child process; an identical
# process outside the session survives and cannot be stopped after the owned jobs are gone. A bare
# stop runs the rest of its command, a wrapped one none of it; each job's end shows once, as a notice.
import os
import subprocess
commands = [("one", "sleep 600 & printf one-ready > one; wait"),
            ("two", "sleep 601 & printf two-ready > two; wait")]
for stop, bare in (("pkill -9 -f '^sleep 60[01]$'", True), ("env pkill -TERM -x sleep", False)):
    replies = [response([call("bash", json.dumps({"command": command, "wait_ms": 0}), id)
                         for id, command in commands]),
               response([call("bash", json.dumps({"command": "pkill -USR1 -f sleep"}), "signal_only"),
                         call("bash", json.dumps({"command": stop + "; printf restarted > after"}), "stop_owned"),
                         call("bash", json.dumps({"command": "pkill -f '^sleep 600$'"}), "stop_absent")]), done()]
    with flow(replies, model="gpt-6.1-sol", config=PLAIN) as f:
        outside = subprocess.Popen(["sleep", "600"])
        try:
            result = f.run("--print=json", "Stop my matching jobs.")
            assert result.returncode == 0, result.stderr
            entries = list(f.record(json.loads(result.stdout)["session"]))
            signal_only = next(e for e in entries if e["kind"] == "result" and e["call"] == "signal_only")
            assert signal_only["outcome"] == "error", signal_only
            stopped = next(e for e in entries if e["kind"] == "result" and e["call"] == "stop_owned")
            assert (stopped["outcome"] == "success") == bare == (f.cwd / "after").exists(), stopped
            assert '"job":3' in stopped["output"] if bare else "without `pkill`" in stopped["output"], stopped
            notices = [e["text"].split()[1] for e in entries if e["kind"] == "input" and e["from"] == "razor"]
            assert sorted(notices) == ["job-1", "job-2"], notices
            absent = next(e for e in entries if e["kind"] == "result" and e["call"] == "stop_absent")
            assert absent["outcome"] == "error" and "kill a job by its handle" in absent["output"], absent
            assert outside.poll() is None, "a pattern stop killed a process outside this session"
            starts = [e for e in entries if e["kind"] == "job" and e["event"] == "started"]
            assert len(starts) == 2 + bare, starts
            for job in starts[:2]:
                try:
                    os.kill(job["pid"], 0)
                except ProcessLookupError:
                    pass
                else:
                    raise AssertionError(f"pattern stop left job {job['job']} alive")
        finally:
            outside.terminate()
            outside.wait()
