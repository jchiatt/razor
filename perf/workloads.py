"""The workloads Razor is measured on, each defined once for the gate's budget check
(scripts/perf.py) and the performance harness (perf/scenarios.py). Imported with flows/ on the
path; the replies built here are the Responses route's, the route the gate measures."""
from dataclasses import replace
import json
import time

from responses import call, response
import server

LINE = "The record keeps every entry once, and a request is rendered from what it holds."
# The long session: 63 turns of 159 lines (an input, a 100-line read, a 58-line answer),
# 10,017 lines and 442 entries.
TURNS, PAGE, ANSWER = 63, 100, 58
BUSY = "Run the long build."
SEQ = 14_888_896  # bytes of `seq 2000000`


def page_text(lines=PAGE, line=LINE):
    """The file the long session's turns read: `lines` numbered lines."""
    return "".join(f"{i}: {line}\n" for i in range(lines))


def answer_text(lines, line=LINE):
    """An answer of `lines` numbered lines."""
    return "\n".join(f"{i}. {line}" for i in range(lines))


def message(text, id):
    """A final answer's Responses item."""
    return {"type": "message", "id": f"msg_{id}", "role": "assistant", "phase": "final_answer",
            "content": [{"type": "output_text", "text": text}]}


def say(lines, id, match=None):
    """A reply that ends the turn with `lines` lines of text."""
    return replace(response([message(answer_text(lines), id)]), match=match)


def run(name, arguments, id, match=None):
    """A reply holding one call, whose call id is `call_<id>`."""
    return replace(response([call(name, json.dumps(arguments), f"call_{id}")]), match=match)


def turn(f, session, after, text):
    """Sends `text` to `session` as a person's input, with a client attached after entry `after`:
    the time just before it was sent, the time its turn_end reached the client, and that entry's
    id."""
    connection = server.Local(f.home / ".razor/server.sock")
    key = (f.home / ".razor/device.key").read_text().strip()
    connection.request("GET", f"/v1/sessions/{session}/events?after={after}",
                       headers={"Authorization": f"Bearer {key}"})
    stream = connection.getresponse()
    assert json.loads(stream.readline())["kind"] == "attached"
    sent = time.monotonic()
    entry = server.post(f, f"/sessions/{session}/input", {"text": text})["entry"]
    while line := stream.readline():
        end = json.loads(line)
        if end["kind"] == "turn_end" and entry in end["inputs"]:
            connection.close()
            return sent, time.monotonic(), end["id"]
    raise AssertionError("the session's stream ended before its turn did")


def history(f, session, replies, replies_of, turns=TURNS):
    """The long session's turns on `session`, each a person's line answered by `replies_of(t)`:
    one read of `page_text()` and an `answer_text(ANSWER)`, in the measured route's replies.
    Returns the last turn_end's id."""
    last = 1
    for t in range(turns):
        replies += replies_of(t)
        last = turn(f, session, last, f"Turn {t}: read the page, then answer.")[2]
    return last


def busy(f, replies, stop):
    """Another session working until the file `stop` exists: one `bash` call writing a 15 MB log,
    then a line every 10 ms. The loop sleeps: the load is Razor's own work for a busy session (the
    output file, the job-output tail refreshed every 250 ms, the job's events), not a CPU hog on a
    shared box. Returns the session once the log is written."""
    replies += [run("bash", {"command": f"seq 2000000; while [ ! -e {stop} ]; do echo tick; "
                                        "sleep 0.01; done", "wait_ms": 600_000}, "busy", BUSY),
                say(1, "busy", BUSY)]
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    server.post(f, f"/sessions/{session}/input", {"text": BUSY})
    log = f.home / ".razor/sessions" / session / "job-1"
    server.until(f, session, lambda _: log.exists() and log.stat().st_size > SEQ)
    return session
