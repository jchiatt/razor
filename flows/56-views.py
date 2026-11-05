"""Saved views (docs/design/agents-view.md): A stores definitions and B forwards edits to it.
Both resolve layouts, send only changed views, reconnect whole and coalesce publications.
The real terminal edits through $EDITOR, retains a stale save, opens and sends paired input.
Earlier follows terminal presence and replies since a stream last read the session; list Escape
stops nothing and duplicate shortcut rows cannot trap keyboard navigation. Both terminal lists
follow the same saved view; B's list shows both servers, its own labelled, and A's resident, apart from the groups."""
import json
import mcp
import os
import shutil
from dataclasses import replace
from types import SimpleNamespace
import time
from driver import flow
from model import agent, message, reply
from server import Local, send, stop, until, wait
from terminal import Terminal

replies = [agent("RESIDENT_56", "start", name="resident-kid56", brief="Deliver and wait.", placement="in_place"),
           reply("RESIDENT_56", text="Started."),
           agent("You are resident-kid56,", "start", name="resident-grandkid56", brief="Wait.", placement="in_place"),
           reply("You are resident-grandkid56,", "Bash", {"command": "sleep 60"}),
           reply("You are resident-kid56,", "mcp__razor__deliver", {"line": "resident-unread56", "url": "https://example.com/resident56"}),
           reply("You are resident-kid56,", text="Idle with a delivery."),
           reply("THREAD_56", text="thread56"), reply("voice-words56", text="voice56"), reply("FLOW_A56", text="alpha-visible"), reply("FLOW_B56", text="beta-visible"),
           reply("FLOW_C56", "mcp__razor__deliver", {"line": "unread56", "url": "https://example.com/unread56"}), reply("FLOW_C56", text="loose-visible"), reply("STEER_56", "Bash", {"command": "printf observed56"}),
           reply("STEER_56", text="steered"),
           reply("WORK_56", "Bash", {"command": "sleep 60"}),
           message("ASK_56", "question", text="Choose 56", options=["yes", "no"]),
           reply("ASK_56", text="answered56"),
           reply("You are kid56,", "Bash", {"command": "sleep 2"}),
           reply("You are kid56,", "mcp__razor__mcp__ask56__form", {}),
           agent("CHILD_56", "start", name="kid56", brief="Wait.", placement="in_place"),
           reply("CHILD_56", text="parent56")]
