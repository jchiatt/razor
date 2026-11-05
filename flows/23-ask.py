"""A resident's question to the person returns once recorded as an open ask; the ask command's
answer reaches the resident's next request; an ask answered first through the API is not answered
again by its command (docs/design/residents.md)."""
import json
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from driver import REPLIES, Reply, flow
from server import until


def answered(entry):
    return isinstance(entry["from"], dict) and "answer" in entry["from"]


names = ("done", "resident-ask", "done", "resident-ask-second", "done", "done")
with tempfile.TemporaryDirectory() as scratch:
    command = Path(scratch) / "ask"
    command.write_text('#!/bin/sh\ncase "$1" in *second*) sleep 3;; *) sleep 1;; esac\n'
                       'echo "Yes, ship it."\n')
    command.chmod(0o755)
    with flow([Reply((REPLIES / f"{name}.sse").read_bytes()) for name in names],
              config=f"ask = [{json.dumps(str(command))}]\n"
                     '[server]\naddress = "127.0.0.1:0"') as f:
        (f.cwd / "persona.md").write_text("You are Ember.")
        with (f.home / ".razor/config.toml").open("a") as config:
            config.write(f"\n[residents.ember]\ncwd = {json.dumps(str(f.cwd))}\n"
                         f"startup = [{json.dumps(str(f.cwd / 'persona.md'))}, \"razor\"]\n")
        start = f.run("--print=json", "Start the server.")
        assert start.returncode == 0, start.stderr
        result = f.run("--resume", "ember", "--print=json", "Ask me about the review.")
        assert result.returncode == 0, result.stderr
        entries = until(f, "ember", lambda es: sum(e["kind"] == "turn_end" for e in es) >= 2)
        first = [e for e in entries if e["kind"] == "call"][0]
        answers = [e for e in entries if e["kind"] == "input" and answered(e)]
        assert answers[0]["from"]["answer"]["ask"] == first["id"], answers
        assert answers[0]["text"] == "Yes, ship it.", answers[0]
        turn = [e for e in entries if e["kind"] == "turn_end"][0]
        assert turn["id"] < answers[0]["id"], "the question waited for its answer"
        assert "Yes, ship it." in (f.requests / "004.body").read_text()
        second = [e for e in entries if e["kind"] == "call"][-1]
        server = json.loads((f.home / ".razor/server.json").read_text())
        key = (f.home / ".razor/device.key").read_text().strip()
        route = f"http://{server['address']}/v1/sessions/ember/asks/{second['id']}"

        def answer():
            body = json.dumps({"text": "From the client."}).encode()
            request = urllib.request.Request(route, data=body, method="POST", headers={
                "Authorization": f"Bearer {key}", "Content-Type": "application/json"})
            return urllib.request.urlopen(request, timeout=10).status

        assert answer() == 200
        try:
            answer()
            raise AssertionError("a second answer to one ask was admitted")
        except urllib.error.HTTPError as refused:
            assert refused.code == 400, refused.code
        time.sleep(4)  # The command's own answer to the second ask comes after 3 seconds.
        entries = until(f, "ember", lambda es: sum(e["kind"] == "turn_end" for e in es) >= 3)
        answers = [e for e in entries if e["kind"] == "input" and answered(e)]
        late = [e for e in answers if e["from"]["answer"]["ask"] == second["id"]]
        assert [e["text"] for e in late] == ["From the client."], late
