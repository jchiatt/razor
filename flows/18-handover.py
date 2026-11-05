"""On-demand handover uses the same rebuild and preserves the complete old ask."""
import json
import os
from pathlib import Path
import signal
import time
from apns import apple
from claude import reply
from clock import wall_clock
from driver import REPLIES, Reply, flow
from responses import PLAIN, call, done, response
import server

ask = "Keep this ask verbatim: the silver kingfisher visits at 06:43."
first_note = "Preserve the user's exact ask; recall entry 2 next."
handover = reply(("toolu_changed", "Bash", {"command": "printf kingfisher > kingfisher.txt"}),
                 ("toolu_handover", "mcp__razor__handover", {"note": first_note}))
with flow([handover] + [Reply((REPLIES / f"{name}.sse").read_bytes()) for name in ("recall", "done")]) as f:
    f.base()
    result = f.run("--print=json", ask)
    assert result.returncode == 0, result.stderr
    answer = json.loads(result.stdout)
    assert answer["message"] == "flow done", answer
    entries = list(f.record(answer["session"]))
    assert entries[1]["kind"] == "input" and entries[1]["text"] == ask, entries[1]
    rebuilds = [e for e in entries if e["kind"] == "rebuild"]
    assert len(rebuilds) == 1 and rebuilds[0]["trigger"] == "handover", rebuilds
    assert ask in rebuilds[0]["context"], rebuilds[0]
    assert "?? kingfisher.txt" in rebuilds[0]["context"], rebuilds[0]["context"]
    assert rebuilds[0]["note"] == first_note, rebuilds[0]
    calls = [e for e in entries if e["kind"] == "call"]
    assert [e["call"] for e in calls] == ["toolu_changed", "toolu_handover", "toolu_recall"], calls
    outputs = [e for e in entries if e["kind"] == "result"]
    assert [e["call"] for e in outputs] == [e["call"] for e in calls], outputs
    assert calls[1]["id"] < rebuilds[0]["id"] < calls[2]["id"], entries
    assert json.loads(outputs[2]["output"]) == entries[1], outputs[2]
    conversations = {e["native"]["claude"]["conversation"] for e in entries if e.get("native")}
    assert len(conversations) == 2, conversations
    assert len([e for e in entries if e["kind"] == "start"]) == 1, entries
    fresh = json.loads((f.requests / "002.body").read_bytes())
    assert ask in json.dumps(fresh["messages"]), fresh
    assert rebuilds[0]["note"].strip() in json.dumps(fresh["messages"]), fresh

# While a turn waits on news, the person's message ends the wait at once with null and joins
# after that Result; so does the person's /handover, sent as the command it is, and a second is
# refused {}. On GPT, the threshold that the waiting response itself reaches ends that wait too.
wait = {"action": "wait", "timeout_ms": 30000}
wait_note = "Hand over from the wait."
replies = [reply(("toolu_wait", "mcp__razor__agent", wait)), reply(("toolu_again", "mcp__razor__agent", wait)),
           reply(("toolu_wait_note", "mcp__razor__handover", {"note": wait_note})), reply(text="Rebuilt.")]
with flow(replies) as f:
    server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    path = f"/sessions/{session}"
    server.post(f, path + "/input", {"text": "Wait for news."})
    server.until(f, session, lambda es: any(e.get("call") == "toolu_wait" for e in es))
    time.sleep(0.5)
    server.post(f, path + "/input", {"text": "Steer now."})
    server.until(f, session, lambda es: any(e.get("call") == "toolu_again" for e in es), timeout=10)
    time.sleep(0.5)
    asked = server.post(f, path + "/handover", {})["entry"]
    assert server.post(f, path + "/handover", {}) == {}, "a second handover"
    entries = server.until(f, session, lambda es: any(
        e["kind"] == "turn_end" and asked in e["inputs"] for e in es), timeout=10)
    results = [e["output"] for e in entries if e["kind"] == "result"]
    request = next(e for e in entries if e["id"] == asked)
    assert results[:2] == ["null", "null"] and request["command"] == "handover", (results, request)
    assert "Steer now." in (f.requests / "002.body").read_text()
    assert json.dumps(request["text"])[1:-1] in (f.requests / "003.body").read_text()
    rebuild = next(e for e in entries if e["kind"] == "rebuild")
    assert rebuild["trigger"] == "handover" and rebuild["note"].strip() == wait_note, rebuild
