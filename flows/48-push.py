"""Pushes to Apple's service, against a stand-in for it: `razor devices add`
prints a pairing line with the id `agents` carries, and for an IP address says what to set; a
question sends one signed alert without its words; once answered, the next pushes drop it from
the open list; a delivery, a top-level finish and a top-level failure send theirs, while a child
and a stopped turn send none; a token Apple calls gone and a removed device get nothing more; a
followed session's Live Activity updates, sends again after a restart and ends when the session
finishes; a repeated input and a repeated creation return the first answer, the input's across a
restart; each device's level and mutes filter what it gets, a root's note to the person pushes
`notify`, a finish goes at `agents` only to the device that sent its input while it has no stream
open, an alert's interruption level says whether the person is in the session, and a note too
long for a lock screen is refused."""
from pathlib import Path
from urllib.parse import parse_qsl, urlsplit
import json
import tempfile
import time
from apns import apple
from driver import flow
from responses import PLAIN, reply as turn
from standin import Reply
import server
from server import wait


def ask(marker):
    return [turn(marker, "message", type="question", text="SECRET48 which way?", options=["left", "right"]),
            turn(marker, f"asked {marker}")]


FAIL = Reply(b'{"error":{"code":"invalid_request_error","message":"flow failure 48"}}', status=400, match="FAIL48")
replies = [*ask("ASK48"), turn("ASK48", "deliver", line="DELIVERED48", url="https://example.com/48"),
           turn("ASK48", "delivered48"),
           turn("KIDS48", "agent", action="start", name="kid48", kind="researcher", brief="Report KID48."),
           turn("KIDS48", "started48"),
           turn("You are kid48,", "bash", command="until [ -e kid48 ]; do sleep 0.05; done"),
           turn("You are kid48,", "agent", action="finish", status="finished", report="KIDREPORT48"),
           turn("KIDREPORT48", "reported48"),
           turn("STOP48", "bash", command="until [ -e never48 ]; do sleep 0.05; done"),
           FAIL,
           *ask("GONE48"), turn("GONE48", "answered48"), turn("DONE48", "done48"),
           turn("FOLLOW48", "bash", command="until [ -e go48 ]; do sleep 0.05; done"),
           turn("FOLLOW48", "followed48"),
           turn("REPEAT48", "repeated48"),
           turn("NOTE48", "message", type="note", to="parent", text="NOTEWORDS48 is ready."),
           turn("NOTE48", "noted48"), turn("MUTE48", "message", type="note", text="Muted48."),
           turn("MUTE48", "muted48"), turn("LONG48", "message", type="note", text="x" * 301),
           turn("LONG48", "message", type="note", text="two\nlines"), turn("LONG48", "long48"),
           turn("REPLY48A", "replied48a"), turn("REPLY48B", "replied48b"),
           Reply(b'{"error":{"code":"invalid_request_error","message":"flow failure"}}', status=400,
                 match="STUCK48"), *ask("QUESTION48"), *ask("OFF48"), turn("ELSEWHERE48", "elsewhere48"),
           *ask("ANSWER48"), turn("ANSWER48", "answered48")]


def api(f, method, path, key, body=None):
    """The status and JSON answer of a request through the socket with `key`."""
    answer = server.send(f, method, path, body, key=key)
    return answer.status, json.loads(answer.read() or b"null")


def restart(f):
    server.stop(f)
    server.start(f)


