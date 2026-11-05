"""Splitting UTF-8, JSON escapes and SSE framing never changes an admitted call."""
import json

from driver import Reply, flow
from responses import PLAIN, call, done, response

name = 'quoted " 🪶.txt'
raw = json.dumps({"path": name}, ensure_ascii=False)
reply = response([call("read", raw)])
feather = reply.body.index("🪶".encode())
escape = reply.body.index(b'\\\\\\"')
cuts = sorted({1, 6, feather + 1, feather + 2, feather + 3, escape + 1, escape + 2, len(reply.body) - 1})
seen = []
for cut in cuts:
    with flow([Reply(reply.body, (cut,)), done()], model="gpt-6.1-sol", config=PLAIN) as f:
        (f.cwd / name).write_text("payload 🪶\n")
        result = f.run("--print=json", "Read the named file.")
        assert result.returncode == 0, result.stderr
        answer = json.loads(result.stdout)
        entries = list(f.record(answer["session"]))
        calls = [e for e in entries if e["kind"] == "call"]
        assert len(calls) == 1, calls
        native = next(e["native"]["responses"]["items"] for e in entries if e["kind"] == "response")
        seen.append((calls[0]["name"], native[0]["arguments"]))
        outputs = [e for e in entries if e["kind"] == "result"]
        assert len(outputs) == 1 and outputs[0]["outcome"] == "success", outputs
        assert "payload 🪶" in outputs[0]["output"], outputs
assert len(seen) == len(cuts) and len(set(seen)) == 1, seen
assert seen[0] == ("read", raw), seen