large = response([call("agent", json.dumps(wait), "call_wait")]).body
replies = [Reply(large.replace(b'"input_tokens": 100', b'"input_tokens": 200000')),
           response([call("handover", json.dumps({"note": wait_note}), "call_note")]), done()]
with flow(replies, model="gpt-6.1-sol", config=PLAIN + "[rebuild]\nthreshold = 0.5") as f:
    result = f.run("--print=json", ask, timeout=10)
    assert result.returncode == 0, result.stderr
    entries = list(f.record(json.loads(result.stdout)["session"]))
    waited = next(e for e in entries if e["kind"] == "result" and e["call"] == "call_wait")
    rebuilds = [e["trigger"] for e in entries if e["kind"] == "rebuild"]
    assert waited["output"] == "null" and rebuilds == ["threshold"], (waited, rebuilds)

# The person's API request opens an idle note turn on both model paths. Files hold the
# note batch and the rebuilt turn open, so duplicate requests cannot race their endings.
for model in ("opus", "gpt-6.1-sol"):
    def message(*calls, text="flow done"):
        if model == "opus":
            names = {"bash": "Bash", "handover": "mcp__razor__handover"}
            return reply(*[(id, names[name], args) for id, name, args in calls], text=text)
        item = {"type": "message", "id": "msg_note", "role": "assistant",
                "content": [{"type": "output_text", "text": text}]}
        return response([item, *[call(name, json.dumps(args), id) for id, name, args in calls]])

    note = "Keep working on the silver kingfisher ask."
    replies = [message(),
               message(("note_wait", "bash", {"command": "while [ ! -e note_go ]; do sleep 0.05; done"}),
                       ("note_handover", "handover", {"note": note})),
               message(("rebuilt_wait", "bash", {"command": "while [ ! -e rebuilt_go ]; do sleep 0.05; done"})),
               message(), message()]
    with flow(replies, model=model, config=PLAIN) as f:
        first = f.run("--print=json", ask)
        assert first.returncode == 0, first.stderr
        session = json.loads(first.stdout)["session"]
        path = f"/sessions/{session}"
        admitted = server.post(f, path + "/handover", {})["entry"]
        waiting = server.until(f, session, lambda es: any(
            e["kind"] == "job" and e["event"] == "started" for e in es))
        assert not any(e["kind"] == "rebuild" for e in waiting), waiting
        assert server.post(f, path + "/handover", {}) == {}, "duplicate note request"
        (f.cwd / "note_go").touch()
        server.until(f, session, lambda es: any(
            e["kind"] == "job" and e["event"] == "started" and e["job"] == 2 for e in es))
        (f.cwd / "rebuilt_go").touch()
        entries = server.until(f, session, lambda es: any(
            e["kind"] == "turn_end" and admitted in e["inputs"] for e in es))
        requests = [e for e in entries if e["kind"] == "input" and e["from"] == "razor"]
        assert len(requests) == 1 and requests[0]["id"] == admitted, requests
        rebuilds = [e for e in entries if e["kind"] == "rebuild"]
        assert len(rebuilds) == 1 and rebuilds[0]["trigger"] == "handover", rebuilds
        rebuild = rebuilds[0]
        assert rebuild["note"].strip() == note and ask in rebuild["context"], rebuild
        bodies = [p.read_text() for p in sorted(f.requests.glob("[0-9]*.body"))]
        assert json.dumps(requests[0]["text"])[1:-1] in bodies[1], bodies[1]
        next_turn = f.run("--resume", session, "--print=json", "Continue the kingfisher work.")
        assert next_turn.returncode == 0, next_turn.stderr
        assert json.loads(next_turn.stdout)["message"] == "flow done", next_turn.stdout
        last = sorted(f.requests.glob("[0-9]*.body"))[-1].read_text()
        assert note in last and "Continue the kingfisher work." in last, last
        assert len([e for e in f.record(session) if e["kind"] == "rebuild"]) == 1

