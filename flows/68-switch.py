"""Model switches on both routes: the switch map, priced without the
working tree; no choice keeping the conversation on the new model and effort; a chosen handover and
the other family each writing a fresh note while the cache is warm, then opening the new model
ahead of the words; and a note turn cut off by its model path's end or a restart doing the same."""
from dataclasses import replace
import json
import os
import shutil
import signal
from claude import reply as claude
from driver import flow
from responses import PLAIN, reply as gpt
import server

said = lambda route, words: claude(text=words) if route == "claude" else gpt(None, words)
note = lambda route, words: claude((None, "mcp__razor__handover", {"note": words})) if route == "claude" else gpt(None, "handover", note=words)
# A 60,000-token context, over the 50,000-token floor.
big = lambda r: replace(r, body=r.body.replace(b'"input_tokens": 1,', b'"input_tokens": 60000,').replace(b'"input_tokens": 100,', b'"input_tokens": 60000,'))


def texts(body):
    """The text of each user message part of a Messages or Responses request, in order."""
    parts = [p for m in body.get("messages", body.get("input")) if m.get("role") == "user"
             for p in ([{"text": m["content"]}] if isinstance(m["content"], str) else m["content"])]
    return [p["text"] for p in parts if "text" in p]


for route, other, start, near, far in (("claude", "gpt", "sonnet", "opus", "gpt-6.1-sol"),
                                       ("gpt", "claude", "gpt-6.1-terra", "gpt-6.1-sol", "opus")):
    replies = [said(route, "FIRST_DONE"), big(said(route, "GROWN")), big(said(route, "KEPT")), note(route, "NOTE_ONE"),
               big(said(route, "FRESH")), note(route, "NOTE_TWO"), said(other, "ACROSS")]
    with flow(replies, model=start, config=PLAIN) as f:
        # A git that marks each status it is asked for.
        (bin := f.home / "bin").mkdir()
        (bin / "git").write_text(f'#!/bin/sh\ncase "$*" in *status*) touch {f.home}/status;; esac\nexec {shutil.which("git")} "$@"\n')
        (bin / "git").chmod(0o755)
        f.env["PATH"] = f"{bin}:{f.env['PATH']}"
        bodies = lambda: [json.loads(p.read_bytes()) for p in sorted(f.requests.glob("[0-9]*.body"))]
        ended = lambda n: lambda es: sum(e["kind"] == "turn_end" for e in es) == n
        session = json.loads(f.run("--print=json", "First words.").stdout)["session"]
        switches = lambda: json.loads(server.send(f, "GET", f"/models?session={session}").read())["switch"]
        # Under the floor a switch keeps the conversation without asking; the other family hands over.
        seen = switches()
        assert not (f.home / "status").exists(), "pricing a switch read the working tree"
        assert seen[near] == {"keep": 1 if route == "claude" else 100, "handover": None, "reason": None}, seen
        assert seen[far]["keep"] is None and seen[far]["handover"] > 0 and far in seen[far]["reason"], seen
        server.post(f, f"/sessions/{session}/input", {"text": "Grow."})
        server.until(f, session, ended(2))
        assert (seen := switches()[near])["keep"] == 60000 and seen["handover"] > 0 and not seen["reason"], seen
        # No choice keeps it: the new model and effort over the earlier messages, in one conversation.
        server.post(f, f"/sessions/{session}/input", {"text": "Keep going.", "model": near, "effort": "low"})
        entries, kept = server.until(f, session, ended(3)), bodies()[-1]
        assert near in kept["model"] and "First words." in json.dumps(kept) and "GROWN" in json.dumps(kept), kept
        assert "low" in json.dumps(kept.get("output_config", kept.get("reasoning"))), kept
        conversations = {e["native"]["claude"]["conversation"] for e in entries if "claude" in (e.get("native") or {})}
        assert not [e for e in entries if e["kind"] == "rebuild"] and len(conversations) == (route == "claude"), entries
        # A chosen handover, then the other family: the old model's note, then the new model's request.
        for model, words, written, n in ((start, "Fresh start.", "NOTE_ONE", 5), (far, "Other family.", "NOTE_TWO", 7)):
            server.post(f, f"/sessions/{session}/input", {"text": words, "model": model, **({"handover": True} if model == start else {})})
            entries, (asked, opened) = server.until(f, session, ended(n)), bodies()[-2:]
            assert [(e["trigger"], e["note"]) for e in entries if e["kind"] == "rebuild"][-1] == ("change", written), entries
            assert "switching to another model" in texts(asked)[-1] and model not in asked["model"], texts(asked)
            assert model in opened["model"] and written in "".join(texts(opened)[:-1]) and texts(opened)[-1].endswith(words), texts(opened)
        assert next(e for e in entries if e.get("text") == "Fresh start.")["handover"] is True, entries

