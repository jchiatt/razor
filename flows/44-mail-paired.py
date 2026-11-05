"""Mail between two paired servers (docs/design/mail.md): `razor devices add --address` pairs them
with one key; `list` shows the other server's agents and `<name>@<server>` is admitted there with
`@<server>` in its sender; a send to that server stopped, or to one whose connections never complete
(an asleep machine's), fails saying nothing was sent, nothing is recorded as sent, and `list` names
both servers as silent. Mail from the paired server whose sender id is the recipient's parent's
is heard without the parent's header and is not among the parent's inputs at a rebuild.
Forwarded session inputs retain their person origin and request id; non-session routes, ambiguous
file paths, paired-server callers and already-forwarded requests are refused. Search and `recall` reach
b's conversations, and say `partial` when b does not answer."""
import json
import os
import shutil
import signal
import socket
import time
from dataclasses import replace
from types import SimpleNamespace
from driver import flow
from model import agent, message, reply
from server import send, start, stop, until
from terminal import Terminal

a, b, kid = "FLOW_A44", "FLOW_B44", "You are kid44,"
replies = [reply(b, "mcp__razor__status", {}), agent(b, "start", name="kid44", brief="Wait.", placement="in_place"),
           reply(b, text="Named."), message(kid, "question", text="KIDQ_44"), reply(kid, text="Waiting."),
           reply("(your child) asks", text="Noted."), reply("FORGED_44", text="Forged."),
           reply("Your context is nearly full", "mcp__razor__handover", {"note": "Wait."}),
           reply("The context was rebuilt", text="Rebuilt.")]