with apple(Path(tempfile.mkdtemp(prefix="razor-apns-"))) as apns:
    config = PLAIN + f"""[server]
address = "localhost:0"
label = "flow · 48"
name = "desk"
realm = "work"
[push]
team = "FLOWTEAM48"
bundle = "com.example.razor"
endpoint = "{apns.url}"
[push.development]
key_id = "FLOWKEY48"
key = {json.dumps(str(apns.key))}
"""
    with flow(replies, model="gpt-6.1-sol", config=config) as f:
        state = f.home / ".razor"

        def alerts(token=None):
            return [r for r in apns.received if r["headers"]["apns-push-type"] == "alert"
                    and token in (None, r["path"].removeprefix("/3/device/"))]

        def activities(token):
            return [r for r in apns.received if r["headers"]["apns-push-type"] == "liveactivity"
                    and r["path"] == f"/3/device/{token}"]

        def razor(push):
            return push["body"]["razor"]

        def enroll(name):
            added = f.run("devices", "add", name)
            assert added.returncode == 0, added.stderr
            return added.stdout.splitlines()

        def session(text):
            made = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
            server.post(f, f"/sessions/{made}/input", {"text": text})
            return made

        def ended(made, n=1):
            return server.until(f, made, lambda es: sum(e["kind"] == "turn_end" for e in es) == n)

        # 1. The pairing line carries the id, label, address and key; `agents` the same server.
        printed = enroll("phone")
        phone, pairing = printed[1], printed[-1]
        line = urlsplit(pairing)
        fields = dict(parse_qsl(line.query))
        home = (state / "server.id").read_text()
        assert (line.scheme, line.netloc) == ("razor", "pair") and len(home) == 32, pairing
        assert fields == {"id": home, "label": "flow · 48", "address": "localhost:0", "key": phone}, pairing
        key = (state / "device.key").read_text().strip()
        agents = json.loads(server.send(f, "GET", "/status").readline())
        # The row's identity; fields other features add (transcribe, video, load) are theirs to check.
        expected = {"id": home, "label": "flow · 48", "name": "", "machine": "desk", "realm": "work", "silent": None}
        mine = lambda row: {k: row.get(k) for k in expected}
        assert agents["kind"] == "agents" and mine(agents["server"]) == expected, agents
        assert [mine(r) for r in agents["servers"]] == [expected], agents

        # 2. Only an enrolled device registers; a token for an environment with no key is kept
        # with a sentence. A question sends one alert, with no words of the question.
        token = {"token": "aa11", "environment": "development"}
        assert api(f, "PUT", "/devices/self/push", key, token)[0] == 400
        status, answer = api(f, "PUT", "/devices/self/push", phone, dict(token, environment="production"))
        assert status == 200 and "no [push] key for Production" in answer["unsent"], answer
        assert api(f, "PUT", "/devices/self/push", phone, token) == (200, {})
        everything = (200, {"level": "all", "muted": []})
        assert api(f, "PUT", "/devices/self/notifications", phone, {"level": "all"}) == everything
        asking = session("ASK48")
        question = next(e["id"] for e in ended(asking) if e["kind"] == "call")
        first = wait(alerts)[0]
        headers = first["headers"]
        assert first["path"] == "/3/device/aa11", first
        assert (headers["apns-push-type"], headers["apns-topic"], headers["apns-priority"]) == (
            "alert", "com.example.razor", "10"), headers
        assert razor(first) == {"server": home, "kind": "question", "session": asking, "entry": question,
                                "open": [[asking, question]], "more": False}, first
        aps = first["body"]["aps"]
        assert (aps["category"], aps["thread-id"], aps["mutable-content"]) == ("question", asking, 1), aps
        assert aps["alert"]["body"].endswith(" on flow · 48 has a question"), aps

        # 3. Answered, the next turn delivers and finishes: an alert for each, the question gone
        # from their open lists.
        server.post(f, f"/sessions/{asking}/asks/{question}", {"option": 0})
        ended(asking, 2)
        later = wait(lambda: len(alerts()) == 3 and alerts()[1:])
        assert sorted(razor(a)["kind"] for a in later) == ["delivery", "finished"], later
        assert all(razor(a)["open"] == [] and razor(a)["session"] == asking for a in later), later

        receipt = f"/sessions/{asking}/deliveries/{next(razor(a)['entry'] for a in later if razor(a)['kind'] == 'delivery')}"
        for method,field in (("PUT","read"),("DELETE","read"),("PUT","archive"),("DELETE","archive")):
            assert api(f,method,receipt+"/"+field,key)[0] == 200

        # 4. A child's report sends nothing for the child; a stopped turn nothing, and its job's
        # end notice opens no turn; a failed top-level turn one `stuck`, which arrives after them.
        parent = session("KIDS48")
        ended(parent)
        (f.cwd / "kid48").touch()
        ended(parent, 2)
        child = next(p.name for p in (state / "sessions").iterdir() if next(f.record(p.name)).get("child"))
        stopped = session("STOP48")
        server.until(f, stopped, lambda es: any(e["kind"] == "job" for e in es))
        server.post(f, f"/sessions/{stopped}/stop", {})
        assert [e["end"] for e in ended(stopped) if e["kind"] == "turn_end"] == ["stopped"]
        failed = session("FAIL48")
        why = ended(failed)[-1]["end"]["error"]
        stuck = wait(lambda: [a for a in alerts() if razor(a)["session"] == failed])
        assert [razor(a)["kind"] for a in stuck] == ["stuck"], stuck
        assert len([a for a in alerts() if razor(a)["session"] == asking]) == 3, alerts()
        row = server.agents(f)[failed]
        assert row["error"] == why and "interrupted" not in row, row
        sessions = [razor(a)["session"] for a in alerts()]
        assert child not in sessions and parent in sessions, sessions
        assert not [a for a in alerts() if razor(a)["session"] == stopped], alerts()

        # 5. A token Apple calls gone leaves its device; a removed device's new token gets
        # nothing. A second device's alerts mark when each event's pushes went.
        apns.gone.add("aa11")
        pad = enroll("pad")[1]
        assert api(f, "PUT", "/devices/self/push", pad, {"token": "ee55", "environment": "development"}) == (200, {})
        assert api(f, "PUT", "/devices/self/notifications", pad, {"level": "all"}) == everything
        before = len(alerts("aa11"))
        gone = session("GONE48")
        ended(gone)
        wait(lambda: alerts("ee55"))
        assert len(alerts("aa11")) == before + 1, alerts("aa11")
        wait(lambda: "push" not in json.loads((state / "devices.json").read_text())[0])
        assert api(f, "PUT", "/devices/self/push", phone, {"token": "bb22", "environment": "development"})[0] == 200
        assert f.run("devices", "remove", "phone").returncode == 0
        entry = next(e["id"] for e in f.record(gone) if e["kind"] == "call")
        server.post(f, f"/sessions/{gone}/asks/{entry}", {"option": 1})
        ended(gone, 2)
        wait(lambda: len(alerts("ee55")) == 2)
        assert len(alerts("aa11")) == before + 1 and not alerts("bb22"), apns.received

        # 6. A follow: a child or a missing session is refused; the follow sends again after a
        # restart, then updates no sooner than ten seconds after its last, and ends when its
        # session finishes; a waiting update is not sent once its device is removed. A followed
        # session whose turn had finished ends at the restart.
        follow = {"token": "ff66", "environment": "development"}
        assert api(f, "PUT", f"/sessions/{child}/follow", pad, follow)[0] == 400
        assert api(f, "PUT", "/sessions/20200101-000000-0000/follow", pad, follow)[0] == 404
        done = session("DONE48")
        ended(done)
        assert api(f, "PUT", f"/sessions/{done}/follow", pad, dict(follow, token="dd77")) == (200, {})
        followed = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        assert api(f, "PUT", f"/sessions/{followed}/follow", pad, follow) == (200, {})
        watch = enroll("watch")[1]
        assert api(f, "PUT", f"/sessions/{followed}/follow", watch, dict(follow, token="ee77")) == (200, {})
        restart(f)
        wait(lambda: [a["body"]["aps"]["event"] for a in activities("dd77")][-1:] == ["end"])
        resent = wait(lambda: activities("ff66"))[0]
        headers, aps = resent["headers"], resent["body"]["aps"]
        assert (headers["apns-push-type"], headers["apns-topic"]) == (
            "liveactivity", "com.example.razor.push-type.liveactivity"), headers
        assert (aps["event"], aps["content-state"]["state"]) == ("update", "needs_you"), aps
        assert aps["stale-date"] - aps["timestamp"] == 900, aps
        wait(lambda: activities("ee77"))
        at = time.monotonic()
        server.post(f, f"/sessions/{followed}/input", {"text": "FOLLOW48"})
        server.until(f, followed, lambda es: any(e["kind"] == "job" for e in es))
        assert f.run("devices", "remove", "watch").returncode == 0
        working = wait(lambda: activities("ff66")[1:])[0]
        assert time.monotonic() - at > 8 and working["headers"]["apns-priority"] == "5", working
        assert len(activities("ee77")) == 1, activities("ee77")
        assert working["body"]["aps"]["content-state"]["state"] == "working", working
        (f.cwd / "go48").touch()
        last = wait(lambda: activities("ff66")[2:])[0]
        assert last["body"]["aps"]["event"] == "end" and "dismissal-date" in last["body"]["aps"], last
        wait(lambda: not json.loads((state / "devices.json").read_text())[0].get("follows"))

        # 7. A repeated creation returns the session it made; a repeated input its entry, also
        # after a restart, and the record holds one input.
        made = {"cwd": str(f.cwd), "request": "create-48"}
        status, created = api(f, "POST", "/sessions", key, made)
        assert status == 200 and api(f, "POST", "/sessions", key, made) == (200, created), created
        repeat = created["session"]
        sent = {"text": "REPEAT48", "request": "input-48"}
        status, first = api(f, "POST", f"/sessions/{repeat}/input", key, sent)
        assert status == 200 and api(f, "POST", f"/sessions/{repeat}/input", key, sent) == (200, first)
        ended(repeat)
        restart(f)
        assert api(f, "POST", f"/sessions/{repeat}/input", key, sent) == (200, first)
        inputs = [e for e in f.record(repeat) if e["kind"] == "input"]
        assert [(e["id"], e["request"]) for e in inputs] == [(first["entry"], "input-48")], inputs
        assert api(f, "POST", f"/sessions/{repeat}/input", key, dict(sent, request="no spaces"))[0] == 400

        # 8. Three more devices, at `agents` (the default), `all` and `questions`.
        one, two, three = (enroll(name)[1] for name in ("one", "two", "three"))
        for device, token in ((one, "a1"), (two, "b2"), (three, "c3")):
            assert api(f, "PUT", "/devices/self/push", device, {"token": token, "environment": "development"})[0] == 200
        assert api(f, "GET", "/devices/self/notifications", one) == (200, {"level": "agents", "muted": []})
        assert api(f, "GET", "/devices/self/notifications", key)[0] == 400
        assert api(f, "PUT", "/devices/self/notifications", one)[0] == 415
        assert api(f, "PUT", "/devices/self/notifications", two, {"level": "all", "muted": [asking] * 257})[0] == 400
        assert api(f, "PUT", "/devices/self/notifications", two, {"level": "all"}) == everything
        assert api(f, "PUT", "/devices/self/notifications", three, {"level": "questions"})[0] == 200

        def kinds(token, made):
            return [razor(a)["kind"] for a in alerts(token) if razor(a)["session"] == made]

        def level(token, made):
            return [a["body"]["aps"]["interruption-level"] for a in alerts(token) if razor(a)["session"] == made]

        # A root's note pushes `notify` with no words, which replaces its turn's finish even at
        # `all`; a muted session sends nothing.
        noting = session("NOTE48")
        call = next(e["id"] for e in ended(noting) if e["kind"] == "call")
        note = wait(lambda: [a for a in alerts("a1") if razor(a)["session"] == noting])[0]
        assert razor(note)["kind"] == "notify" and razor(note)["entry"] == call, note
        aps = note["body"]["aps"]
        assert (aps["category"], aps["interruption-level"]) == ("notify", "active"), aps
        assert aps["alert"]["body"].endswith(" on flow · 48 sent a note"), aps
        muted = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        assert api(f, "PUT", "/devices/self/notifications", two, {"level": "all", "muted": [muted]})[0] == 200
        server.post(f, f"/sessions/{muted}/input", {"text": "MUTE48"})
        ended(muted)
        wait(lambda: kinds("a1", muted) == ["notify"])

        # A note to the person over 300 characters, or of two lines, is refused and sends nothing.
        long = session("LONG48")
        results = [e for e in ended(long) if e["kind"] == "result"]
        assert [r["outcome"] for r in results] == ["error"] * 2, results
        assert all("Shorten this note to one line of at most 300 characters." in r["output"] for r in results), results

        # At `agents` a finish goes to the device that sent the input, and not while that device
        # has the session's stream open; `all` gets it, active, since that device does not watch.
        replied = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        assert api(f, "POST", f"/sessions/{replied}/input", one, {"text": "REPLY48A"})[0] == 200
        assert next(e for e in ended(replied) if e["kind"] == "input")["device"] == "one"
        wait(lambda: kinds("a1", replied) == ["finished"] and kinds("b2", replied) == ["finished"])
        assert level("a1", replied) == ["active"], alerts("a1")
        watched = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        with server.send(f, "GET", f"/sessions/{watched}/events", key=one) as stream:
            assert json.loads(stream.readline())["kind"] == "attached"
            assert api(f, "POST", f"/sessions/{watched}/input", one, {"text": "REPLY48B"})[0] == 200
            ended(watched)
            wait(lambda: level("b2", watched) == ["active"])
        # Passive is the watching device's alone: `two` watching leaves `one`'s finish active.
        elsewhere = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        with server.send(f, "GET", f"/sessions/{elsewhere}/events", key=two) as stream:
            assert json.loads(stream.readline())["kind"] == "attached"
            assert api(f, "POST", f"/sessions/{elsewhere}/input", one, {"text": "ELSEWHERE48"})[0] == 200
            ended(elsewhere)
            wait(lambda: level("a1", elsewhere) == ["active"] and level("b2", elsewhere) == ["passive"])
        # An answer from `one` is its input, so the answered turn's finish reaches it.
        answering = session("ANSWER48")
        question = next(e["id"] for e in ended(answering) if e["kind"] == "call")
        assert api(f, "POST", f"/sessions/{answering}/asks/{question}", one, {"option": 0})[0] == 200
        ended(answering, 2)
        wait(lambda: kinds("a1", answering) == ["question", "finished"])

        # `questions` gets questions and stuck sessions, time-sensitive for a question; `off`
        # nothing. Each later alert to `one` marks that the earlier ones had gone.
        ended(stuck := session("STUCK48"))
        wait(lambda: kinds("c3", stuck) == ["stuck"] and kinds("a1", stuck) == ["stuck"])
        ended(asked := session("QUESTION48"))
        wait(lambda: kinds("c3", asked) == ["question"])
        assert level("c3", asked) == ["time-sensitive"], alerts("c3")
        assert api(f, "PUT", "/devices/self/notifications", three, {"level": "off"})[0] == 200
        ended(off := session("OFF48"))
        wait(lambda: kinds("a1", off) == ["question"])
        assert kinds("b2", noting) == ["notify"] and kinds("b2", muted) == [], alerts("b2")
        assert kinds("a1", watched) == [] and kinds("a1", long) == [] != kinds("b2", long), alerts("a1")
        got = [razor(a)["session"] for a in alerts("c3")]
        assert got == [answering, stuck, asked], alerts("c3")
        assert not any(b"NOTEWORDS48" in r["raw"] for r in apns.received), "a note's words reached Apple"

        # 9. With server.address an IP address, enrolling says what to set instead of a line.
        config = (state / "config.toml").read_text().replace('"localhost:0"', '"10.9.8.7:7480"')
        (state / "config.toml").write_text(config)
        printed = "\n".join(enroll("tablet"))
        assert "razor://" not in printed and "set server.address to this computer's tailnet name" in printed, printed
        assert not any(b"SECRET48" in r["raw"] for r in apns.received), "a question's words reached Apple"