# The recorder and the transcriber are stand-ins for devices: a steady tone, and one phrase.
voice = 'capture = ["/usr/bin/python3", "-c", "import os,struct,time\\nwhile 1: os.write(1,struct.pack(\'<h\',5000)*1600); time.sleep(.1)"]\ntranscribe = ["/bin/sh", "-c", "cat >/dev/null; echo voice-words56"]\n'
with mcp.serve() as external, flow(replies, config=voice + f'[server]\naddress = "127.0.0.1:0"\n[mcp.ask56]\nurl = "{external.url}/mcp"\ntools = ["form"]') as f:
    a, b = f.home / ".razor", f.home / ".razor-b"
    b.mkdir()
    shutil.copy(a / "auth.json", b / "auth.json")
    alpha = replace(f, cwd=f.cwd / "alpha56")
    beta = replace(f, cwd=f.cwd / "beta56", env={**f.env, "RAZOR_HOME": str(b)})
    loose = replace(beta, cwd=f.cwd / "loose56")
    for where in (alpha, beta, loose):
        where.cwd.mkdir()
    assert alpha.run("devices").returncode == 0
    owner = (a / "server.id").read_text().strip()
    (b / "config.toml").write_text((a / "config.toml").read_text().replace(
        'address = "127.0.0.1:0"', f'address = "127.0.0.1:0"\nshared = "{owner}"\nlabel = "box · personal"'))
    assert beta.run("devices").returncode == 0
    try:
        address = {home: json.loads((home / "server.json").read_text())["address"] for home in (a, b)}
        printed = alpha.run("devices", "add", "b", "--address", address[b]).stdout
        assert beta.run("devices", "add", "a", *printed.splitlines()[-1].split()[4:]).returncode == 0

        def status(home, query="", path="/v1/status"):
            connection = Local(home / "server.sock")
            connection.timeout = 20  # Heartbeats arrive every 15 seconds.
            key = (home / "device.key").read_text().strip()
            connection.request("GET", path + query, headers={"Authorization": f"Bearer {key}"})
            return connection.getresponse()

        # B forwards only once its reader of A has heard A's id.
        wait(lambda: any(s["id"] == owner and not s["silent"] for s in json.loads(status(b).readline())["servers"]), 30, "B never heard from A")

        def request(home, method, path, body=None, status=200, query=""):
            answer = send(home, method, path + query, body)
            raw = answer.read()
            assert answer.status == status, (path, answer.status, raw)
            return json.loads(raw)

        def session(where, home, text):
            made = request(home, "POST", "/sessions", {"cwd": str(where.cwd)})["session"]
            request(home, "POST", f"/sessions/{made}/input", {"text": text})
            path = home / "sessions" / made / "record.jsonl"
            records = SimpleNamespace(record=lambda _: map(json.loads, path.read_bytes().split(b"\n")[:-1]))
            until(records, made, lambda es: es[-1]["kind"] == "turn_end")
            return made

        there = session(alpha, a, "FLOW_A56")
        here, unclaimed = session(beta, b, "FLOW_B56"), session(loose, b, "FLOW_C56")

        # 1. The owner serves version 0; a write through B is forwarded and both read version 1,
        # an old view's `claims` saved as `matches`.
        assert request(a, "GET", "/views") == {"version": 0, "views": [], "pinned": []}
        group = {"id": "g56", "name": "Atlas", "colour": "plum",
                 "claims": [{"folder": "alpha56"}, {"session": here}]}
        saved = request(b, "PUT", "/views", {"version": 0, "views": [{"name": "Work", "groups": [group]}]})
        assert saved["version"] == 1 and saved["views"][0]["groups"][0]["matches"] == group["claims"], saved
        assert request(a, "GET", "/views") == saved == request(b, "GET", "/views")

        # 2. A stale write or a fourth group level changes nothing.
        request(b, "PUT", "/views", {"version": 0, "views": []}, status=409)
        deep = {**group, "groups": [{**group, "id": "g2", "groups": [{**group, "id": "g3", "groups": [{**group, "id": "g4"}]}]}]}
        request(a, "PUT", "/views", {"version": 1, "views": [{"name": "Deep", "groups": [deep]}]}, status=400)
        # Under the bound as sent, over it as stored with its defaults written out: refused.
        many = [{"id": f"g{i}", "name": "n", "colour": "plum"} for i in range(4000)]
        assert len(json.dumps(many)) < 256 << 10
        request(a, "PUT", "/views", {"version": 1, "views": [{"name": "Big", "groups": many}]}, status=413)
        assert request(a, "GET", "/views") == saved

        # 3. Layouts carry only resolved ids, agree across both servers, and reconnect whole.
        def layout(home, version, test=lambda _: True):
            layouts, deadline = {}, time.monotonic() + 30
            with status(home, "?views=true") as stream:
                for l in map(json.loads, iter(stream.readline, b"")):
                    assert time.monotonic() < deadline, f"Views never reached version {version}"
                    if l["kind"] == "views":
                        layouts.update((v["name"], v) for v in l["layouts"])
                        l["layouts"] = [layouts[n] for n in l["names"] if n in layouts]
                        if l["version"] == version and not l["error"] and test(l): return l

        resolved = layout(b, 1)
        assert resolved["names"] == ["Work"] and "views" not in resolved, resolved
        atlas = next(s for s in resolved["layouts"][0]["sections"] if s["id"] == "group/g56")
        assert {r["session"].split("@")[0] for r in atlas["sessions"]} == {there, here}, atlas
        assert layout(a, 1)["layouts"][0]["sections"][0]["kind"] == "group"
        # Server assigns omitted group ids and preserves unknown manual-order ids. Atlas's agents move
        # to a subgroup, which keeps its heading; Later, with no agents, is not drawn.
        inner = {**group, "claims": [], "order": ["unseen"], "groups": [{"name": "Inner56", "colour": "sand", "claims": [], "groups": [{"name": "Deep56", "colour": "steel", "claims": group["claims"]}]}]}
        edited = {**saved, "views": [{**saved["views"][0], "groups": [inner, {"name": "Later", "colour": "moss"}]},
                                   {"name": "Other"}]}
        saved = request(a, "PUT", "/views", edited)
        assert saved["views"][0]["groups"][1]["id"] and saved["views"][0]["groups"][0]["order"] == ["unseen"]
        stream = status(b, "?views=true")
        next(l for l in map(json.loads, iter(stream.readline, b""))
             if l["kind"] == "views" and l["version"] == 2)
        renamed = {**saved, "views": [{**saved["views"][0], "groups": [{**saved["views"][0]["groups"][0], "name": "Atlas 2"}, saved["views"][0]["groups"][1]]}, saved["views"][1]]}
        saved = request(a, "PUT", "/views", renamed)
        changed = next(l for l in map(json.loads, iter(stream.readline, b"")) if l["kind"] == "views" and l["version"] == 3)
        assert [l["name"] for l in changed["layouts"]] == ["Work"], changed
        stream.close()
        nested = layout(b, 3)
        assert len(nested["layouts"]) == 2, "reconnect lost unchanged layouts"
        third = next(s for s in nested["layouts"][0]["sections"] if s["name"] == "Deep56")
        assert third["depth"] == 2 and len(third["sessions"]) == 2, third
        assert saved["views"][0]["groups"][0]["groups"][0]["groups"][0]["id"]

        # Two paired idle homes update their gauges without resending every agent.
        time.sleep(1)
        stream, heartbeats = status(a, "?heartbeat=true"), []
        assert json.loads(stream.readline())["kind"] == "agents"
        deadline = time.monotonic() + 35
        while time.monotonic() < deadline:
            heartbeats.append(json.loads(stream.readline()))
        stream.close()
        assert not any(l["kind"] == "agents" for l in heartbeats), heartbeats
        assert any(l["kind"] == "server" and l.get("name") == "b" for l in heartbeats), heartbeats

        # 4. No person input and no open terminal belongs to Earlier; opening it brings it live.
        old = request(b, "POST", "/sessions", {"cwd": str(beta.cwd)})["session"]
        earlier = layout(b, 3, lambda l: any(r["session"] == old for s in l["layouts"][0]["sections"] for r in s["sessions"]))["layouts"][0]["sections"]
        assert old in [r["session"] for s in earlier if s["kind"] == "earlier" for r in s["sessions"]], earlier
        names = {s["session"].split("@")[0]: s["name"] for s in json.loads(status(b).readline())["agents"]}
        request(b, "POST", f"/sessions/{here}/input", {"text": "WORK_56"})
        # An editor is a real child process: it asserts cooked mode and writes the actual file.
        editor = f.cwd / "edit-views.py"
        editor.write_text("import json,sys,termios\n"
                          "assert termios.tcgetattr(0)[3] & termios.ICANON\n"
                          "p=sys.argv[1]; v=json.load(open(p)); v['views'][0]['groups'][0]['name']='Edited'\n"
                          "open(p,'w').write(json.dumps(v))\n")
        beta = replace(beta, env={**beta.env, "EDITOR": f"{os.sys.executable} {editor}"})
        # A resident added now is listed only once its home is made, within a minute.
        (a / "config.toml").write_text((a / "config.toml").read_text() + f"[residents.ember56]\ncwd = {json.dumps(str(f.cwd))}\n")
        assert "ember56" not in status(a).readline().decode() or (a / "sessions/ember56").exists(), "a resident without a home is listed"
        with Terminal(beta, "--resume", here, columns=160, rows=50) as t:
            t.shown("beta-visible")
            t.send("\x1bOQ")
            t.shown("AGENTS", "Atlas 2", "Inner56", names[unclaimed], "servers · all 2 answer", "1 resident", timeout=90)
            assert "Later" not in t.text(), t.text()
            for columns, gutter in ((49, False), (50, True)):
                t.resize(50, columns)
                t.until(lambda t: t.screen.display[0][columns - 1] == "┐" and any(line[15] == "│" for line in t.screen.display[2:20]) == gutter)
            t.click("▾ Atlas 2")
            t.until(lambda t: "Inner56" not in t.text() and names[here] not in t.text())
            t.click("▸ Atlas 2")
            t.shown("Inner56")
            t.resize(50, 160)
            t.until(lambda t: t.screen.display[0][52] == "┐")
            config = b / "config.toml"
            config.write_text(config.read_text() + '[ui.keys.global]\nstatus = ["Ctrl-Y"]\n')
            t.shown("Ctrl-Y")
            t.send("\x19")
            t.shown("── USAGE")
            t.send("\x19")
            t.until(lambda t: "── USAGE" not in t.text())
            t.click("▸ Residents")
            t.click("▸ servers")
            t.shown("load", "mem", "personal", "○ ember56 ")
            t.click("○ ember56 ")
            t.until(lambda _: next(r for r in json.loads(status(a).readline())["agents"] if r["session"] == "ember56")["terminal"])
            # /thread starts a thread on A, the resident's server, as ember56's.
            t.send("/thread ember56\r")
            t.shown("New thread with ember56")
            t.send("THREAD_56\r")
            t.shown("thread56")
            # A voice note as a thread's first message makes it there too, not on this server.
            t.send("/thread ember56\r")
            t.shown("New thread with ember56")
            t.send("\x12")
            t.shown("Recording on", "0:01")
            t.send("\r")
            t.shown("voice56")
            assert len(request(a, "GET", "/sessions", query="?resident=ember56&jobs=false")) == 2
            t.click(f" {names[there]} ")
            t.shown("alpha-visible")
            t.send("/name named56\r")
            t.until(lambda t: " named56 " in "\n".join(line[:52] for line in t.screen.display)
                    and "named56" in t.screen.display[2][53:])
            names[there] = "named56"
            t.send("STEER_56\r")
            until(f, there, lambda es: any(e.get("text") == "steered" for e in es))
            steer = next(e for e in f.record(there) if e["kind"] == "input" and e["text"] == "STEER_56")
            assert (steer["from"], steer["device"]) == ("person", "b"), steer
            t.send("/views\r")
            t.shown("Edited")
            assert request(a, "GET", "/views")["version"] == 4
            # One arrow from Atlas's first agent to its second moves only the highlight, Escape
            # stops nothing, and Enter opens it.
            t.send("\x1b[D")
            y = lambda name: next(y for y, line in enumerate(t.screen.display) if f" {name} " in line[:52])
            t.send(("\x1b[B" if y(names[here]) > y(names[there]) else "\x1b[A") + "\x1b")
            conversation = lambda: "\n".join(line[53:] for line in t.screen.display)
            t.until(lambda t: "❯" in t.screen.display[y(names[here])][:52])
            t.pump(1)
            assert "beta-visible" not in conversation() and "steered" in conversation(), t.text()
            records = list(map(json.loads, (b / "sessions" / here / "record.jsonl").read_text().splitlines()))
            assert all(e.get("end") != "stopped" for e in records), records[-3:]
            t.send("\r\x19")
            t.shown("── USAGE", "Work", "beta-visible")
            t.send("/view Other\r")
            t.send("/resume\r")
            t.shown("AGENTS · Other")
            t.send("\x1b")
            t.pump(0.5)
            t.send("/view missing\r")
            t.shown("THAT VIEW IS UNKNOWN")
            # Valid JSON of the wrong type is an editor error; the terminal stays usable.
            original_editor = editor.read_text()
            editor.write_text("import sys\nopen(sys.argv[1],'w').write('[]')\n")
            t.send("/views\r")
            t.shown("VIEWS WERE NOT SAVED")
            editor.write_text(original_editor)
            # A concurrent save while $EDITOR is open refuses our stale save and keeps the file.
            ready, resume = f.cwd / "editor-ready", f.cwd / "editor-resume"
            editor.write_text(editor.read_text() +
                f"from pathlib import Path\nimport time\nPath({str(ready)!r}).write_text(p)\n"
                f"while not Path({str(resume)!r}).exists(): time.sleep(0.05)\n")
            t.send("/views\r")
            deadline = time.monotonic() + 10
            while not ready.exists():
                assert time.monotonic() < deadline, "editor never opened"
                t.pump(0.05)
            saved = request(a, "PUT", "/views", request(a, "GET", "/views"))
            frames = t.frames
            resume.touch()
            t.until(lambda t: t.frames > frames and "VIEWS WERE NOT SAVED" in t.text())
            assert os.path.exists(ready.read_text()) and request(a, "GET", "/views") == saved
        def in_earlier(line, session=old):
            return session in [r["session"] for section in line["layouts"][0]["sections"]
                           if section["kind"] == "earlier" for r in section["sessions"]]
        with Terminal(beta, "--resume", old, columns=160, rows=50) as t:
            t.shown("Message the agent")
            t.send("/resume\r")
            t.shown("AGENTS ·")
            assert not in_earlier(layout(b, saved["version"], lambda line: not in_earlier(line)))
        assert in_earlier(layout(b, saved["version"], in_earlier))
        # Two changed publications on one connection stay at least one second apart.
        stream = status(a, "?views=true")
        next(l for l in map(json.loads, iter(stream.readline, b""))
             if l["kind"] == "views" and l["version"] == saved["version"])
        received = []
        for name in ("Changed once", "Changed twice"):
            saved["views"][0]["groups"][0]["name"] = name
            saved = request(a, "PUT", "/views", saved)
            next(l for l in map(json.loads, iter(stream.readline, b""))
                 if l["kind"] == "views" and l["version"] == saved["version"])
            received.append(time.monotonic())
        assert received[1] - received[0] >= 0.9, received
        stream.close()
        # An asking child under Earlier has one flat NEEDS YOU shortcut; navigation reaches Earlier.
        request(b, "PUT", f"/sessions/{old}/name", "old56")
        asker = request(b, "POST", "/sessions", {"cwd": str(loose.cwd)})["session"]
        request(b, "POST", f"/sessions/{asker}/input", {"text": "ASK_56"})
        request(b, "POST", "/mail", {"to": old, "id": [here, 1], "kind": "note", "from": "flow56", "project": "beta56", "text": "CHILD_56"})
        # old's reply to the mail, after the terminal read it, keeps it out of Earlier once idle, until
        # a stream reads it again. Saving the views after it idles forces a layout resolved after that.
        with status(b) as agents:
            rows = (r for l in map(json.loads, iter(agents.readline, b"")) for r in l.get("agents", [l]))
            next(r for r in rows if r.get("session") == old and r["state"] == "needs_you" and "spoke_at" in r)
        saved = request(a, "PUT", "/views", saved)
        assert not in_earlier(layout(b, saved["version"]))
        with status(b, path=f"/v1/sessions/{old}/events") as read:
            assert json.loads(read.readline())["kind"] == "attached"
        layout(b, saved["version"], in_earlier)
        resolved = layout(b, saved["version"], lambda l: any(r["session"] != asker for s in l["layouts"][0]["sections"] if s["kind"] == "needs_you" for r in s["sessions"]))
        sections = resolved["layouts"][0]["sections"]
        needed = next(s["sessions"] for s in sections if s["kind"] == "needs_you")
        assert len(needed) == 2 and all(r["depth"] == 0 and r["session"] != old for r in needed), sections
        assert any(r["session"] == old for s in sections if s["kind"] == "earlier" for r in s["sessions"]), sections
        count = len(sections) + len({r["session"] for s in sections if s["kind"] != "earlier" for r in s["sessions"]})
        with Terminal(beta, "--resume", asker, columns=160, rows=50) as t:
            t.shown("Choose 56")
            t.shown("of 2")
            t.send("\x1b[D")
            t.until(lambda t: "MCP form consent" in "\n".join(t.screen.display[-16:]))
            assert "AGENTS" not in t.text(), t.text()
            t.send("\x1bOQ")
            t.shown("AGENTS", "NEEDS YOU")
            t.send("\x1b[A" * count)
            for _ in range(count * 2):
                t.send("\x1b[B")
                if t.pump(0.15, lambda t: "❯ Earlier" in t.text()):
                    break
            else:
                raise AssertionError("Navigation never reached Earlier:\n" + t.text())
            t.send("\r")
            t.shown("old56")
        # /quit addresses the next open row; a closed root leaves for Earlier despite its unread delivery.
        with Terminal(beta, "--resume", unclaimed, columns=160, rows=50) as t:
            t.shown("loose-visible")
            t.send("\x1bOQ")
            t.shown("AGENTS")
            t.send("/quit\r")
            t.until(lambda t: "loose-visible" not in t.text() and "AGENTS" in t.text())
            layout(b, saved["version"], lambda l: in_earlier(l, unclaimed))
            assert any(d["session"] == unclaimed and not d["read"] for d in request(b, "GET", "/deliveries"))
            layout(a, saved["version"], lambda l: in_earlier(l, unclaimed + "@b"))
        request(a, "POST", "/sessions/ember56/input", {"text": "RESIDENT_56"})
        kid = next(e["session"] for e in until(f, "ember56", lambda es: any(e["kind"] == "child" for e in es)) if e["kind"] == "child")
        until(f, kid, lambda es: es[-1]["kind"] == "turn_end")
        assert any(d["session"] == kid and not d["read"] for d in request(a, "GET", "/deliveries"))
        time.sleep(1.1)  # Allow the coalesced layout publication after the helper goes idle.
        assert next(r for r in json.loads(status(a).readline())["agents"] if r["session"] == kid)["state"] == "needs_you"
        sections = layout(a, saved["version"])["layouts"][0]["sections"]
        resident_rows = next(s["sessions"] for s in sections if s["kind"] == "residents")
        assert resident_rows[0]["session"] == "ember56" and any(r["session"] == kid and r["depth"] == 1 for r in resident_rows), "idle resident helper with unread delivery disappeared"
        assert all(r["session"] != kid for s in sections if s["kind"] != "residents" for r in s["sessions"]), "resident helper escaped into a group"
        delivery = next(d for d in request(a, "GET", "/deliveries") if d["session"] == kid)
        request(a, "PUT", f"/sessions/{kid}/deliveries/{delivery['entry']}/archive")
        layout(a, saved["version"], lambda l: all(r["session"] != kid for s in l["layouts"][0]["sections"] for r in s["sessions"]))
        home = "ember56@a" in status(b).readline().decode()
        sections = layout(b, saved["version"])["layouts"][0]["sections"]
        assert home and all("ember56" not in json.dumps(s) for s in sections if s["kind"] != "residents"), "a resident's home is in the groups"
    finally:  # A's server the driver stops; B's here, however the flow ended
        stop(b)
