"""The person's input admitted while the model streams goes to the model path at the next call's
admission and stays unread until that batch's results: an `agent wait` in the batch answers null
at once, and the input reaches the next request (3 November: the wait hung 86 s)."""
from dataclasses import replace
import json
import time
import claude
from driver import flow
from responses import PLAIN, call, done, response
import server

wait = {"action": "wait", "timeout_ms": 20000}
routes = {"opus": ([claude.reply(("toolu_wait67", "mcp__razor__agent", wait)), claude.reply(text="Heard.")], ""),
          "gpt-6.1-sol": ([response([call("agent", json.dumps(wait), "toolu_wait67")]), done()], PLAIN)}
for model, ((first, heard), config) in routes.items():
    # About 3 s of streaming before the call, in which the person writes.
    with flow([replace(first, cuts=tuple(range(1, 100))), heard], model=model, config=config) as f:
        server.start(f)
        session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        path = f"/sessions/{session}"
        server.post(f, path + "/input", {"text": "Wait for news."})
        while not (f.requests / "001.json").exists():
            time.sleep(0.02)
        began, steer = time.monotonic(), server.post(f, path + "/input", {"text": "STEER67"})["entry"]
        entries = server.until(f, session, lambda es: es[-1]["kind"] == "turn_end")
        call_at, waited = (next(e for e in entries if e.get("call") == "toolu_wait67" and e["kind"] == k) for k in ("call", "result"))
        assert waited["output"] == "null" and time.monotonic() - began < 10, (model, waited)
        assert call_at["id"] > steer and steer in entries[-1]["inputs"], (model, entries)
        # The request carrying the wait's null, its last mention of the call, carries the input.
        body = (f.requests / "002.body").read_text()
        assert body.rindex("toolu_wait67") < body.find("STEER67"), model