# Recall never hands a model its own thinking back, and neither its answer nor a handover's ever
# names thinking: Claude refuses a request that asks for it as reasoning extraction. The note is
# the handover call's own field, so a model's thinking can never hide it.
thought = "The kingfisher note sits only in this reasoning."
note = "Recall the kingfisher ask next."
reasoning = {"type": "reasoning", "id": "rs_note", "summary": [{"type": "summary_text", "text": thought}]}
replies = [response([reasoning, call("recall", json.dumps({"id": 4}), "call_reasoning")]),
           response([call("handover", json.dumps({"note": note}), "call_noted")]), done()]
with flow(replies, model="gpt-6.1-sol", config=PLAIN) as f:
    result = f.run("--print=json", ask)
    assert result.returncode == 0, result.stderr
    entries = list(f.record(json.loads(result.stdout)["session"]))
    assert entries[3]["kind"] == "thinking" and entries[3]["text"] == thought, entries[:5]
    outputs = {e["call"]: e for e in entries if e["kind"] == "result"}
    assert outputs["call_reasoning"]["outcome"] == "error", outputs["call_reasoning"]
    for said in (outputs["call_reasoning"]["output"], outputs["call_noted"]["output"]):
        assert thought not in said and not any(w in said.lower() for w in ("thinking", "reasoning")), said
    rebuilds = [e for e in entries if e["kind"] == "rebuild"]
    assert len(rebuilds) == 1 and rebuilds[0]["note"].strip() == note, rebuilds


def sized(tokens, *calls, text="flow done"):
    events = reply(*calls, text=text).body.decode().splitlines()
    for i, line in enumerate(events):
        if line.startswith("data: "):
            event = json.loads(line[6:])
            if event["type"] == "message_start":
                event["message"]["usage"].update(input_tokens=13, cache_read_input_tokens=tokens - 13,
                                               cache_creation_input_tokens=0)
                events[i] = "data: " + json.dumps(event)
    return Reply(("\n".join(events) + "\n").encode())


def completed(es):
    return es[-1]["kind"] == "turn_end"


def unloaded(f, session):
    pid = json.loads((f.home / ".razor/server.json").read_text())["pid"]
    record = f.home / ".razor/sessions" / session / "record.jsonl"
    return all(fd.resolve() != record for fd in Path(f"/proc/{pid}/fd").iterdir())


