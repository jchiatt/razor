"""A complete call, progress during real work, a correction GPT-Live never hands off in time, the
person's stop holding the voice, and the agent's bounded view of a long call."""
import asyncio
import json
import os
import re
import tempfile
from pathlib import Path
from driver import flow
from live import phone, serve
from responses import PLAIN, call, done, response
import server


release, hushes = Path(tempfile.mkdtemp()) / "release", []


async def end(ws, event):
    if event.get("type") == "ready":
        await ws.send(bytes(960))
    if event.get("type") == "hush":
        hushes.append(event)
        if len(hushes) == 2:  # The slow check ends during the second hold, so its reply comes held.
            release.touch()
    if event.get("type") == "usage":
        await ws.send('{"type":"end"}')


script = [
    {"after": "session.input_audio.append", "type": "session.input_transcript.delta",
     "delta": "Please ", "start_ms": 0, "end_ms": 300},
    {"type": "session.input_transcript.delta", "delta": "check the work.", "start_ms": 300, "end_ms": 900},
    {"type": "session.delegation.created", "offset_ms": 900,
     "delegation": {"id": "d1", "target": "client"}},
    # Once the voice acknowledges the request, "wait, stop" holds it: its audio and words go nowhere,
    # and a blank fragment is not the person speaking again.
    {"after": "session.commentary.append", "type": "session.input_transcript.delta",
     "delta": "Wait, stop.", "start_ms": 1000, "end_ms": 1500},
    {"type": "session.output_transcript.delta", "delta": "Okay, pausing.", "start_ms": 1500, "end_ms": 2000},
    {"delay": 3, "type": "session.input_transcript.delta", "delta": " ", "start_ms": 4000, "end_ms": 4100},
    # A correction lifts it and reaches the working agent at its pause; GPT-Live's late hand-off of
    # it adds nothing. Seven quiet seconds later the voice says the agent's step is still running,
    # and twenty more later, once, that it is still on it.
    {"delay": 3, "type": "session.input_transcript.delta", "delta": "Also check the logs.", "start_ms": 5000, "end_ms": 5500},
    {"delay": 3, "type": "session.delegation.created", "offset_ms": 5600, "delegation": {"id": "d2", "target": "client"}},
    # "Wait" holds the voice again while the reply comes; "go on" lifts it and the reply comes silently.
    {"after": "session.commentary.append"},
    {"after": "session.commentary.append", "type": "session.input_transcript.delta", "delta": "Wait.",
     "start_ms": 6000, "end_ms": 6300},
    {"delay": 5, "type": "session.input_transcript.delta", "delta": "Go on.", "start_ms": 7000, "end_ms": 7500},
    {"after": "session.thinking.append", "type": "session.output_transcript.delta", "delta": "The check ",
     "start_ms": 8000, "end_ms": 9000},
    {"type": "session.output_transcript.delta", "delta": "passed.", "start_ms": 7000, "end_ms": 8000},
]
slow = response([call("bash", json.dumps({"command": f"until [ -e {release} ]; do sleep .2; done", "wait_ms": 60000, "description": "Run the slow check"}))])
check = response([call("bash", json.dumps({"command": "true", "description": "Checking the result."}), "call_check")])
quick = response([call("bash", json.dumps({"command": "sleep 1"}), "call_quick")])
logs, config = (response([call("bash", json.dumps({"command": f"until [ -e {release}-{n} ]; do sleep .2; done",
    "wait_ms": 60000, "description": f"Read the {n}"}), f"call_{n}")]) for n in ("logs", "config"))
