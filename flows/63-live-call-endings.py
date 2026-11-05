"""Refusals after upgrade, unmetered loss, server stop, a phone streaming through closure, and a
model and effort chosen during a call."""
import asyncio
import json
import os
import signal
import socket
import time
from driver import flow
from live import phone, serve
from responses import PLAIN, call, done, response
import server


def voice(f, settings=None):
    response = server.send(f, "GET" if settings is None else "PUT", "/voice", settings)
    body = response.read()
    assert response.status == 200, (response.status, body)
    return json.loads(body)


# A partial settings update retains every omitted field, including the call budget. Invalid
# shapes and values cannot change the saved file.
with flow([]) as f:
    server.start(f)
    saved = voice(f, {"call_daily_dollars": 4.5, "call_voice": "chosen", "memos": True})
    changed = voice(f, {"voice": "another"})
    assert changed == {**saved, "voice": "another"}, (saved, changed)
    path = f.home / ".razor/voice.json"
    before = path.read_bytes()
    for body, status in (([], 422), ({"voice": None}, 400)):
        refused = server.send(f, "PUT", "/voice", body)
        assert refused.status == status, (refused.status, refused.read())
        assert path.read_bytes() == before and voice(f) == changed

said = lambda text: response([{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}])
with flow([done(), said("Checked."), said("Fast."), done(), response([call("bash", '{"command": "until [ -e go ]; do sleep .05; done"}')]), done()], model="sol", config=PLAIN + '\n[server]\naddress="127.0.0.1:0"\n[models.slow]\nbase_url="{standin}"\nkey=""') as f:
    seeded = f.run("--print=json", "Seed the call.")
    assert seeded.returncode == 0, seeded.stderr
    session = json.loads(seeded.stdout)["session"]
    config = f.home / ".razor/config.toml"
    async def refused(which, reason):
        seen = await phone(f, which)
        assert len(seen) == 1 and seen[0].get("type") == "closed" and seen[0].get("reason") == reason, ("refusal after upgrade", reason, seen)
    asyncio.run(refused(session, "key"))
    asyncio.run(refused("missing", "error"))
    (f.home / ".razor/key").write_text("flow-dummy-live-key")
    # A refused choice after a held one changes nothing; the hand-off carries the held one, a
    # handover, and the new model's rebuilt request ends with the person's words.
    script = [{"after": "session.input_audio.append", "type": "session.input_transcript.delta", "delta": "Check it.", "start_ms": 0, "end_ms": 900}, {"type": "session.delegation.created", "offset_ms": 900, "delegation": {"id": "c1", "target": "client"}}]
    async def choose(ws, event):
        if reply := {"ready": '{"type":"choose","model":"terra","effort":"high","handover":true}', "chose": '{"type":"choose","model":"nope"}', "refused": bytes(960), "working": event.get("on") and '{"type":"end"}'}.get(event.get("type")):
            await ws.send(reply)
    with serve(script) as endpoint:
        config.write_text(config.read_text() + f'\n[live]\nurl={json.dumps(endpoint)}\n')
        seen = [e for e in asyncio.run(phone(f, session, choose)) if e.get("type") in ("ready", "chose", "refused")]
    assert [e["type"] for e in seen] == ["ready", "chose", "refused"] and seen[0]["model"] == "gpt-6.1-sol" and seen[1]["model"] == "gpt-6.1-terra" and seen[1]["handover"], seen
    handed = [(e.get("model"), e.get("effort"), e.get("handover")) for e in server.until(f, session, lambda es: es[-2].get("text") == "Checked.") if e.get("voice_call")]
    assert handed == [("gpt-6.1-terra", "high", True)], handed
    asked = json.loads(sorted(f.requests.glob("[0-9]*.body"))[-1].read_text())
    parts = [p["text"] for m in asked["input"] if m.get("role") == "user" for p in m["content"]]
    assert asked["model"] == "gpt-6.1-terra" and parts[-1].endswith("Check it.") and "Check it." not in (rest := "".join(parts[:-1])) and "context was rebuilt" in rest, parts
    # A held choice the session no longer takes (Fast came after it) never costs the person's words:
    # they go without it, and the phone hears why.
    async def fallback(ws, event):
        if event.get("type") == "chose":
            server.post(f, f"/sessions/{session}/input", {"text": "Go fast.", "speed": "fast"})
            server.until(f, session, lambda es: es[-2].get("text") == "Fast.")
        if reply := {"ready": '{"type":"choose","model":"slow"}', "chose": bytes(960), "refused": '{"type":"end"}'}.get(event.get("type")):
            await ws.send(reply)
    with serve(script) as again:
        config.write_text(config.read_text().replace(endpoint, again))
        seen = [e for e in asyncio.run(phone(f, session, fallback)) if e.get("type") == "refused"]
    entries = server.until(f, session, lambda es: sum(e.get("text") == "flow done" for e in es) == 2)
    assert "unavailable" in seen[0]["sentence"] and [(e["text"], e.get("model")) for e in entries if e.get("voice_call")][-1] == ("Check it.", None), (seen, entries)
    # 63 inputs, the first naming a model and handover, join the running turn on its
    # current model, with the call's end not yet sent; each is read and answered once.
    async def flood(ws, event):
        if event.get("type") == "ready":
            server.post(f, f"/sessions/{session}/input", {"text": "Hold."})
            server.until(f, session, lambda es: any(e["kind"] == "job" for e in es))
            held.extend(server.post(f, f"/sessions/{session}/input", {"text": f"Held {i}.", **({"model": "sol", "handover": True} if i == 0 else {})})["entry"] for i in range(63))
            await ws.send('{"type":"end"}')
    with serve([]) as quiet:
        config.write_text(config.read_text().replace(again, quiet))
        held = []
        asyncio.run(phone(f, session, flood))
    (f.cwd / "go").touch()
    ends = [e for e in server.until(f, session, lambda es: set(held) <= {i for e in es if e["kind"] == "turn_end" for i in e["inputs"]}) if e["kind"] == "turn_end"]
    assert ends[-1]["end"] == "done" and set(held) <= set(ends[-1]["inputs"]), ends[-1]
    assert all(sum(e["inputs"].count(i) for e in ends) == 1 for i in held), ends
    current = json.loads(sorted(f.requests.glob("[0-9]*.body"))[-1].read_bytes())
    assert current["model"] == "gpt-6.1-terra" and all(json.dumps(current).count(f"Held {i}.") == 1 for i in range(63)), current
    with serve([], refuse=401) as url:
        config.write_text(config.read_text().replace(quiet, url))
        settings = voice(f)
        settings["call_daily_dollars"] = 0
        voice(f, settings)
        asyncio.run(refused(session, "cap"))
        settings["call_daily_dollars"] = 10
        voice(f, settings)
        asyncio.run(refused(session, "key"))
    # Hold a port without listening: connection refusal without a race to reuse the port.
    with socket.socket() as unavailable:
        unavailable.bind(("127.0.0.1", 0))
        lost = f"ws://127.0.0.1:{unavailable.getsockname()[1]}"
        config.write_text(config.read_text().replace(url, lost))
        asyncio.run(refused(session, "lost"))

    for mode in ("drop", "stop", "streaming"):
        script = ([{"after": "session.input_audio.append", "delay": .3, "drop": True}] if mode == "drop" else
                  [{"after": "session.input_audio.append", "type": "session.input_transcript.delta",
                    "delta": "Half a sentence", "start_ms": 0, "end_ms": 200}] if mode == "stop" else
                  [{"after": "session.input_audio.append", "type": "session.usage.updated", "usage": {"seconds": 30}}])
        with serve(script) as endpoint:
            config.write_text(config.read_text().replace(lost, endpoint))
            lost = endpoint
            settings["call_daily_dollars"] = .01 if mode == "streaming" else 10
            voice(f, settings)
            async def act(ws, event):
                if event.get("type") == "ready":
                    await ws.send(bytes(960))
                if mode == "stop" and event.get("type") == "caption":
                    os.kill(json.loads((f.home / ".razor/server.json").read_text())["pid"], signal.SIGTERM)
            before = list(f.record(session))
            pid = json.loads((f.home / ".razor/server.json").read_text())["pid"]
            opened = time.monotonic()
            seen = asyncio.run(phone(f, session, act, streaming=mode == "streaming"))
            elapsed = time.monotonic() - opened
            assert seen[0].get("type") == "ready" and seen[-1].get("type") == "closed" and seen[-1].get("reason") == ("cap" if mode == "streaming" else "lost"), (mode, "phone lost its ending", seen)
            added = [e for e in f.record(session) if e["id"] > before[-1]["id"] and e["kind"] == "voice_call"]
            assert added[0]["event"] == "started" and added[-1]["event"] == "ended" and added[-1]["reason"] == seen[-1]["reason"], (mode, "call ending not recorded", added)
            if mode == "drop":
                charged = voice(f)["call_used_today"] * 1200
                assert .25 <= charged <= elapsed + .1, ("unmetered call not charged its wall-clock length", charged, elapsed)
            if mode == "stop":
                assert [e["text"] for e in added if e["event"] == "line"] == ["Half a sentence"], "server stop lost the last utterance"
                server.gone(pid)
                assert f.run("devices").returncode == 0, "scratch server could not restart after call ended"

