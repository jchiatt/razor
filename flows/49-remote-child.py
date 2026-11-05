"""A paired child keeps git on its owner and facts/cards on its parent: mail, wait, cap,
continuation, stale-report rejection, local descendants/channels, stop, settle and restart recovery.
The two real homes use reciprocal enrollment; only the model is a stand-in."""
import asyncio
import json
import re
import shutil
import threading
import time
from dataclasses import replace
from types import SimpleNamespace
from driver import flow
from model import agent, message, reply
from server import listed, send, start, stop, until
from terminal import Terminal

root, child = "FLOW_REMOTE_49", "You are oak49,"
replies = []
with flow(replies, config='[server]\naddress = "127.0.0.1:0"\n[children]\nmax = 1\ngrace = "0s"') as f:
    local, other = f.home / ".razor", f.home / ".razor-b"
    other.mkdir()
    for name in ("config.toml", "auth.json"):
        shutil.copy(local / name, other / name)
    beta = replace(f, cwd=f.cwd / "target", env={**f.env, "RAZOR_HOME": str(other)})
    beta.cwd.mkdir()
    f.base()
    beta.base()
    start(f)
    assert beta.run("devices").returncode == 0
    addresses = {h: json.loads((h / "server.json").read_text())["address"] for h in (local, other)}
    # Count recovery on the real peer connection, including ordinary status traffic.
    recovery, listening, control, silent = [], [], [], [False]

    def proxy():
        async def pipe(reader, writer, count):
            tail = b""
            try:
                while data := await reader.read(65536):
                    if count:
                        joined = tail + data
                        recovery.extend([None] * joined.count(b'"action":"report"'))
                        tail = joined[-16:]
                    if not silent[0]:
                        writer.write(data)
                        await writer.drain()
            except ConnectionError:
                pass
            finally:
                writer.close()

        async def handle(reader, writer):
            try:
                host, port = json.loads((other / "server.json").read_text())["address"].rsplit(":", 1)
                remote_reader, remote_writer = await asyncio.open_connection(host, int(port))
                await asyncio.gather(pipe(reader, remote_writer, True), pipe(remote_reader, writer, False))
            except OSError:
                writer.close()

        async def serve():
            server = await asyncio.start_server(handle, "127.0.0.1", 0)
            control.extend([asyncio.get_running_loop(), server])
            listening.append(server.sockets[0].getsockname()[1])
            await asyncio.Event().wait()  # Closing the listener must leave accepted sockets open.

        asyncio.run(serve())

    threading.Thread(target=proxy, daemon=True).start()
    until(SimpleNamespace(record=lambda _: listening), "proxy", bool)
    paired = f.run("devices", "add", "b", "--address", f"127.0.0.1:{listening[0]}")
    assert paired.returncode == 0, paired.stderr
    assert beta.run("devices", "add", "a", *paired.stdout.splitlines()[-1].split()[4:]).returncode == 0
    key = next(d["key"] for d in json.loads((local / "devices.json").read_text()) if d["name"] == "b")

    def request(home, path, body=None, *, peer=False, status=200, method="POST"):
        answer = send(home, method, path, body, key=key if peer else None)
        raw = answer.readline() if path == "/status" else answer.read()
        assert answer.status == status, (path, answer.status, raw)
        return json.loads(raw) if raw else None

    def far(session):
        return [json.loads(line) for line in (other / "sessions" / session / "record.jsonl").read_text().splitlines()]

    def ended(entries):
        return any(e["kind"] == "turn_end" for e in entries)

    def reports(session):
        return [e for e in f.record(session) if e["kind"] == "input" and "report" in e["from"]]

    remote = SimpleNamespace(record=far)
    # A paired helper may comment but cannot claim: its parent's settle owns another list.
    replies += [reply("TASK_OWNER_49", "mcp__razor__task", {"action": "write", "title": "Remote work"}),
                reply("TASK_OWNER_49")]
    assert beta.run("--print=json", "TASK_OWNER_49").returncode == 0
    # Shared files expand at the source; paths, worktrees and commits resolve at the target.
    (f.cwd / "brief49").write_text("SHARED_49")
    replies += [agent(root, "start", server="b", name="oak49", cwd=str(beta.cwd), shared_brief="brief49", brief="Commit."),
                agent(root, "start", server="b", name="over49", cwd=str(beta.cwd), brief="Over cap."),
                message(root, "note", to="oak49", text="PARENT_NOTE_49"), agent(root, "wait"), agent(root, "wait"), reply(root, text="First report."),
                reply(child, "Bash", {"command": "sleep 1; printf marker > marker49; git add -- marker49 && git -c user.name=Flow -c user.email=flow@example.invalid commit -m Marker"}),
                message(child, "note", text="CHILD_NOTE_49"),
                reply(child, "Bash", {"command": "sleep 60", "run_in_background": True}),
                reply(child, "mcp__razor__task", {"action": "write", "id": 1, "claim": True, "comment": "Must not land."}),
                reply(child, "mcp__razor__task", {"action": "write", "id": 1, "claim": False}),
                reply(child, "mcp__razor__task", {"action": "write", "id": 1, "comment": "Remote evidence."}),
                agent(child, "finish", status="finished", report="REMOTE_REPORT_49")]
    ran = f.run("--print=json", root)
    assert ran.returncode == 0, ran.stderr
    parent = json.loads(ran.stdout)["session"]
    records = until(f, parent, lambda es: any(e.get("text") == "First report." for e in es))
    started = next(e for e in records if e["kind"] == "child" and isinstance(e["event"], dict))
    qualified = started["session"]
    sid = qualified.split("@")[0]
    assert qualified.endswith("@b") and started["event"]["started"]["worktree"] is None
    there = far(sid)
    calls = {e["call"] for e in there if e["kind"] == "call" and e["name"] == "mcp__razor__task"}
    writes = [e for e in there if e["kind"] == "result" and e["call"] in calls]
    assert [e["outcome"] for e in writes] == ["error", "error", "success"], writes
    task = next((other / "tasks").rglob("1.md"))
    assert "claim:" not in task.read_text() and [p.read_text().split("---\n")[-1].strip() for p in task.parent.glob("comments/1/*.md")] == ["Remote evidence."]
    assert there[0]["child"]["parent"] == parent + "@a", there[0]
    assert there[0]["child"]["worktree"]["origin"] == str(beta.cwd)
    first_input = next(e for e in there if e["kind"] == "input")
    assert first_input["child_task"] == started["id"] and "SHARED_49" in first_input["text"]
    assert any(e["kind"] == "result" and e["outcome"] == "error" and "children.max" in e["output"] for e in records)
    until(f, parent, lambda es: len(reports(parent)) == 1)
    old = next(e for e in far(sid) if "report" in e)
    saved = old["report"]
    # The report is written with its turn's end, after the calls' results.
    assert old["kind"] == "turn_end" and old["id"] > max(e["id"] for e in there if e["kind"] == "result")
    assert any(e["kind"] == "job" and e["event"] == "ended" and e["id"] > old["id"] for e in far(sid)), \
        "the report was not saved before closing the child's background job"
    assert any(e["kind"] == "input" and e["text"] == "PARENT_NOTE_49" and e["from"]["mail"]["id"][0] == parent + "@a" for e in there)
    assert any(e["kind"] == "input" and e["text"] == "CHILD_NOTE_49" and e["from"]["mail"]["id"][0] == qualified for e in records)
    cards = SimpleNamespace(record=lambda _: request(local, "/status", method="GET")["agents"])
    rows = until(cards, parent, lambda rows: any(r["session"] == qualified for r in rows))
    row = next(r for r in rows if r["session"] == qualified)
    assert row["parent"] == parent and row["launched"] == started["event"]["started"]["launched"]
    assert row["report"]["entry"] == reports(parent)[0]["id"] and row["state"] == "done" and not row["changed_files"], row

    # A paired child's OSC id must fit the terminal protocol and survive reattachment.
    with Terminal(f, "--resume", parent, status=True) as t:
        t.until(lambda t: any(r.get("title") == "oak49" for r in t.records().values()))
        osc_id = next(k for k, r in t.records().items() if r.get("title") == "oak49")
        assert re.fullmatch(r"[A-Za-z0-9_.+-]{1,32}", osc_id), osc_id
    with Terminal(f, "--resume", parent, status=True) as t:
        t.until(lambda t: osc_id in t.records())
        assert t.records()[osc_id]["title"] == "oak49", t.records()

    # A continue fails while the child's server is down; the next one, once it is back, reaches it.
    stop(other)
    replies += [agent(root, "continue", child="oak49", brief="SECOND_TASK_49"), reply(root, text="Not sent.")]
    assert f.run("--resume", parent, "--print=json", "Continue.").returncode == 0
    assert "the child operation" in [e for e in f.record(parent) if e["kind"] == "result"][-1]["output"]
    assert beta.run("devices").returncode == 0
    # Hold the new task, then submit a real saved report from its older task through peer auth.
    release = beta.cwd / "release49"
    replies += [agent(root, "continue", child="oak49", brief="SECOND_TASK_49"), reply(root, text="Continued."), reply(root, text="Second report heard."),
                reply(child, "Bash", {"command": f"while [ ! -e {release} ]; do sleep 0.05; done"}),
                agent(child, "finish", status="partial", report="SECOND_REPORT_49")]
    assert f.run("--resume", parent, "--print=json", "Continue.").returncode == 0
    until(remote, sid, lambda es: any(e.get("text") == "SECOND_TASK_49" for e in es))
    request(local, "/children", {"action": "update", "parent": parent, "child": sid, "update": {"report": saved}}, peer=True)
    assert [e["text"] for e in reports(parent)] == ["REMOTE_REPORT_49"], "a stale report ended the new task"
    release.touch()
    until(f, parent, lambda es: len(reports(parent)) == 2)
    assert reports(parent)[-1]["text"] == "SECOND_REPORT_49"
    until(f, parent, lambda es: any(e.get("text") == "Second report heard." for e in es))
    request(local, "/children", {"action": "update", "parent": parent, "child": sid, "update": {"report": saved}}, peer=True)
    assert len(reports(parent)) == 2, "a duplicate report woke the parent"
    # A delayed continue of a task the child has moved past is refused, never run again.
    request(other, "/children", {"action": "continue", "session": sid, "input": first_input}, peer=True, status=400)

    # A person's continuation can exceed the parent's cap, as it can for a local child.
    replies += [agent(root, "start", server="b", name="hold49", cwd=str(beta.cwd), placement="in_place", brief="Hold the slot."),
                reply(root, text="Slot held."), reply("You are hold49,", "Bash", {"command": "sleep 60"})]
    assert f.run("--resume", parent, "--print=json", "Hold a slot.").returncode == 0
    hold = next(e["session"].split("@")[0] for e in f.record(parent) if e["kind"] == "child" and isinstance(e["event"], dict)
                and e["event"].get("started", {}).get("name") == "hold49")
    until(remote, hold, lambda es: any(e["kind"] == "call" and e["input"].get("command") == "sleep 60" for e in es))
    # The person reopens the remote child through its owning server, using the parent's fact id.
    replies += [agent(child, "start", server="a", name="third49", brief="Refuse the extra hop."),
                message(child, "note", to="channel", text="TARGET_CHANNEL_49"),
                agent(child, "start", name="local49", placement="in_place", brief="Local descendant."),
                agent(child, "wait"), agent(child, "settle", child="local49", decision="discard"),
                agent(child, "finish", status="finished", report="PERSON_REPORT_49"),
                agent("You are local49,", "finish", status="finished", report="LOCAL_49"), reply(root, text="Heard.")]
    request(other, f"/sessions/{sid}/input", {"text": "PERSON_TASK_49"})
    until(f, parent, lambda es: len(reports(parent)) == 3)
    assert reports(parent)[-1]["text"] == "PERSON_REPORT_49"
    until(f, parent, lambda es: any(e.get("text") == "Heard." for e in es))
    assert any(e["kind"] == "result" and e["outcome"] == "error" and "subtree" in e["output"] for e in far(sid))
    assert not list((local / "channels").glob("*.jsonl")), "the channel crossed the machine boundary"
    assert "TARGET_CHANNEL_49" in (other / "channels" / f"{sid}.jsonl").read_text()
    continued = [e for e in f.record(parent) if e["kind"] == "child" and e["event"] == "continued"]
    person = next(e for e in far(sid) if e["kind"] == "input" and e["text"] == "PERSON_TASK_49")
    assert person["from"] == "person" and person["child_task"] == continued[-1]["id"]
    replies += [agent(root, "stop", child="hold49"), agent(root, "settle", child="hold49", decision="discard"), reply(root, text="Slot free.")]
    assert f.run("--resume", parent, "--print=json", "Free the slot.").returncode == 0

    # Stop closes remote jobs; another continuation finishes while the parent's server is down.
    replies += [agent(root, "continue", child="oak49", brief="STOP_TASK_49"), reply(root, text="Running."),
                reply(child, "Bash", {"command": "sleep 60 # STOP_TASK_49"})]
    assert f.run("--resume", parent, "--print=json", "Run.").returncode == 0
    until(remote, sid, lambda es: any(e["kind"] == "call" and e["input"].get("command") == "sleep 60 # STOP_TASK_49" for e in es))
    replies += [agent(root, "stop", child="oak49"), reply(root, text="Stopped.")]
    assert f.run("--resume", parent, "--print=json", "Stop.").returncode == 0
    assert any(e["kind"] == "child" and e["event"] == "stopped" for e in f.record(parent))
    assert not listed(local, qualified), "a stopped helper stayed in its parent’s agents list"
    go = beta.cwd / "recover49"
    replies += [agent(root, "continue", child="oak49", brief="RECOVER_TASK_49"), reply(root, text="Recovering."),
                reply("RECOVER_TASK_49", "Bash", {"command": f"while [ ! -e {go} ]; do sleep 0.05; done"}),
                agent("RECOVER_TASK_49", "finish", status="finished", report="RECOVER_REPORT_49")]
    assert f.run("--resume", parent, "--print=json", "Recover.").returncode == 0
    until(remote, sid, lambda es: any(e["kind"] == "input" and e["text"] == "RECOVER_TASK_49" for e in es))
    stop(local)
    assert (local / "live" / parent).exists(), "a parent with an outstanding remote task lost its live marker"
    go.touch()
    until(remote, sid, lambda es: sum("report" in e for e in es) == 4)
    assert len(reports(parent)) == 3
    # Bind the same address, so the existing reciprocal enrollment remains valid on restart.
    config = local / "config.toml"
    config.write_text(config.read_text().replace('127.0.0.1:0', addresses[local]))
    replies += [reply(root, text="Recovered.")]
    start(f)
    until(f, parent, lambda es: len(reports(parent)) == 4)
    assert reports(parent)[-1]["text"] == "RECOVER_REPORT_49"
    until(f, parent, lambda es: any(e.get("text") == "Recovered." for e in es))
    request(local, "/children", {"action": "update", "parent": parent, "child": sid,
            "update": {"report": next(e["report"] for e in reversed(far(sid)) if "report" in e)}}, peer=True)
    assert len(reports(parent)) == 4, "push and recovery each admitted the report"
    replies += [agent(root, "settle", child="oak49", decision="merge"), reply(root, text="Merged.")]
    with Terminal(f, "--resume", parent, status=True) as t:
        t.until(lambda t: osc_id in t.records())
        assert f.run("--resume", parent, "--print=json", "Merge.").returncode == 0
        t.until(lambda t: osc_id not in t.records())
    assert (beta.cwd / "marker49").read_text() == "marker" and not (f.cwd / "marker49").exists()
    assert not (other / "sessions" / sid / "worktree").exists()
    row = next(r for r in request(local, "/status", method="GET")["agents"] if r["session"] == qualified)
    assert row["settled"] == "merge", row
    assert not listed(other, sid), "a remotely settled helper stayed in its owner’s agents list"
    request(local, "/children", {"action": "report", "session": sid}, status=400)
    request(other, "/children", {"action": "report", "session": qualified}, peer=True, status=400)
    # An idle child has no report; unrelated peer activity must not cause recovery per line.
    replies += [agent(root, "start", server="b", name="idle49", cwd=str(beta.cwd), placement="in_place", brief="Ask."),
                agent(root, "wait"), reply(root, text="Idle held."),
                message("You are idle49,", "question", text="IDLE_QUESTION_49?"), reply("You are idle49,", text="Waiting.")]
    assert f.run("--resume", parent, "--print=json", "Hold idle.").returncode == 0
    idle = next(e["session"].split("@")[0] for e in f.record(parent) if e["kind"] == "child" and isinstance(e["event"], dict)
                and e["event"].get("started", {}).get("name") == "idle49")
    until(remote, idle, ended)
    before = len(recovery)
    replies += [reply("BUSY_49", "Bash", {"command": "true"}) for _ in range(20)] + [reply("BUSY_49", text="Busy done.")]
    assert beta.run("--print=json", "BUSY_49").returncode == 0
    assert len(recovery) - before <= 2, "ordinary peer updates repeatedly recovered an idle child"
    replies += [agent(root, "stop", child="idle49"), agent(root, "settle", child="idle49", decision="discard"), reply(root, text="Idle released.")]
    assert f.run("--resume", parent, "--print=json", "Release idle.").returncode == 0
    # Finishing and settling a local ancestor closes and retires its foreign leaf as well.
    leaf_started = beta.cwd / "leaf-started49"
    replies += [agent(root, "start", name="ancestor49", placement="in_place", brief="Close the remote leaf."),
                agent(root, "wait"), agent(root, "settle", child="ancestor49", decision="discard"), reply(root, text="Ancestor settled."),
                agent("You are ancestor49,", "start", server="b", name="leaf49", cwd=str(beta.cwd), brief="Wait for closure."),
                reply("You are ancestor49,", "Bash", {"command": f"while [ ! -e {leaf_started} ]; do sleep 0.05; done"}),
                agent("You are ancestor49,", "finish", status="finished", report="Ancestor finished."),
                reply("You are leaf49,", "Bash", {"command": f"touch {leaf_started}; sleep 60"})]
    ran = f.run("--resume", parent, "--print=json", "Close the ancestor.")
    assert ran.returncode == 0, ran.stderr
    ancestor = next(e["session"] for e in f.record(parent) if e["kind"] == "child" and isinstance(e["event"], dict)
                    and e["event"].get("started", {}).get("name") == "ancestor49")
    assert any(e["kind"] == "child" and e["event"] == "stopped" and e["session"].endswith("@b") for e in f.record(ancestor))
    assert not (local / "live" / ancestor).exists(), "the retired ancestor kept an outstanding remote fact"
    # A local child's report still reaches its parent when its remote descendant is down.
    ready, release = beta.cwd / "lost-ready49", beta.cwd / "lost-release49"
    replies += [agent(root, "start", name="outage49", placement="in_place", brief="Report during an outage."),
                agent(root, "wait", timeout_ms=5000), reply(root, text="Outage report heard."),
                agent("You are outage49,", "start", server="b", name="lost49", cwd=str(beta.cwd), brief="Hold."),
                reply("You are outage49,", "Bash", {"command": f"while [ ! -e {release} ]; do sleep 0.05; done"}),
                agent("You are outage49,", "finish", status="finished", report="OUTAGE_REPORT_49"),
                reply("You are lost49,", "Bash", {"command": f"touch {ready}; while [ ! -e {release} ]; do sleep 0.05; done"})]
    result = {}
    worker = threading.Thread(target=lambda: result.setdefault("ran", f.run("--resume", parent, "--print=json", "Report during the outage.")))
    worker.start()
    until(SimpleNamespace(record=lambda _: [ready.exists()]), parent, any)
    # A read leaves reusable connections open. The peer then goes silent on those sockets
    # and refuses fresh connections: mail and child mutations must say nothing was sent.
    request(local, "/search?q=warm49", method="GET")
    silent[0] = True
    closed = threading.Event()
    def disconnect():
        control[1].close()
        closed.set()
    control[0].call_soon_threadsafe(disconnect)
    assert closed.wait(2), "proxy listener did not close"
    replies += [agent("SILENT_49", "start", server="b", name="unsent49", cwd=str(beta.cwd), brief="Hold."),
                message("SILENT_49", "note", to=f"{sid}@b", text="UNSENT_49"), reply("SILENT_49")]
    began = time.monotonic()
    attempted = f.run("--print=json", "SILENT_49")
    assert attempted.returncode == 0, attempted.stderr
    results = [e for e in f.record(json.loads(attempted.stdout)["session"]) if e["kind"] == "result"]
    assert len(results) == 2 and all(e["outcome"] == "error" and ("nothing was sent" in e["output"] or "was not sent" in e["output"]) for e in results), results
    assert time.monotonic() - began < 4, "silent pooled connections delayed a mutation"
    assert not any(e.get("text") == "UNSENT_49" for e in far(sid)), "refused mail arrived"
    stop(other)
    release.touch()
    worker.join(timeout=60)
    assert not worker.is_alive() and result["ran"].returncode == 0
    assert any("OUTAGE_REPORT_49" in e["text"] for e in reports(parent)), "the descendant outage lost its local parent's report"