with flow([done(), slow, check, done(), done(), quick, done(), done(), logs, done(), config, done()], model="sol", config=PLAIN + '\n[server]\naddress="127.0.0.1:0"') as f:
    capture = f.cwd / "live"
    tone = f.cwd / "tone.pcm"
    tone.write_bytes(bytes(range(240)) * 8)
    script.insert(6, {"audio": str(tone)})  # While held: the phone hears the tone once, after "go on".
    script += [{"audio": str(tone)}, {"type": "session.usage.updated", "usage": {"seconds": 2}}]
    with serve(script, capture) as url:
        config = f.home / ".razor/config.toml"
        config.write_text(config.read_text() + f'\n[live]\nurl={json.dumps(url)}\n')
        (f.home / ".razor/key").write_text("flow-dummy-live-key")
        seeded = f.run("--print=json", "Seed the call.")
        assert seeded.returncode == 0, seeded.stderr
        session = json.loads(seeded.stdout)["session"]
        # A session started for a call runs on the call's model and effort, Razor's or the config's;
        # a model or effort the phone sends wins, and Razor tells the phone the call's.
        def start(**choice):
            first = next(f.record(server.post(f, "/sessions", {"cwd": str(f.cwd), "call": True, **choice})["session"]))
            return first["model"], first["effort"]
        assert start() == ("gpt-6.1-terra", "low"), "a call's own session is not on terra at low effort"
        assert start(model="luna", effort="high") == ("gpt-6-luna", "high"), "the phone's choice lost"
        config.write_text('call_model = "sol"\ncall_effort = "medium"\n' + config.read_text())
        assert start() == ("gpt-6.1-sol", "medium"), "the configured call model or effort did not apply"
        assert {"call_model": "sol", "call_effort": "medium"}.items() <= json.loads(server.send(f, "GET", "/models").read()).items(), "/models"
        seen = asyncio.run(phone(f, session, end))
        assert b"".join(e["audio"] for e in seen if "audio" in e) == tone.read_bytes(), "voice PCM changed, vanished or was heard while held"
        assert [e for e in seen if e.get("type") == "hush"] == [{"type": "hush"}] * 2 and not any(e.get("text") == "Okay, pausing." for e in seen), "stop did not hush the phone or hold the voice's words"
        assert seen[-1].get("type") == "closed" and seen[-1]["reason"] == "ended", ("end was not acknowledged", seen[-1])
        sent = list(map(json.loads, (capture / "commands.jsonl").read_text().splitlines()))
        assert sent[0]["session"]["delegation"] == {"type": "client", "acknowledgement_delay_ms": 800}, "acknowledgement not delayed"
        assert any(c["type"] == "session.input_audio.append" and c["audio"] == 960 for c in sent), "phone PCM never reached voice"
        silent = [c for c in sent if c["type"] == "session.thinking.append"]
        spoken = [c for c in sent if c["type"] == "session.commentary.append"]
        assert [c["content"].rsplit(": ", 1)[-1] for c in spoken] == ["Please check the work.", "Run the slow check", "Say in two or three words that you're still here and still on it; the line has been quiet."], "acknowledgement did not name the request, or the step or its reminder was not said once"
        assert [c["content"] for c in silent] == ["flow done"], "held reply spoken, or a step given to the voice"
        assert [c["delegation_id"] for c in spoken + silent] == ["d1", "d2", "d2", "d2"], "agent's progress did not reach the voice's latest delegation"
        entries = server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 2)
        facts = [e for e in entries if e["kind"] == "voice_call"]
        assert [e["event"] for e in facts] == ["started"] + ["line"] * 6 + ["ended"] and facts[-1]["reason"] == "ended", "call facts or utterance boundaries were lost"
        assert [(e["who"], e["text"]) for e in facts if e["event"] == "line"] == [("person", "Please check the work."), ("person", "Wait, stop."),
            ("person", "Also check the logs."), ("person", "Wait."),
            ("person", "Go on."), ("voice", "The check passed.")], "transcript deltas did not form whole utterances, or held words were recorded"
        handed = [e for e in entries if e.get("voice_call")]
        assert [(e["text"], e["voice_call"]) for e in handed] == [(facts[n]["text"], {"id": facts[0]["call"], "lines": [facts[n]["id"]]})
            for n in (1, 3)], "spoken request or correction was lost, duplicated, or detached from its record"
        assert json.loads(server.send(f, "GET", "/voice").read())["call_used_today"] == 2 / 1200, "usage update and final usage charged the call twice"

        # A second call has more utterances than the agent's view holds. The record keeps all of them.
        script[:] = [{"type": "session." + ("input" if n % 2 == 0 else "output") + "_transcript.delta",
                      "delta": f"line{n:03d}.", "start_ms": n * 30, "end_ms": (n + 1) * 30} for n in range(300)]
        script += [{"type": "session.usage.updated", "usage": {"seconds": 1}}]
        asyncio.run(phone(f, session, end))
        entries = list(f.record(session))
        long_call = [e for e in entries if e["kind"] == "voice_call" and e["call"] != facts[0]["call"]]
        assert [e["text"] for e in long_call if e["event"] == "line"] == [f"line{n:03d}." for n in range(300)], "long call lost record lines"
        server.post(f, f"/sessions/{session}/input", {"text": "Continue by message."})
        server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 3)
        users = [i for i in json.loads((f.requests / "005.body").read_text())["input"] if i.get("role") == "user"]
        view = "\n".join(p.get("text", "") for p in users[-1]["content"])
        shown = re.findall(r"line(\d{3})\.", view)
        omitted = re.findall(r"\b\d+\b", view)
        # The prior call's three lines after its turn are also unread: 303 lines, of which 47 no longer fit.
        assert shown == [f"{n:03d}" for n in range(44, 300)] and omitted == ["47"], "agent did not receive newest 256 lines and one accurate omission count"

        # Words the voice talked over and the agent's turn ended before their pause start the next turn.
        script[:] = [
            {"type": "session.input_transcript.delta", "delta": "Run the quick check.", "start_ms": 0, "end_ms": 500},
            {"type": "session.delegation.created", "offset_ms": 500, "delegation": {"id": "d3", "target": "client"}},
            {"after": "session.commentary.append", "type": "session.input_transcript.delta", "delta": "Then summarize it.",
             "start_ms": 600, "end_ms": 900},
            {"type": "session.output_transcript.delta", "delta": "Mm-hm.", "start_ms": 900, "end_ms": 1000},
            {"delay": 4, "type": "session.usage.updated", "usage": {"seconds": 1}}]
        asyncio.run(phone(f, session, end))
        entries = server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 5)
        asked = [e["text"] for e in entries if e.get("voice_call") and e["voice_call"]["id"] not in (facts[0]["call"], long_call[0]["call"])]
        assert asked == ["Run the quick check.", "Then summarize it."], ("words said as the turn ended were lost", asked)

        # The person talks on after the voice spoke over them: the server waits for their pause
        # without spinning, and the turn ending meanwhile sends their whole line once they pause. A
        # backchannel said as the next turn ends starts nothing, and neither do words begun after
        # GPT-Live's late hand-off of words already sent.
        script[:] = [
            {"type": "session.input_transcript.delta", "delta": "Look into the logs.", "start_ms": 0, "end_ms": 500},
            {"type": "session.delegation.created", "offset_ms": 500, "delegation": {"id": "d4", "target": "client"}},
            {"after": "session.commentary.append", "type": "session.input_transcript.delta", "delta": "And the",
             "start_ms": 600, "end_ms": 800},
            {"type": "session.output_transcript.delta", "delta": "Sure.", "start_ms": 800, "end_ms": 900},
            *({"delay": .5, "type": "session.input_transcript.delta", "delta": f" part{n}",
               "start_ms": 1000 + n * 500, "end_ms": 1400 + n * 500} for n in range(10)),
            {"delay": 4, "type": "session.input_transcript.delta", "delta": "Mm-hm.", "start_ms": 9000, "end_ms": 9200},
            {"delay": 3, "type": "session.delegation.created", "offset_ms": 9300, "delegation": {"id": "d5", "target": "client"}},
            {"delay": 1, "type": "session.input_transcript.delta", "delta": "Thanks, that's all for now.",
             "start_ms": 11000, "end_ms": 11500},
            {"delay": 3, "type": "session.usage.updated", "usage": {"seconds": 1}}]
        pid, cpu = json.loads((f.home / ".razor/server.json").read_text())["pid"], []

        async def talk(ws, event):
            await end(ws, event)
            if event.get("type") == "caption" and event["text"] in (" part0", " part8"):
                cpu.append(sum(map(int, Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[11:13])))
            for word, n in ((" part8", "logs"), ("Mm-hm.", "config")):
                if event.get("type") == "caption" and event["text"] == word:
                    Path(f"{release}-{n}").touch()
        asyncio.run(phone(f, session, talk))
        assert (cpu[1] - cpu[0]) / os.sysconf("SC_CLK_TCK") < .5, ("server spun while the person talked on", cpu)
        entries = server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 7)
        latest = [e["text"] for e in entries if e.get("voice_call")][-2:]
        assert latest == ["Look into the logs.", "And the part0 part1 part2 part3 part4 part5 part6 part7 part8 part9"], (
            "a line talked on past the turn's end was split, or a backchannel or words after a late hand-off were admitted", latest)
