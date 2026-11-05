"""A schedule runs on the model and effort it names, and only for its own turn; its result reaches
the person as a note through the notify command, or as an ask the status stream lists; a failed
model turn sends a note without a model tool call; a fresh run's thread is a root whose row names
its resident, listed only among its resident's sessions; a resident's projects (one per request,
renamed in place, never through a link) and its `sky` file, passed through on its row unless bad
(docs/design/residents.md)."""
import json
import tempfile
import time
import urllib.request
from pathlib import Path
from driver import REPLIES, Reply, flow
from server import post, send, start as start_server, stop, until, wait


def at(seconds):
    return time.strftime("%H:%M:%S", time.localtime(time.time() + seconds))


def find(value, key):
    """Every value of `key` anywhere in a JSON value."""
    if isinstance(value, dict):
        if key in value:
            yield value[key]
        value = list(value.values())
    if isinstance(value, list):
        for item in value:
            yield from find(item, key)


names = (("done", "Start the server."), ("resident-note", "Brief me."),
         ("done", "Brief me."), ("resident-ask", "Review."), ("done", "Review."), ("done", "Fresh run."))
with tempfile.TemporaryDirectory() as scratch:
    note = Path(scratch) / "note.txt"
    (Path(scratch) / "notify").write_text(f'#!/bin/sh\nprintf "%s" "$1" > "{note}"\n')
    (Path(scratch) / "notify").chmod(0o755)
    failure = Reply(b'{"error":{"code":"invalid_request_error","message":"scheduled model failure"}}', status=400, match="Fail.")
    with flow([Reply((REPLIES / f"{name}.sse").read_bytes(), match=marker)
               for name, marker in names] + [failure],
              config=f"notify = [{json.dumps(str(Path(scratch) / 'notify'))}]\n"
                     '[server]\naddress = "127.0.0.1:0"') as f:
        (f.cwd / "persona.md").write_text("You are Ember.")
        with (f.home / ".razor/config.toml").open("a") as config:
            config.write(f"\n[residents.ember]\ncwd = {json.dumps(str(f.cwd))}\n"
                         f"startup = [{json.dumps(str(f.cwd / 'persona.md'))}, \"razor\"]\n"
                         'sky = "sky.json"\n'
                         f'[residents.ember.schedules.brief]\nat = ["{at(5)}"]\ntext = "Brief me."\n'
                         'model = "sonnet"\neffort = "low"\ndeliver = "notify"\n'
                         f'[residents.ember.schedules.review]\nat = ["{at(12)}"]\ntext = "Review."\n'
                         'deliver = "ask"\n'
                         f'[residents.ember.schedules.broken]\nat = ["{at(20)}"]\ntext = "Fail."\n'
                         'model = "gpt-6.1-sol"\n'
                         '[residents.ember.schedules.fresh]\nat = []\ntext = "Fresh run."\nfresh = true\n')
        start = f.run("--print=json", "Start the server.")
        assert start.returncode == 0, start.stderr
        entries = until(f, "ember", lambda es: sum(e["kind"] == "turn_end" for e in es) >= 2, timeout=40)
        requests = [json.loads(p.read_bytes()) for p in sorted(f.requests.glob("[0-9]*.body"))]
        brief = next(r for r in requests if "Brief me." in json.dumps(r) and "Review." not in json.dumps(r))
        review = next(r for r in requests if "Review." in json.dumps(r))
        assert "sonnet" in brief["model"] and "opus" in review["model"], (brief["model"], review["model"])
        # Claude Code sends the effort level in the request body; the next turn's is the session's.
        assert brief["output_config"]["effort"] == "low" != review["output_config"]["effort"], "effort"
        inputs = [e for e in entries if e["kind"] == "input"]
        assert [e["from"]["schedule"]["name"] for e in inputs] == ["brief", "review"], inputs
        assert inputs[0]["model"] == "sonnet" and inputs[0]["effort"] == "low", inputs[0]
        assert "type `note`, `to` `parent`" in inputs[0]["text"], inputs[0]
        assert note.read_text() == "Brief: all quiet.", "the notify command did not get the note"
        ask = [e for e in entries if e["kind"] == "call"][-1]
        server = json.loads((f.home / ".razor/server.json").read_text())
        key = (f.home / ".razor/device.key").read_text().strip()
        status = urllib.request.Request(f"http://{server['address']}/v1/status",
                                        headers={"Authorization": f"Bearer {key}"})
        with urllib.request.urlopen(status, timeout=10) as stream:
            agents = json.loads(stream.readline())
        assert any(question["id"] == ["ember", ask["id"]]
                   for asks in find(agents, "asks") for question in asks), agents
        entries = until(f, "ember", lambda es: any(e["kind"] == "turn_end" and
                        isinstance(e["end"], dict) and "error" in e["end"] for e in es))
        failed = [e for e in entries if e["kind"] == "turn_end"][-1]
        assert "scheduled model failure" in failed["end"]["error"], failed
        expected = f"Resident ember's schedule broken failed: {failed['end']['error'].rstrip('.')}."
        wait(lambda: note.read_text() == expected, 5, expected)
        run = post(f, "/residents/ember/run/fresh", {})["entry"]
        thread = next(e for e in f.record("ember") if e["id"] == run)["session"]
        until(f, thread, lambda es: any(e["kind"] == "turn_end" for e in es))
        with urllib.request.urlopen(status, timeout=10) as stream:
            row = next(r for r in json.loads(stream.readline())["agents"] if r["session"] == thread)
        assert row.get("resident") == "ember" and row["parent"] is None, row
        ask = lambda method, path, body=None: ((a := send(f, method, path, body)).status, json.loads(a.read()))
        assert thread not in [s["session"] for s in ask("GET", "/sessions")[1]], "a fresh run is listed"
        assert [s["job"] for s in ask("GET", "/sessions?resident=ember")[1] if s["session"] == thread] == ["fresh"]
        # A project made twice by one request is one project; a rename keeps its folder; a link is none.
        made = ask("POST", "/residents/ember/projects", {"name": "Alpha", "request": "make-22"})
        assert made[0] == 201 and ask("POST", "/residents/ember/projects", {"name": "Alpha", "request": "make-22"}) == made
        assert ask("PATCH", "/residents/ember/projects/Alpha", {"name": "Beta"})[1]["id"] == "Alpha"
        (f.cwd / "projects/Link").symlink_to(f.home / "tmp", target_is_directory=True)
        assert ask("PATCH", "/residents/ember/projects/Link", {"name": "Gone"})[0] == 404 and not (f.home / "tmp/.razor").exists()
        assert [(p["id"], p["name"]) for p in ask("GET", "/residents/ember/projects")[1]] == [("Alpha", "Beta")]
        # The sky file reaches the row as its days and counts alone; one malformed or over 64 KiB
        # leaves the field out and the row in.
        listing = urllib.request.Request(f"http://{server['address']}/v1/residents", headers=status.headers)
        days = [{"day": "2026-08-27", "n": 2}, {"day": "2026-09-01", "n": 18}]
        for text, shown in [(json.dumps([{**day, "note": "private"} for day in days]), days),
                            ('[{"day": "2026-08-27"', None), (json.dumps(days * 2000), None)]:
            (f.cwd / "sky.json").write_text(text)
            with urllib.request.urlopen(listing, timeout=10) as answer:
                row = json.load(answer)[0]
            assert row["session"] == "ember" and row.get("sky") == shown, (text[:60], row.get("sky"))
        # Replay closed runs older than a day: a refreshed old name and a new weekly name
        # survive churn even when name order and first insertion disagree with run age.
        stop(f)
        config = f.home / ".razor/config.toml"
        config.write_text(config.read_text().split("\n[residents.ember]")[0] +
                          f'\n[residents.ember]\ncwd = {json.dumps(str(f.cwd))}\n' +
                          ''.join(f'[residents.ember.schedules.{name}]\nat = []\ntext = "Run."\n'
                                  for name in ("old65", "old64", "old01", "old00", "weekly")))
        now, day = int(time.time() * 1000), 86_400_000
        entries = [dict(next(f.record("ember")), at=now - 141 * day)]
        names = [f"old{i:02}" for i in reversed(range(66))] + ["old65", "weekly", "old64"]
        for i, name in enumerate(names):
            at_ms, entry = now - (140 - i * 2) * day, len(entries) + 1
            entries.append({"id": entry, "at": at_ms, "kind": "input", "text": name,
                            "from": {"schedule": {"name": name, "due": at_ms}}})
            entries.append({"id": entry + 1, "at": at_ms + 1, "kind": "withdrawn", "inputs": [entry]})
        (f.home / ".razor/sessions/ember/record.jsonl").write_text(''.join(json.dumps(e) + '\n' for e in entries))
        start_server(f)
        row = next(r for r in json.load(send(f, "GET", "/residents")) if r["session"] == "ember")
        for name, i in (("old65", 66), ("weekly", 67), ("old01", 64), ("old00", 65), ("old64", 68)):
            latest = row["schedules"][name]["latest"]
            assert latest and latest["at"] == now - (140 - i * 2) * day, (name, latest)