# A note turn cut off by its model path's end or by a restart: the switch still opens the new model
# behind its words, with the abandoned note request left out and all held words in its first request.
for cut in ("path", "restart"):
    wait = claude((None, "Bash", {"command": "until [ -e never ]; do sleep 0.05; done"}))
    with flow([big(said("claude", "GROWN")), wait, said("gpt", "OPENED")], model="sonnet", config=PLAIN) as f:
        session = json.loads(f.run("--print=json", "First words.").stdout)["session"]
        words = server.post(f, f"/sessions/{session}/input", {"text": "Cut words.", "model": "gpt-6.1-sol"})["entry"]
        server.until(f, session, lambda es: any(e["kind"] == "job" for e in es))
        later = server.post(f, f"/sessions/{session}/input", {"text": "Later words."})["entry"]
        if cut == "path":
            # The Claude Code CLI under the server's bridge exits.
            pid = json.loads((f.home / ".razor/server.json").read_text())["pid"]
            assert os.system(f"pkill -KILL -P $(pgrep -P {pid} node)") == 0, "no Claude Code under the bridge"
        else:
            server.stop(f, signal.SIGKILL)
            server.start(f)
        ends = [e["end"] for e in server.until(f, session, lambda es: any(later in e.get("inputs", []) for e in es)) if words in e.get("inputs", [])]
        bodies = [json.loads(p.read_bytes()) for p in sorted(f.requests.glob("[0-9]*.body"))]
        opened = bodies[-1]
        assert len(bodies) == 3 and ends == ["done"] and opened["model"] == "gpt-6.1-sol", (cut, ends, texts(opened))
        assert texts(opened)[-2:] == ["Cut words.", "Later words."], (cut, texts(opened))
        assert "switching to another model" not in json.dumps(opened), (cut, texts(opened))
        counted = [i for e in f.record(session) if e["kind"] == "turn_end" for i in e["inputs"]]
        assert counted.count(words) == counted.count(later) == 1, counted

