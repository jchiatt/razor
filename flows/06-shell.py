"""Shell results, and a person's correction before the next action on both model paths."""
import json
from pathlib import Path
import re
from claude import reply
from driver import REPLIES, Reply, flow
from responses import PLAIN, call, done, response
import server

replies = [Reply((REPLIES / f"{name}.sse").read_bytes()) for name in ("shell-short", "shell-large")]
# Over the old 2,000-character preview and under SPILL: inline, as Claude Code's own Bash does.
medium = "head -c 2560 /dev/zero | tr '\\0' m"
replies += [reply(("toolu_medium", "Bash", {"command": medium})), Reply((REPLIES / "done.sse").read_bytes())]
with flow(replies) as f:
    result = f.run("--print=json", "Run the two commands, then finish.")
    assert result.returncode == 0, result.stderr
    answer = json.loads(result.stdout)
    assert answer["message"] == "flow done", answer
    entries = list(f.record(answer["session"]))
    calls = [e for e in entries if e["kind"] == "call"]
    outputs = [e for e in entries if e["kind"] == "result"]
    assert [e["call"] for e in calls] == ["toolu_short", "toolu_large", "toolu_medium"], calls
    assert [e["call"] for e in outputs] == [e["call"] for e in calls], outputs
    # The SDK mirrors the trained status-first shape, trimming the trailing newline.
    short, large, medium = outputs
    assert medium["output"] == "Exit code 0\n" + "m" * 2560, medium
    assert short["output"] == "Exit code 7\nshell-output", short
    assert "Exit code 7" in short["output"] and short["outcome"] == "error", short
    # A spill is Claude Code's <persisted-output>: the job's output file and the end of it.
    assert large["outcome"] == "success" and "<persisted-output>" in large["output"], large
    paths = re.findall(r"/\S+/job-2\b", large["output"])
    assert paths and Path(paths[0]).read_bytes() == b"x" * 40000, large
    assert 0 < len(large["output"]) < 5000 and "x" * 100 in large["output"], large
    ends = [e["end"] for e in entries if e["kind"] == "job" and e["event"] == "ended"]
    assert ends == [{"exit": 7}, {"exit": 0}, {"exit": 0}], ends


for model in ("opus", "gpt-6.1-sol"):
    correction = "copper-heron-06: write steered before the next action."
    # Hold the sleeping call until the input is admitted, not for a timing-dependent interval.
    sleeping = "printf ready > ready; while [ ! -e continue ]; do sleep 0.05; done; printf slept"
    commands = [("call_sleep", sleeping), ("call_batch", "printf batch > batch")]
    following = "printf steered > steered"
    if model == "opus":
        replies = [reply(*[(id, "Bash", {"command": command}) for id, command in commands]),
                   reply(("call_next", "Bash", {"command": following}), match=correction),
                   reply(text="flow done")]
    else:
        replies = [response([call("bash", json.dumps({"command": command}), id) for id, command in commands]),
                   Reply(response([call("bash", json.dumps({"command": following}), "call_next")]).body,
                         match=correction), done()]
    with flow(replies, model=model, config=PLAIN if model != "opus" else "") as f:
        server.start(f)
        session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        path = f"/sessions/{session}"
        first = server.post(f, path + "/input", {"text": "Run the sleeping command, then the next action."})["entry"]
        server.until(f, session, lambda es: (f.cwd / "ready").exists()
                     and len([e for e in es if e["kind"] == "call"]) == 2)
        steering = server.post(f, path + "/input", {"text": correction})["entry"]
        assert not any(e["kind"] == "result" for e in f.record(session)), "sleep ended before steering"
        stream = server.send(f, "GET", f"{path}/events?after={steering}")
        assert stream.status == 200 and json.loads(stream.readline())["kind"] == "attached"
        (f.cwd / "continue").touch()
        next_request = f.requests / "002.body"
        server.until(f, session, lambda es: any(
            e.get("call") == "call_next" or e.get("text") == "flow done" for e in es))
        request = json.loads(next_request.read_bytes())
        assert correction in json.dumps(request), f"{model}: first request after call_sleep omitted steering"
        if model == "opus":
            blocks = [b for m in request["messages"] if isinstance(m["content"], list) for b in m["content"]]
            results = [b["tool_use_id"] for b in blocks if b["type"] == "tool_result"]
            corrected = [i for i, b in enumerate(blocks) if correction in b.get("text", "")]
            outputs = [i for i, b in enumerate(blocks) if b["type"] == "tool_result"]
        else:
            blocks = request["input"]
            results = [b["call_id"] for b in blocks if b["type"] == "function_call_output"]
            corrected = [i for i, b in enumerate(blocks) if b["type"] == "message"
                         and b["role"] == "user" and correction in json.dumps(b["content"])]
            outputs = [i for i, b in enumerate(blocks) if b["type"] == "function_call_output"]
        assert results == [id for id, _ in commands] and len(corrected) == 1, blocks
        assert max(outputs) < corrected[0], "steering split calls from their outputs"
        live = []
        while not live or live[-1].get("call") != "call_next":
            live.append(json.loads(stream.readline()))
        taken = [i for i, e in enumerate(live) if e["kind"] == "taken" and e["input"] == steering]
        result = next(i for i, e in enumerate(live) if e["kind"] == "result" and e["call"] == "call_sleep")
        assert len(taken) == 1 and taken[0] < result, live
        stream.close()
        entries = server.until(f, session, lambda es: any(
            e["kind"] == "turn_end" and steering in e["inputs"] for e in es))
        ends = [e for e in entries if e["kind"] == "turn_end"]
        calls = [e for e in entries if e["kind"] == "call"]
        result = next(e for e in entries if e["kind"] == "result" and e["call"] == "call_sleep")
        assert len(ends) == 1 and ends[0]["end"] == "done" and ends[0]["inputs"] == [first, steering], ends
        assert [e["call"] for e in calls] == ["call_sleep", "call_batch", "call_next"], calls
        assert first < steering < result["id"] < calls[-1]["id"], entries
        assert (f.cwd / "batch").read_text() == "batch" and (f.cwd / "steered").read_text() == "steered"
        requests = [p.read_text() for p in sorted(f.requests.glob("[0-9]*.body"))]
        assert len(requests) == 3 and [r.count(correction) for r in requests] == [0, 1, 1], requests
    print(f"PASS 06-shell steering {model}: input taken once at the first result, before the next action", flush=True)