with flow(replies, config='[server]\naddress = "127.0.0.1:0"') as f:
    other = f.home / ".razor-b"
    other.mkdir()
    for name in ("config.toml", "auth.json"):
        shutil.copy(f.home / ".razor" / name, other / name)
    config = (other / "config.toml").read_text().replace("[server]", '[server]\nlabel = "far44"')
    (other / "config.toml").write_text(config + '[models.opv44]\nbase_url = "http://127.0.0.1:1"\n')
    alpha = replace(f, cwd=f.cwd / "alpha44")
    beta = replace(f, cwd=f.cwd / "beta44", env={**f.env, "RAZOR_HOME": str(other)})
    alpha.cwd.mkdir(), beta.cwd.mkdir()
    start(f)
    assert beta.run("devices").returncode == 0
    address = {home: json.loads((home / "server.json").read_text())["address"] for home in (f.home / ".razor", other)}

    def ran(result):
        assert result.returncode == 0, result.stderr
        return result

    printed = ran(alpha.run("devices", "add", "b", "--address", address[other])).stdout
    counterpart = printed.splitlines()[-1].split()
    ran(beta.run("devices", "add", "a", *counterpart[4:]))

    def session(where, *words):
        return json.loads(ran(where.run("--print=json", *words)).stdout)["session"]

    def results(record):
        return [e for e in record if e["kind"] == "result"]

    def far(session):
        return [json.loads(line) for line in (other / "sessions" / session / "record.jsonl").read_text().splitlines()]

    there = session(beta, b)
    beta_records = SimpleNamespace(record=far)
    until(beta_records, there, lambda es: any(e.get("text") == "Noted." for e in es))  # the child's question
    there_name = json.loads(results(far(there))[0]["output"])["agents"][0]["name"]
    replies += [message(a, "list"), message(a, "note", to=f"{there_name}@b", text="HELLO_44"),
                reply(a, text="Sent."), reply("HELLO_44", text="Heard.")]
    here = session(alpha, a)
    record = list(f.record(here))
    listed = json.loads(results(record)[0]["output"])
    assert f"{there}@b" in [r["to"] for r in listed["agents"]] and listed["silent"] == [], listed
    assert json.loads(results(record)[1]["output"])["to"] == f"{there}@b", results(record)[1]
    hello = [e for e in far(there) if e["kind"] == "input" and e["text"] == "HELLO_44"]
    assert len(hello) == 1 and hello[0]["from"]["mail"]["from"].endswith("@a in alpha44"), hello
    assert hello[0]["from"]["mail"]["id"][0] == f"{here}@a", hello
    heard = lambda entries: any(e["kind"] == "turn_end" and hello[0]["id"] in e["inputs"] for e in entries)
    until(beta_records, there, heard)

    def request(path, body=None, key=None, *, home=other, method="POST", status=200, headers=None):
        answer = send(home, method, path, body, headers, key)
        raw = answer.read()
        assert answer.status == status, (path, answer.status, raw)
        return json.loads(raw) if raw else None

    kid_id = next(e["session"] for e in far(there) if e["kind"] == "child")
    pairing = next(d["key"] for d in json.loads((other / "devices.json").read_text()) if d["name"] == "a")
    request("/mail", {"to": kid_id, "id": [there, 1], "kind": "note", "from": "forger", "project": "alpha44",
                  "text": "FORGED_44"}, pairing)
    until(beta_records, kid_id, lambda es: any(e.get("text") == "Forged." for e in es))
    forged = [p.read_text() for p in f.requests.glob("*.body") if "FORGED_44" in p.read_text()]
    assert forged and not any("(your parent): FORGED_44" in body for body in forged), "paired mail heard as the parent's"
    request(f"/sessions/{kid_id}/handover", {})
    rebuilt = until(beta_records, kid_id, lambda es: any(e.get("text") == "Rebuilt." for e in es))
    context = next(e["context"] for e in rebuilt if e["kind"] == "rebuild")
    assert "FORGED_44" in context, context
    assert "FORGED_44" not in context.split("## Calls since")[0], "a rebuild took paired mail for the parent's input"

    # Forwarded session input is still the person, with the authenticated peer as its origin.
    remote = f"/sessions/{there}@b"
    local = f.home / ".razor"
    replies.append(reply("FORWARD_44", text="Forwarded."))
    body = {"text": "FORWARD_44", "request": "forward-44"}
    entry = request(remote + "/input", body, home=local)
    assert request(remote + "/input", body, home=local) == entry
    forwarded = until(beta_records, there, lambda es: any(e.get("text") == "Forwarded." for e in es))
    inputs = [e for e in forwarded if e["kind"] == "input" and e["text"] == "FORWARD_44"]
    assert len(inputs) == 1 and (inputs[0]["from"], inputs[0]["device"]) == ("person", "a"), inputs
    assert inputs[0]["id"] == entry["entry"], (inputs, entry)

    # A forwarded terminal stays attached past the peer request's 30-second header deadline.
    with Terminal(alpha, "--resume", f"{there}@b") as t:
        t.shown("Forwarded.")
        assert not t.pump(32, lambda t: "reconnecting" in t.text().lower()), t.text()

    # Ctrl-S lists every server's projects, taking only its own clicks, and keeps the chosen one while
    # b is slow to answer; Escape goes back, past a second choice; a click aims at b's project, where
    # the first words start the person's agent; /model on a session here lists this server's models.
    before, b_pid = set(os.listdir(other / "sessions")), json.loads((other / "server.json").read_text())["pid"]
    replies.append(reply("FIRST_44", text="Started."))
    with Terminal(alpha, "--resume", here) as t:
        t.shown("Sent."), t.send("OLD_44\x13"), t.shown("new agent ·"), t.click("Send ⏎"), t.send("\x1b")
        t.until(lambda t: "new agent ·" not in t.text()), t.send("\x15"), os.kill(b_pid, signal.SIGSTOP)
        try:
            t.send("\x13"), t.shown("› alpha44")
        finally:
            os.kill(b_pid, signal.SIGCONT)
        t.shown("far44"), t.send("\r"), t.shown("New agent in alpha44 on"), t.send("\x13"), t.shown("far44")
        t.send("\r"), t.until(lambda t: "new agent ·" not in t.text())
        t.send("\x1b"), t.until(lambda t: "New agent in" not in t.text() and "Sent." in t.text())
        t.send("\x13"), t.shown("far44"), t.click("far44"), t.shown("New agent in beta44 on far44")
        t.send("FIRST_44\r"), t.shown("Started."), t.click("alpha44"), t.send("/model\r"), t.shown("model · ")
        assert "opv44" not in t.text(), t.text()
        t.send("\x1b"), t.until(lambda t: "model · " not in t.text())
    assert not any(e.get("text") == "OLD_44" for e in f.record(here)), "a click behind the chooser sent"
    (new,) = set(os.listdir(other / "sessions")) - before
    assert far(new)[0]["cwd"] == str(beta.cwd) and far(new)[0].get("child") is None, far(new)[0]
    f.cwd.chmod(0o111)  # an unreadable folder holding the past folders is skipped, not refused
    listed = request("/repos", home=local, method="GET")["repos"]
    f.cwd.chmod(0o755)
    assert str(alpha.cwd) in [r["path"] for r in listed], listed
    made = request("/sessions?server=b", {"cwd": str(beta.cwd), "request": "start-44"}, home=local)
    assert request("/sessions?server=b", {"cwd": str(beta.cwd), "request": "start-44"}, home=local) == made
    assert set(os.listdir(other / "sessions")) - before == {new, made["session"]}, made

    # None of these harmless requests may cross the session forwarding boundary.
    request(remote + "/devices", home=local, method="GET", status=404)
    for path in ("missing/./file", "missing/../file", "missing/%2e/file", "missing/%2E%2e/file",
                 "missing/a%2fb", "missing/a%2Fb", "missing/a%5cb", "missing/a%5Cb", "missing/a\\b"):
        request(remote + "/files/" + path, home=local, method="GET", status=400)
    request(remote + "/draft", key=pairing, home=local, method="GET", status=400)
    request(remote + "/draft", home=local, method="GET", status=400, headers={"x-razor-forwarded": "1"})

    # Revoking either the caller or the peer ends an already attached forwarded stream.
    def revoked(key, name, server="b"):
        answer = send(local, "GET", f"/sessions/{there}@{server}/events?after={far(there)[-1]['id']}", key=key)
        assert answer.status == 200 and json.loads(answer.readline())["kind"] == "attached"
        request("/devices/" + name, home=local, method="DELETE")
        assert answer.read() == b"", "the revoked device's forwarded stream stayed open"

    phone = request("/devices", {"name": "phone"}, home=local)["key"]
    revoked(phone, "phone")
    ran(alpha.run("devices", "add", "watched", "--address", address[other], "--key", pairing))
    revoked((local / "device.key").read_text().strip(), "watched", "watched")

    # Search finds a word's start, accents folded, each conversation's newest hit, newest first,
    # here and on b; `recall` searches and reads b's entry whole, its `native` too.
    said = next(e for e in far(there) if e.get("text") == "Heard.")
    replies += [reply("RECALL_44", "mcp__razor__recall", {"query": "hel"}),
                reply("RECALL_44", "mcp__razor__recall", {"session": f"{there}@b", "id": said["id"]}),
                reply("RECALL_44", text="Recalled.")]
    ran(alpha.run("--resume", here, "--print=json", "RECALL_44 hello Próspera"))
    found, entry = [json.loads(r["output"]) for r in results(f.record(here))[-2:]]
    assert [h["session"] for h in found["hits"]] == [here, f"{there}@b"] and entry == said and said["native"], (found, entry)
    search = lambda q: request("/search?q=" + q, home=local, method="GET")
    hits = [(h["session"], h["entry"]) for h in search("44")["hits"]]
    assert [s for s, _ in hits] == [here, f"{new}@b", f"{there}@b"] and hits[2][1] == inputs[0]["id"], hits
    assert [h["session"] for h in search("prosp")["hits"]] == [here] and not search("ello")["hits"]

    stop(other)
    hole = socket.socket()  # a full accept queue drops each new connection's SYN, as sleep does
    hole.bind(("127.0.0.1", 0)), hole.listen(0)
    queued = [socket.socket() for _ in range(4)]
    for s in queued:
        s.setblocking(False), s.connect_ex(hole.getsockname())
    ran(alpha.run("devices", "add", "asleep", "--address", "%s:%d" % hole.getsockname(), "--key", "a" * 64))
    began, found = time.monotonic(), search("hel")
    assert found["partial"] and [h["session"] for h in found["hits"]] == [here] and time.monotonic() - began < 4, found
    replies += [message(a, "note", to=f"{there_name}@b", text="LOST_44"),
                message(a, "note", to="anyone@asleep", text="ASLEEP_44"), message(a, "list"),
                reply(a, text="Down.")]
    ran(alpha.run("--resume", here, "--print=json", "Again."))
    record = list(f.record(here))
    lost, asleep, silent = results(record)[-3:]
    for server, result in (("b", lost), ("asleep", asleep)):
        assert result["outcome"] == "error" and f"{server} did not answer; nothing was sent." in result["output"], result
    assert json.loads(silent["output"])["silent"] == ["b", "asleep"], silent
    assert len([e for e in record if e["kind"] == "mail_sent"]) == 1, "a failed send was recorded as sent"