# Effort takes effect without input, including at a busy Claude boundary after an in-place
# model switch; a later staged model waits for its input, and a repeated effort adds nothing.
with flow([claude(text="FIRST"), claude((None, "Bash", {"command": "IFS= read -r release < effort-gate"})),
           claude(text="CHANGED")], model="sonnet", config='effort="low"') as f:
    os.mkfifo(f.cwd / "effort-gate")
    server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    effort = f"/sessions/{session}/effort"
    change = server.post(f, effort, {"effort": "medium"})["entry"]
    before = list(f.record(session))
    assert before[-1]["id"] == change and before[-1]["kind"] == "effort" and before[-1]["effort"] == "medium", before
    assert not list(f.requests.glob("[0-9]*.body")) and not any(e["kind"] == "input" for e in before), before
    assert server.post(f, effort, {"effort": "medium"}) == {} and list(f.record(session)) == before, "repeated effort appended an entry"
    assert (first := f.run("--resume", session, "--print=json", "First words.")).returncode == 0, first.stderr
    with server.send(f, "GET", f"/sessions/{session}/events?after={list(f.record(session))[-1]['id']}") as events:
        active = server.post(f, f"/sessions/{session}/input", {"text": "Keep working.", "model": "opus"})["entry"]
        for line in iter(events.readline, b""):
            if json.loads(line)["kind"] == "call":
                break
        later = server.post(f, f"/sessions/{session}/input", {"text": "Switch later.", "model": "sonnet", "effort": "low"})["entry"]
        server.post(f, effort, {"effort": "xhigh"})
        assert server.post(f, effort, {"effort": "xhigh"}) == {}, "busy repeated effort appended an entry"
        with (f.cwd / "effort-gate").open("w") as gate:
            gate.write("go\n")
        for line in iter(events.readline, b""):
            end = json.loads(line)
            if end["kind"] == "turn_end" and later in end["inputs"]:
                break
    bodies = [json.loads(p.read_bytes()) for p in sorted(f.requests.glob("[0-9]*.body"))]
    assert len(bodies) == 3 and [b["output_config"]["effort"] for b in bodies] == ["medium", "medium", "xhigh"], bodies
    assert ["opus" in b["model"] for b in bodies] == [False, True, True], [b["model"] for b in bodies]
    assert any(m.get("output_config", {}).get("effort") == "xhigh" for m in bodies[2]["messages"] if m["role"] == "system"), bodies[2]
    entries = list(f.record(session))
    assert [e["effort"] for e in entries if e["kind"] == "effort"] == ["medium", "xhigh"], entries
    assert len([e for e in entries if e["kind"] == "input"]) == 3 and len([e for e in entries if e["kind"] == "turn_end"]) == 2, entries
    assert next(e for e in entries if e["kind"] == "turn_end" and active in e["inputs"])["end"] == "done", entries
    assert not any(e["kind"] == "rebuild" for e in entries), entries
    assert len({e["native"]["claude"]["conversation"] for e in entries if "claude" in (e.get("native") or {})}) == 1, entries

# A model choice joins the current turn, but its chosen handover waits for that model's
# next turn, even across a restart and a heartbeat on the old model in between.
import time
for restart, heartbeat in ((False, True), (True, False), (True, True)):
    at = time.strftime("%H:%M:%S", time.localtime(time.time() + 5))
    with flow([gpt(None, "agent", action="wait", timeout_ms=30000), said("gpt", "JOINED"),
               *([said("gpt", "HEARTBEAT_DONE")] if heartbeat else []), said("gpt", "SWITCHED")], model="sol", config=PLAIN) as f:
        with (f.home / ".razor/config.toml").open("a") as config:
            config.write(f'\n[residents.ember]\ncwd={json.dumps(str(f.cwd))}\nnight="23:59:59"\n'
                         f'[residents.ember.heartbeat]\nat={json.dumps([at] if heartbeat else [])}\ntext="HEARTBEAT_68"\nmodel="sol"\n')
        server.start(f)
        path = "/sessions/ember/input"
        first = server.post(f, path, {"text": "Wait."})["entry"]
        server.until(f, "ember", lambda es: any(e["kind"] == "call" for e in es))
        choice = server.post(f, path, {"text": "CHOICE_68", "model": "gpt-6-sol", "handover": True})["entry"]
        server.until(f, "ember", lambda es: any(e["kind"] == "turn_end" and choice in e["inputs"] for e in es))
        joined = json.loads((f.requests / "002.body").read_bytes())
        assert joined["model"] == "gpt-6.1-sol" and json.dumps(joined).count("CHOICE_68") == 1, joined
        if restart:
            server.stop(f, signal.SIGKILL)
            server.start(f)
        if heartbeat:
            server.until(f, "ember", lambda es: any(e.get("text") == "HEARTBEAT_DONE" for e in es))
        later = server.post(f, path, {"text": "NEXT_68"})["entry"]
        entries = server.until(f, "ember", lambda es: any(e["kind"] == "turn_end" and later in e["inputs"] for e in es))
        rebuilds = [e for e in entries if e["kind"] == "rebuild"]
        assert [e["trigger"] for e in rebuilds] == ["change"] and rebuilds[0]["id"] > later, entries
        bodies = [json.loads(p.read_bytes()) for p in sorted(f.requests.glob("[0-9]*.body"))]
        assert [b["model"] for b in bodies] == ["gpt-6.1-sol"] * (3 if heartbeat else 2) + ["gpt-6-sol"], bodies
        assert all(sum(e.get("inputs", []).count(i) for e in entries if e["kind"] == "turn_end") == 1 for i in (first, choice, later)), entries
