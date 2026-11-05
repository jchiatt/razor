"""GPT early EOF and missing done item admit no response and execute no calls."""
import json

from driver import flow
from responses import PLAIN, call, response

items = [call("bash", json.dumps({"command": "printf first > first.txt"}), "call_first"),
         call("bash", json.dumps({"command": "printf second > second.txt"}), "call_second")]
for name, reply in (("early", response(items, completed=False)),
                    ("missing", response(items, missing=(1,)))):
    # Five stream retries after the first attempt; no response is admitted on any attempt.
    with flow([reply] * 6, model="gpt-6.1-sol", config=PLAIN) as f:
        result = f.run("--print=stream", f"Exercise {name}.", timeout=90)
        entries = [json.loads(line) for line in result.stdout.splitlines()]
        ends = [e for e in entries if e.get("kind") == "turn_end"]
        assert len(ends) == 1 and isinstance(ends[0]["end"], dict) and "error" in ends[0]["end"], entries
        assert not [e for e in entries if e.get("kind") in ("call", "result", "response")], entries
        assert not (f.cwd / "first.txt").exists() and not (f.cwd / "second.txt").exists()
        assert len(list(f.requests.glob("[0-9]*.body"))) == 6, list(f.requests.iterdir())
