"""Installed service uses server.wrapper; residents start unprompted, skip busy heartbeats, and
hold busy schedules for the next turn (docs/design/residents.md)."""
import json
import plistlib
import subprocess
import os
import sys
import tempfile
import time
from pathlib import Path
from driver import BINARY, REPLIES, Reply, flow
from server import post, until


def at(seconds):
    return time.strftime("%H:%M:%S", time.localtime(time.time() + seconds))


names = ("done", "resident-sleep", "done", "done", "done")
with tempfile.TemporaryDirectory() as fake, flow(
        [Reply((REPLIES / f"{name}.sse").read_bytes()) for name in names],
        env={"PATH": f"{fake}:{os.environ['PATH']}"},
        config='[server]\nwrapper = ["/usr/bin/env", \'FLOW=<a&b"c%d$e\\f>\', "/x/.razor", '
               '"RAZOR_HOME=/x/.razor", "L=1\\n2"]') as f:
    # Service managers that accept every command, so no real one is ever touched on either OS.
    calls = Path(fake) / "calls"
    for manager in ("systemctl", "launchctl"):
        (Path(fake) / manager).write_text(f'#!/bin/sh\necho "{manager} $*" >> "{calls}"\nexit 0\n')
        (Path(fake) / manager).chmod(0o755)
    result = f.run("install")
    assert result.returncode == 0, result
    assert "daemon-reload" in calls.read_text() or "bootstrap" in calls.read_text(), calls.read_text()
    service = f.home / ("Library/LaunchAgents/razor.plist" if sys.platform == "darwin"
                        else ".config/systemd/user/razor.service")
    unit = service.read_text()
    if sys.platform == "darwin":
        command = plistlib.loads(unit.encode())["ProgramArguments"]
        wrapper = ["/usr/bin/env", 'FLOW=<a&b"c%d$e\\f>', "/x/.razor", "RAZOR_HOME=/x/.razor", "L=1\n2"]
        assert command[:5] == wrapper and command[-1] == "serve", unit
    else:
        command = next(line for line in unit.splitlines() if line.startswith("ExecStart="))
        assert command.startswith('ExecStart="/usr/bin/env" "FLOW=<a&b\\"c%%d$$e\\\\f>" "/x/.razor" '
                                  '"RAZOR_HOME=/x/.razor" "L=1\\n2" "'), unit
        assert command.endswith('" "serve"') and "Restart=on-failure" in unit, unit
    assert str(f.home / ".razor") in unit, unit
    # Another home's install is refused, though wrapper words name its folder and its RAZOR_HOME.
    taken = subprocess.run([BINARY, "install"], env={**f.env, "RAZOR_HOME": "/x/.razor"},
                           capture_output=True, text=True, timeout=30)
    assert taken.returncode == 1 and "another Razor home" in taken.stderr and service.read_text() == unit, taken
    service.unlink()  # Uninstalled: the manager here starts nothing, so the flow's own server must.
    (f.cwd / "persona.md").write_text("You are Ember, this flow's resident.")
    with (f.home / ".razor/config.toml").open("a") as config:
        config.write(f"\n[residents.ember]\ncwd = {json.dumps(str(f.cwd))}\n"
                     f"startup = [{json.dumps(str(f.cwd / 'persona.md'))}, \"razor\"]\n"
                     f'[residents.ember.heartbeat]\nat = ["{at(5)}", "{at(7)}"]\ntext = "Heartbeat."\n'
                     f'[residents.ember.schedules.mid]\nat = ["{at(8)}"]\ntext = "Mid-turn schedule."\n')
    # Another session's input starts the server; the resident gets none from a client.
    start = f.run("--print=json", "Start the server.")
    assert start.returncode == 0, start.stderr
    entries = until(f, "ember", lambda es: sum(e["kind"] == "turn_end" for e in es) >= 2, timeout=40)
    ends = [e for e in entries if e["kind"] == "turn_end"]
    assert entries[0]["kind"] == "start" and entries[0]["cwd"] == str(f.cwd), entries[0]
    inputs = [e for e in entries if e["kind"] == "input"]
    assert len(inputs) == 2 and inputs[0]["from"] == "heartbeat", inputs
    schedule, call = inputs[1], next(e for e in entries if e["kind"] == "call")
    assert schedule["from"]["schedule"]["name"] == "mid", schedule
    assert call["id"] < schedule["id"] < ends[0]["id"], "the schedule was not admitted mid-turn"
    assert inputs[0]["id"] in ends[0]["inputs"] and schedule["id"] not in ends[0]["inputs"], ends
    assert schedule["id"] in ends[1]["inputs"], ends
    beat = json.loads((f.requests / "002.body").read_bytes())
    # The resident's own list: its persona first, then Razor's text; its identity in the first input.
    assert beat["system"][-1]["text"].startswith("You are Ember, this flow's resident.\n\nYou are working in Razor"), beat["system"]
    facts = beat["messages"][0]["content"][0]["text"]
    assert "- Name: ember (session id ember)\n" in facts and "## Your identity record" in facts, facts
    assert "Mid-turn schedule." not in (f.requests / "003.body").read_text()
    # The model reads each input under a line naming its sender; the record keeps the words alone.
    assert "(From your heartbeat, not from the person.)\\nHeartbeat." in (f.requests / "002.body").read_text()
    assert "(From your schedule mid, not from the person.)\\nMid-turn schedule." in (f.requests / "004.body").read_text()
    assert [i["text"] for i in inputs] == ["Heartbeat.", "Mid-turn schedule."], inputs
    wake = post(f, "/sessions/ember/input", {"text": "Daylight wake.", "from": "daylight"})["entry"]
    entries = until(f, "ember", lambda es: any(e["kind"] == "turn_end" and wake in e["inputs"] for e in es))
    assert next(e for e in entries if e["id"] == wake)["text"] == "Daylight wake.", entries
    label = "(From the program daylight, not from the person.)\\nDaylight wake."
    assert label in (f.requests / "005.body").read_text(), "the program's input has no label"
    assert (f.home / ".razor/sessions/ember/identity.toml").is_file()