with wall_clock() as (clock, env, advance):
    env |= {"CLAUDE_CODE_PROMPT_CACHE_TTL": "5m", "CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL": "5m"}
    note = "Recall the silver kingfisher ask after idleness."
    child = "You are idle-helper18,"
    replies = [sized(100, ("idle_child", "mcp__razor__agent", {"action": "start", "name": "idle-helper18", "brief": "Wait.", "placement": "in_place"})),
               reply(("child_wait", "Bash", {"command": "until [ -e child_go ]; do sleep 0.05; done"}), match=child),
               reply(("child_finish", "mcp__razor__agent", {"action": "finish", "status": "finished", "report": "Child done."}), match=child),
               sized(100), reply(("idle_wait", "Bash", {"command": "until [ -e idle_go ]; do sleep 0.05; done"}),
                     ("idle_note", "mcp__razor__handover", {"note": note})),
               reply(text="flow done"), reply(text="Child heard."), reply(text="Continued.")]
    with flow(replies, config="[sessions]\nidle = 0\nhandover_tokens = 0\nclose_minutes = 60", env=env) as f:
        first = f.run("--print=json", ask)
        assert first.returncode == 0, first.stderr
        session = json.loads(first.stdout)["session"]
        child_id = next(e["session"] for e in f.record(session) if e["kind"] == "child")
        server.until(f, child_id, lambda es: any(e["kind"] == "job" for e in es))
        advance(2_999_999)
        time.sleep(1.2)
        assert not any(e["kind"] == "input" and e["from"] == "razor" for e in f.record(session)), "handover before 50 minutes"
        advance(3_000_000)
        entries = server.until(f, session, lambda es: any(
            e["kind"] == "job" and e["event"] == "started" for e in es), timeout=8)
        requests = [e for e in entries if e["kind"] == "input" and e["from"] == "razor"]
        assert len(requests) == 1, requests  # The child's running job did not block its parent.
        (f.cwd / "child_go").touch()
        reported = server.until(f, session, lambda es: any("report" in e.get("from", "") for e in es))
        report_id = next(e["id"] for e in reported if "report" in e.get("from", ""))
        late = "Keep the late kingfisher instruction verbatim."
        admitted = server.post(f, f"/sessions/{session}/input", {"text": late})["entry"]
        (f.cwd / "idle_go").touch()
        entries = server.until(f, session, lambda es: any(
            e["kind"] == "turn_end" and admitted in e["inputs"] for e in es))
        entries = server.until(f, session, lambda es: any(e["kind"] == "turn_end" and report_id in e["inputs"] for e in es))
        rebuilds = [e for e in entries if e["kind"] == "rebuild"]
        assert len(rebuilds) == 1 and rebuilds[0]["trigger"] == "idle", rebuilds
        assert rebuilds[0]["note"].strip() == note and ask in rebuilds[0]["context"], rebuilds
        assert late in rebuilds[0]["context"]
        for path in sorted(f.requests.glob("[0-9]*.body")):
            request = json.loads(path.read_text())
            blocks = request.get("system", []) + request.get("tools", [])
            blocks += [b for m in request["messages"] for b in m["content"] if isinstance(b, dict)]
            writes = [b["cache_control"] for b in blocks if "cache_control" in b]
            assert writes and all(w.get("ttl") == "1h" for w in writes), writes
        activity = server.agents(f)[session]["last_activity"]
        advance(9_000_000)
        server.until(f, session, lambda es: any(e.get("close_reason") == "idle" for e in es))
        server.until(f, session, lambda es: any("settled" in e.get("event", {}) for e in es if e["kind"] == "child"))
        assert server.agents(f)[session]["last_activity"] == activity, "idle close's child settlement moved activity"
        resumed = f.run("--resume", session, "--print=json", "Continue the Claude conversation.")
        assert resumed.returncode == 0 and note in sorted(f.requests.glob("[0-9]*.body"))[-1].read_text(), resumed.stderr

    # Running jobs hold off maintenance; answering their notice starts a new idle interval.
    advance(0)
    replies = [sized(50_000, ("idle_background", "Bash", {
                   "command": "until [ -e background_go ]; do sleep 0.05; done", "run_in_background": True})),
               sized(50_000), sized(50_000), reply(("background_note", "mcp__razor__handover", {"note": note})),
               sized(50_000)]
    with flow(replies, config="[sessions]\nidle = 0\nclose_minutes = 60", env=env) as f:
        first = f.run("--print=json", ask)
        assert first.returncode == 0, first.stderr
        session = json.loads(first.stdout)["session"]
        advance(3_600_000)
        time.sleep(1.2)
        assert not any(e.get("closed") or e["kind"] == "rebuild" for e in f.record(session)), "working job handed over or closed"
        (f.cwd / "background_go").touch()
        server.until(f, session, lambda es: completed(es) and any(
            e["kind"] == "job" and e["event"] == "ended" for e in es))
        os.kill(pid := server.start(f), signal.SIGTERM)  # Due maintenance survives a restart.
        server.gone(pid)
        server.start(f)
        advance(6_600_000)
        server.until(f, session, lambda es: completed(es) and any(e["kind"] == "rebuild" for e in es))
        settings = f.home / ".razor/config.toml"
        settings.write_text(settings.read_text().replace("close_minutes = 60", "close_minutes = 0"))
        server.until(f, session, lambda _: unloaded(f, session) and not (f.home / ".razor/live" / session).exists())
        assert not any(note in p.read_text() for p in f.requests.glob("[0-9]*.body")), "the idle rebuild sent a request"
        resumed = f.run("--resume", session, "--print=json", "Back on Claude.")  # The note opens its first request.
        assert resumed.returncode == 0 and all(t in sorted(f.requests.glob("[0-9]*.body"))[-1].read_text() for t in (note, "Back on Claude.")), resumed.stderr

    # A restart inside the idle note turn ends it at its rebuild, with no request nobody asked for.
    advance(0)
    replies = [sized(50_000), reply(("cut_note", "Bash", {"command": "until [ -e cut_go ]; do sleep 0.05; done"})),
               reply(text="Job ended.")]
    with flow(replies, config="[sessions]\nidle = 0\nclose_minutes = 60", env=env) as f:
        session = json.loads(f.run("--print=json", ask).stdout)["session"]
        advance(3_000_000)
        server.until(f, session, lambda es: any(e["kind"] == "job" for e in es))
        sent = len(list(f.requests.glob("[0-9]*.body")))
        server.stop(f, signal.SIGKILL)
        server.start(f)
        entries = server.until(f, session, completed)
        assert [e["trigger"] for e in entries if e["kind"] == "rebuild"] == ["idle"], entries[-3:]
        assert len(list(f.requests.glob("[0-9]*.body"))) == sent, "the restart asked the model again"
        (f.cwd / "cut_go").touch()  # The job's end notice opens the next turn.
        server.until(f, session, lambda es: completed(es) and es[-2].get("text") == "Job ended.")

    # Each route's smaller request clears eligibility against the new 50,000-token floor.
    for model in ("opus", "gpt-6.1-sol"):
        advance(0)
        replies = [sized(n) if model == "opus" else Reply(done().body.replace(
            b'"input_tokens": 100', f'"input_tokens": {n}'.encode())) for n in (50_000, 49_999)]
        with flow(replies, model=model, config=PLAIN + "[sessions]\nidle = 0", env=env) as f:
            first = f.run("--print=json", ask)
            assert first.returncode == 0, first.stderr
            session = json.loads(first.stdout)["session"]
            again = f.run("--resume", session, "--print=json", "One smaller request.")
            assert again.returncode == 0, again.stderr
            if model == "opus":
                server.until(f, session, lambda _: unloaded(f, session))
            advance(3_000_001)
            time.sleep(1.2)
            assert not any(e["kind"] == "input" and e["from"] == "razor" for e in f.record(session))

    # A running tool turn holds off idle close.
    advance(0)
    with flow([sized(150_000, ("running_idle", "Bash", {
            "command": "until [ -e running_go ]; do sleep 0.05; done"})), reply(text="flow done")],
              config="[sessions]\nhandover_minutes = 0\nclose_minutes = 50", env=env) as f:
        server.start(f)
        session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        server.post(f, f"/sessions/{session}/input", {"text": ask})
        server.until(f, session, lambda es: any(e["kind"] == "job" for e in es))
        advance(3_000_001)
        time.sleep(1.2)
        assert not any(e.get("closed") for e in f.record(session))
        (f.cwd / "running_go").touch()
        server.until(f, session, completed)

    # An open question holds off idle handover; an idle request whose turn fails is not repeated.
    advance(0)
    model = "gpt-6.1-sol"
    big = lambda r: Reply(r.body.replace(b'"input_tokens": 100', b'"input_tokens": 50000'))
    refusal = Reply(b'{"error": {"type": "invalid_request_error", "message": "Refused."}}', status=400)
    with flow([big(message(("idle_ask", "message", {"to": "parent", "type": "question", "text": "Which path?"}))),
               big(message()), big(message()), refusal], model=model, config=PLAIN + "[sessions]\nidle = 0", env=env) as f:
        session = json.loads(f.run("--print=json", ask).stdout)["session"]
        asked = next(e["id"] for e in f.record(session) if e["kind"] == "call")
        advance(3_000_001)
        time.sleep(1.2)
        assert not any(e.get("trigger") == "idle" for e in f.record(session)), "handover with a question open"
        answer = server.post(f, f"/sessions/{session}/asks/{asked}", {"text": "Left."})["entry"]
        server.until(f, session, lambda es: any(e["kind"] == "turn_end" and answer in e["inputs"] for e in es))
        advance(6_000_002)
        server.until(f, session, lambda es: completed(es) and any(e.get("trigger") == "idle" for e in es))
        time.sleep(1.2)
        assert sum(e["kind"] == "input" and e.get("trigger") == "idle" for e in f.record(session)) == 1, "the idle request repeated"

    # An idle note turn the host refuses as too long ends at its overflow rebuild, with no request.
    overflow = Reply(json.dumps({"error": {"type": "invalid_request_error", "code": "context_length_exceeded",
                     "message": "prompt is too long: 201000 tokens > 200000 maximum"}}).encode(), status=400)
    for model in ("opus", "gpt-6.1-sol"):
        advance(0)
        with flow([sized(50_000) if model == "opus" else big(message()), overflow], model=model,
                  config=PLAIN + "[sessions]\nidle = 0", env=env) as f:
            session = json.loads(f.run("--print=json", ask).stdout)["session"]
            advance(3_000_000)
            entries = server.until(f, session, lambda es: completed(es) and any(e["kind"] == "rebuild" for e in es))
            triggers = [e["trigger"] for e in entries if e["kind"] == "rebuild"]
            assert triggers == ["overflow"] and len(list(f.requests.glob("[0-9]*.body"))) == 2, entries[-3:]

    # GPT's default handover, resident report, stable phone history and notification silence.
    advance(0)
    replies = [message(text="Home ready."), big(message(text="Initial reply.")),
               message(("configured_note", "handover", {"note": note})),
               message(text="Report received."), message(text="Continued.")]
    config = PLAIN + "[sessions]\nidle = 0\nclose_minutes = 60\n"
    with apple(clock / model) as apns, flow(replies, model=model, env=env, config=config + f"""[push]
team = "FLOWTEAM48"
bundle = "com.example.razor"
endpoint = "{apns.url}"
[push.development]
key_id = "FLOWKEY48"
key = {json.dumps(str(apns.key))}
""") as f:
        with (f.home / ".razor/config.toml").open("a") as config:
            config.write(f"[residents.ember18]\ncwd = {json.dumps(str(f.cwd))}\n")
        server.start(f)
        server.post(f, "/sessions/ember18/input", {"text": "Home check."})
        server.until(f, "ember18", completed)
        session = server.post(f, "/sessions", {"resident": "ember18"})["session"]
        path = f"/sessions/{session}"
        server.post(f, path + "/input", {"text": ask})
        server.until(f, session, completed)
        server.post(f, "/sessions", {"resident": "ember18"})
        history = lambda: json.loads(server.send(f, "GET", "/sessions?resident=ember18").read())
        activity = lambda: (history(), server.agents(f)[session]["last_activity"])
        original_activity = activity()
        advance(3_000_000)
        entries = server.until(f, session, lambda es: completed(es) and any(e["kind"] == "rebuild" for e in es))
        maintenance = [e for e in entries if e["kind"] == "input" and e["from"] == "razor"]
        assert len(maintenance) == 1 and maintenance[0]["trigger"] == "idle", maintenance
        assert not any(note in p.read_text() for p in f.requests.glob("[0-9]*.body")), "the idle rebuild sent a request"
        assert activity() == original_activity, "handover moved activity"
        inputs = [e for e in entries if e["kind"] == "input"]
        phone = f.run("devices", "add", "phone").stdout.splitlines()[1]
        # At `all` every finish is sent, so no finished alert below means the idle close sent none.
        for route, body in (("/devices/self/push", {"token": "aa18", "environment": "development"}),
                            (path + "/follow", {"token": "ff18", "environment": "development"}),
                            ("/devices/self/notifications", {"level": "all"})):
            registered = server.send(f, "PUT", route, body, key=phone)
            assert registered.status == 200, registered.read()
            registered.read()
        with server.send(f, "GET", path + "/events") as stream:
            stream.readline()
            server.until(f, session, lambda _: bool(apns.received))
        requests = lambda: sum(ask in p.read_text() for p in f.requests.glob("[0-9]*.body"))
        before = requests()
        advance(3_600_000)
        entries = server.until(f, session, lambda es: es[-1].get("close_reason") == "idle")
        assert entries[-1]["closed"] and entries[-1]["end"] == "done" and entries[-1]["inputs"] == [], entries[-1]
        assert [e for e in entries if e["kind"] == "input"] == inputs and requests() == before, "close opened a model turn"
        assert activity() == original_activity and server.agents(f)[session]["close_reason"] == "idle"
        assert server.listed(f, session), "idle close moved the recently used thread out of the list"
        report = server.until(f, "ember18", lambda es: completed(es) and any(e.get("text") == note for e in es))
        assert not any(e.get("closed") for e in report), "resident home closed"
        time.sleep(1.2)
        assert not any(r["body"].get("razor", {}).get("kind") == "finished"
                       or r["body"]["aps"].get("event") == "end" for r in apns.received), apns.received
        assert any(follow["session"] == session for device in json.loads((f.home / ".razor/devices.json").read_text())
                   for follow in device.get("follows", [])), "idle close ended the Live Activity"
        resumed = f.run("--resume", session, "--print=json", "Continue our conversation.")
        assert resumed.returncode == 0, resumed.stderr
        last = sorted(f.requests.glob("[0-9]*.body"))[-1].read_text()  # One request: the note and the person's input.
        assert json.loads(resumed.stdout)["session"] == session and note in last and "Continue our conversation." in last
        assert [e["trigger"] for e in f.record(session) if e["kind"] == "rebuild"] == ["idle"] and "close_reason" not in server.agents(f)[session]

