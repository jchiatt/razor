"""Thoughts stream as live lines on every route: numbered on through the session, `start` on each
one's first words, those words together its Thinking entry's text; never recorded or replayed; a
redacted thought sends nothing, and a dropped line never moves words to another thought's number."""
from dataclasses import replace
import json

import claude
from driver import flow
from responses import PLAIN, call, done, message, response, retried, thought
import server
from standin import Reply


def turn(f, *argv):
    """The lines `--print stream` printed for one turn, and its session."""
    result = f.run("--print=stream", *argv, timeout=90)
    assert result.returncode == 0, result.stderr
    lines = [json.loads(line) for line in result.stdout.splitlines()]
    return lines, lines[0]["session"]


def check(lines, n):
    """Thoughts number on from `n`, `start` on each first line alone, a void line drops every
    thought through its number, and each Thinking entry is the words of the oldest thought left;
    returns the last number and the order of thought (v for void), Thinking and reply lines."""
    begun = {}
    for line in lines:
        if line.get("void"):
            assert line == {"kind": "thought", "n": n, "void": True}, (line, n)
            begun.clear()
        elif line["kind"] == "thought":
            assert line.get("start", True) is True, line  # omitted when false
            if "start" in line:
                n += 1
                begun[n] = ""
            assert line["n"] == n and n in begun, (line, n)
            begun[n] += line["text"]
        elif line["kind"] == "thinking":
            assert begun and begun.pop(min(begun)) == line["text"], (begun, line)
    assert not begun, begun
    kinds = {"thought": "t", "thinking": "T", "delta": "d"}
    return n, "".join("v" if l.get("void") else kinds.get(l["kind"], "") for l in lines)


def unrecorded(f, session):
    """No thought is in the record, nor in a replay of the whole of it to a client attaching anew."""
    record = list(f.record(session))
    assert all(e["kind"] != "thought" for e in record)
    with server.send(f, "GET", f"/sessions/{session}/events?after=0") as stream:
        replayed = [json.loads(stream.readline()) for _ in range(len(record) + 1)]
    assert replayed[0]["kind"] == "attached" and [e["kind"] for e in replayed[1:]] == [e["kind"] for e in record], replayed


# Claude: a thought in pieces with a pause between them lands as its entry right after; a redacted
# block beside the next one sends nothing; the resumed session's thought is the third. A message
# cut by an error mid-thought is tried again as a new one, after a void line.
cut = claude.reply(text="claude-cut-73", thinking=["ABAN", "DONED"]).body.decode()
cut = cut[:cut.index("event: content_block_stop")] + \
    'event: error\ndata: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}\n\n'
replies = [claude.reply((None, "mcp__razor__status", {}), thinking=["First ", "thought, ", "in pieces."], gap=0.2),
           claude.reply(text="claude-73", thinking="Second thought.", redacted=True),
           claude.reply(text="claude-again-73", thinking=["Third ", "one."]),
           Reply(cut.encode()), claude.reply(text="claude-retried-73", thinking="COMPLETED")]
with flow(replies) as f:
    lines, session = turn(f, "Think, look, reply.")
    assert check(lines, 0) == (2, "tttTtTd"), lines
    lines, _ = turn(f, "--resume", session, "Again.")
    assert check(lines, 2) == (3, "ttTd"), lines
    lines, _ = turn(f, "--resume", session, "Once more.")
    assert check(lines, 3) == (5, "ttvtTd"), lines
    unrecorded(f, session)

# GPT: two reasoning items, the first in two summary parts, and the reply's words all stream
# before the response's entries; a later part begins with the line break its entry joins with.
streamed = response([thought(["Plan ", "the call."], ["Then ", "look."], id="rs_1"), message("Looking ", "now.", id="msg_1"),
                     thought(["Second ", "item."], id="rs_2"), call("status", "{}")], gap=0.2)
# A cut response tried again sends a void line between the cut attempt's thoughts and the retry's.
retry = retried([thought(["ABAN", "DONED"], id="rs_cut")], [thought(["COMPLETED"], id="rs_ok"), message("done")])
with flow([streamed, done(), *retry], model="gpt-6.1-sol", config=PLAIN) as f:
    lines, session = turn(f, "Think, look, reply.")
    assert check(lines, 0) == (2, "ttttddttTT"), lines
    lines, _ = turn(f, "--resume", session, "Again.")
    assert check(lines, 2) == (4, "ttvtdT"), lines
    unrecorded(f, session)

# A burst the display channel cannot hold drops lines, first ones among them: a thought's later
# words still carry its own number, so a client sees a new n without start and skips it.
burst = response([thought([f"{i}:{j} " for j in range(4)], id=f"rs_{i}") for i in range(1500)])
burst = replace(burst, cuts=burst.cuts[200::201])  # bursts of 201 lines 30 ms apart, each ending mid-thought
with flow([burst], model="gpt-6.1-sol", config=PLAIN) as f:
    lines, _ = turn(f, "Think a lot.")
    items, starts = {}, {l["n"] for l in lines if l.get("start")}
    for line in (l for l in lines if l["kind"] == "thought"):
        items.setdefault(line["n"], set()).add(line["text"].split(":")[0])
    assert all(len(i) == 1 for i in items.values()) and set(items) - starts, items


def chat(*deltas, finish="stop"):
    """A chat stream of `deltas`, then `finish`, which None leaves out so the stream is cut."""
    chunks = [{"choices": [{"index": 0, "delta": d}]} for d in deltas]
    chunks += [{"choices": [{"index": 0, "delta": {}, "finish_reason": finish}]}] * bool(finish)
    return Reply(("".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n").encode())


# A chat host's reasoning deltas are one thought per response, and a cut one is voided too.
replies = [chat({"reasoning_content": "Look "}, {"reasoning_content": "first."}, {"content": "glm-73"}),
           chat({"reasoning_content": "ABANDONED"}, finish=None), chat({"reasoning_content": "DONE"}, {"content": "glm"})]
GLM = '[models."glm-5.3-flash"]\nbase_url = "{standin}"\n'
with flow(replies, model="glm-5.3-flash", config=GLM, env={"RUNINFRA_API_KEY": "flow-key"}) as f:
    lines, session = turn(f, "Think and reply.")
    assert check(lines, 0) == (1, "ttdT"), lines
    assert check(turn(f, "--resume", session, "Again.")[0], 1) == (3, "tvtdT")
