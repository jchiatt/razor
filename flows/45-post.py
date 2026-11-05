"""Post, when it is installed (docs/design/mail.md), with the real `post` under a scratch mail root:
without `post` on PATH nothing is bound and nothing errors; a command whose words name post without
running it binds nothing; the first `post` command binds the session a participant and a later job
acts as it; Post mail opens a turn showing Post's preview; no watch outlives the server; after a
restart a mention opens a turn, channel traffic naming nobody opens none, and a note still opens
one. The mention waits for the restart because a watch sees a channel joined after it started only
at its first 30-second pass, and this session joins #team in the command that starts its watch."""
import json
import re
import shutil
import subprocess
import time
from contextlib import suppress
from pathlib import Path
from driver import flow
from model import reply
from server import start, until, wait
import server

assert shutil.which("post"), "flow 45 needs Post's `post` on PATH"
replies = [reply("NOPOST_45", "Bash", {"command": 'post chat team --join; echo "P=${POST_PARTICIPANT:-none}"'}),
           reply("NOPOST_45", text="No post."),
           reply("JOIN_45", "Bash", {"command": 'printf "%s\\n" post POST; echo "Q=${POST_PARTICIPANT:-none}"'}),
           reply("JOIN_45", "Bash", {"command": "post chat team --join"}),
           reply("JOIN_45", "Bash", {"command": 'echo "P=$POST_PARTICIPANT"'}),
           reply("JOIN_45", text="Joined.")]
with flow(replies) as f:
    full = f.env["PATH"]
    bare = ":".join(d for d in full.split(":") if not (Path(d) / "post").exists())
    f.env.update(PATH=bare, POST_MAIL_ROOT=str(f.home / "post"))
    sender = {**f.env, "PATH": full}

    def outputs(session):
        return "\n".join(str(e["output"]) for e in f.record(session) if e["kind"] == "result")

    def stop():
        server.stop(f)
        wait(lambda: not watches(), 2, "a post watch outlived the server")

    def watches():
        """Post watches whose environment holds this flow's HOME, however they were detached."""
        found, mark = [], f"HOME={f.home}".encode()
        for proc in Path("/proc").glob("[0-9]*"):
            with suppress(OSError):
                if mark in (proc / "environ").read_bytes().split(b"\0") and \
                        (proc / "cmdline").read_bytes().split(b"\0")[1:2] == [b"watch"]:
                    found.append(int(proc.name))
        return found

    def post(*words, who=None):
        env = {**sender, **({"POST_PARTICIPANT": who} if who else {})}
        done = subprocess.run(["post", *words], cwd=f.cwd, env=env, capture_output=True, text=True, timeout=30)
        assert done.returncode == 0, (words, done.stdout, done.stderr)
        return done.stdout

    def watching(participant):
        """Returns once a watch holds its presence lock, which it takes after its `--from now` scan."""
        wait(lambda: any(p["id"] == participant and p["live_watch"] for p in json.loads(post("who", "--json"))["participants"]),
             15, f"{participant} has no live watch")

    first = f.run("--print=json", "NOPOST_45")
    assert first.returncode == 0, first.stderr
    session = json.loads(first.stdout)["session"]
    assert "P=none" in outputs(session) and not (f.home / ".razor/post.json").exists(), outputs(session)
    stop()

    f.env["PATH"] = full
    second = f.run("--resume", session, "--print=json", "JOIN_45")
    assert second.returncode == 0, second.stderr
    participant = re.search(r"P=(razor-[0-9a-f]{8})", outputs(session))
    assert participant and "Q=none" in outputs(session), outputs(session)
    participant = participant[1]
    assert json.loads((f.home / ".razor/post.json").read_text())[0][0] == session
    other = json.loads(post("participant", "bind", "--harness", "flow", "--key", "flow-45", "--json"))["id"]
    post("chat", "team", "--join", who=other)
    watching(participant)

    def heard(marker):
        def done(entries):
            ids = [e["id"] for e in entries if e["kind"] == "input" and marker in e["text"]]
            return ids and any(e["kind"] == "turn_end" and ids[0] in e["inputs"] for e in entries)
        entries = until(f, session, done)
        return next(e for e in entries if e["kind"] == "input" and marker in e["text"])

    replies += [reply("POSTNOTE_45", text="Read."), reply("MENTION_45", text="Seen."),
                reply("AGAIN_45", text="Again.")]
    post("send", "--to", participant, "--body", "POSTNOTE_45 is the parser fixed?", who=other)
    note = heard("POSTNOTE_45")
    assert note["from"] == {"program": {"name": "post"}}, note
    assert re.fullmatch(rf"{other} on Post: POSTNOTE_45 is the parser fixed\? \(post read [0-9a-f-]+\)",
                        note["text"]), note["text"]

    stop()
    start(f)
    watching(participant)
    post("chat", "team", "--body", "CHATTER_45 for nobody", who=other)
    post("chat", "team", "--body", f"@{participant} MENTION_45 check the parser", who=other)
    mention = heard("MENTION_45")
    assert mention["text"] == (f"{other} on Post in #team: @{participant} MENTION_45 check the parser"
                               " (post chat team)"), mention["text"]
    assert not any("CHATTER_45" in e.get("text", "") for e in f.record(session)), "unaddressed chatter opened a turn"
    post("send", "--to", participant, "--body", "AGAIN_45 after the restart", who=other)
    heard("AGAIN_45")

    # A closed session hears nothing more: its watch stops, post.json forgets it, its participant
    # ends, and a ring is refused with one sentence; no watch comes back after a restart.
    server.post(f, f"/sessions/{session}/quit", {})
    until(f, session, lambda es: es[-1].get("closed"))
    wait(lambda: not watches() and not json.loads((f.home / ".razor/post.json").read_text()) and
         any(p["id"] == participant and p["state"] == "ended" for p in json.loads(post("who", "--json"))["participants"]),
         5, "a post watch, binding or participant outlived the quit")
    refused = server.send(f, "POST", f"/sessions/{session}/input", {"text": "RING_45", "from": "post"})
    assert refused.status != 200 and b"closed" in refused.read(), refused.status
    stop()
    start(f)
    time.sleep(1)
    assert not watches() and not any("RING_45" in e.get("text", "") for e in f.record(session))