# Both native routes preserve their structured overflow reason. The first failed request
# rebuilds with the last note; two rejections pause with the rebuilt byte size, without a loop.
for model in ("opus", "gpt-6.1-sol"):
    refusal = Reply(json.dumps({"error": {"type": "invalid_request_error",
        "code": "context_length_exceeded", "message": "prompt is too long: 201000 tokens > 200000 maximum"}}).encode(), status=400)
    for again in (False, True):
        success = reply(text="flow done") if model == "opus" else done()
        with flow([refusal, refusal if again else success], model=model, config=PLAIN) as f:
            result = f.run("--print=json", "Keep the unanswered overflow request.")
            answer = json.loads(result.stdout)
            assert (result.returncode != 0) == again, result
            entries = list(f.record(answer["session"]))
            rebuilds = [e for e in entries if e["kind"] == "rebuild"]
            assert len(rebuilds) == 1 and rebuilds[0]["trigger"] == "overflow", rebuilds
            assert "Keep the unanswered overflow request." in rebuilds[0]["context"], rebuilds
            ends = [e for e in entries if e["kind"] == "turn_end"]
            assert isinstance(ends[0]["end"], dict) and not ends[0]["inputs"], ends
            assert entries[1]["id"] in ends[-1]["inputs"], ends
            if again:
                assert str(len(rebuilds[0]["context"].encode())) + " bytes" in answer["error"], answer
            else:
                assert answer["message"] == "flow done", answer
            assert len(list(f.requests.glob("[0-9]*.body"))) == 2, "overflow retried in a loop"