# A busy night's rebuild waits for the complete current batch, then precedes the next call.
# A threshold reached by that batch consumes the planned night, so its tidy input is read once.
from dataclasses import replace
from responses import PLAIN, call, done, response
import server

for threshold in (False, True):
    batch = response([call("bash", '{"command":"until [ -e release ]; do sleep .05; done"}', "long"),
                      call("bash", '{"command":"true"}', "batch_end")])
    if threshold:
        batch = replace(batch, body=batch.body.replace(b'"input_tokens": 100', b'"input_tokens": 300000'))
    with flow([batch, response([call("bash", '{"command":"true"}', "next")]), done(), done()],
              model="sol", config=PLAIN + '[rebuild]\nthreshold = 0.5\n') as f:
        with (f.home / ".razor/config.toml").open("a") as config:
            config.write(f'\n[residents.ember]\ncwd={json.dumps(str(f.cwd))}\nnight="{at(9)}"\n'
                         f'[residents.ember.schedules.long]\nat=["{at(4)}"]\ntext="Work."\n')
        server.start(f)
        entries = until(f, "ember", lambda es: any(e["kind"] == "input" and e["from"] == {"program": {"name": "razor"}} for e in es))
        tidy = next(e["id"] for e in entries if e["kind"] == "input" and e["from"] == {"program": {"name": "razor"}})
        (f.cwd / "release").touch()
        entries = until(f, "ember", lambda es: any(e["kind"] == "turn_end" and tidy in e["inputs"] for e in es))
        rebuilds = [e for e in entries if e["kind"] == "rebuild"]
        assert len(rebuilds) == 1, rebuilds
        last = next(e["id"] for e in entries if e["kind"] == "result" and e["call"] == "batch_end")
        following = next(e["id"] for e in entries if e["kind"] == "call" and e["call"] == "next")
        assert last < rebuilds[0]["id"] < following, (last, rebuilds, following)
        assert sum(e.get("inputs", []).count(tidy) for e in entries if e["kind"] == "turn_end") == 1, entries