# A cell's handover uses the same path on GPT and Claude, including its sentence result.
for model in ("opus", "sol"):
    code = f"text(await tools.handover({{note: {json.dumps(first_note)}}}));"
    cell = (reply(("cell", "mcp__razor__exec", {"code": code})) if model == "opus" else
            response([{"type": "custom_tool_call", "id": "ct_cell", "call_id": "cell", "name": "exec", "input": code}]))
    end = reply(text="Rebuilt.") if model == "opus" else done()
    config = '[profiles.claude]\ncode_mode = true\n' if model == "opus" else ""
    with flow([end, cell, end], model=model, config=config) as f:
        first = f.run("--print=json", ask)
        assert first.returncode == 0, first.stderr
        session = json.loads(first.stdout)["session"]
        asked = server.post(f, f"/sessions/{session}/handover", {})["entry"]
        results = server.until(f, session, lambda es: any(e["kind"] == "result" and e["call"] == "cell" for e in es))
        assert all(e["outcome"] == "success" for e in results if e["kind"] == "result"), results
        entries = server.until(f, session, lambda es: any(e["kind"] == "rebuild" for e in es)
                               and es[-1]["kind"] == "turn_end" and asked in es[-1]["inputs"])
        rebuilds = [e for e in entries if e["kind"] == "rebuild"]
        assert len(rebuilds) == 1 and rebuilds[0]["note"] == first_note, rebuilds
        results = [e for e in entries if e["kind"] == "result"]
        assert {e["call"] for e in results} == {"cell", "cell:1"} and all(e["outcome"] == "success" for e in results), results
        assert len(list(f.requests.glob("[0-9]*.body"))) == 3, "the last ask was answered twice"
