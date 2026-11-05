"""Person messages admitted while a rebuild is under way, one while the handover's batch is still
running and one after the Rebuild entry while the rebuilt context is being answered, reach the
fresh conversation after the rebuilt context, each once, in the order they were admitted.
The first rebuilt request already includes the words held before the rebuild. A person
message still held when a newer input was sent (mail a waiting call took during the handover's
batch) is left out of the rebuilt context and reaches the model once, after it.

Admission counts what the model has not read (a parent's endless turn once refused
input at 64). A running turn admits a person's input past 64 that the model has read; the 65th it
has not read is refused, so the Responses task never holds more than 64, and mail waiting for room
is admitted when a response reads them. In a Claude turn of 66 waits, each wait's Result answers
the mail it took, and the turn's end answers every person's input, past the SDK's first 64."""
import json
import threading

from claude import reply
from driver import REPLIES, Reply, flow
from responses import PLAIN, call, response
import responses
import server

done = (REPLIES / "done.sse").read_bytes()
note = "Carry on from the asks."
asks = ["ASK_ONE: the first ask.", "ASK_TWO: sent while the batch runs.", "ASK_THREE: sent after the rebuild."]
replies = [reply(("toolu_handover", "mcp__razor__handover", {"note": note}),
                 ("toolu_wait", "Bash", {"command": "sleep 2"})),
           Reply(done, cuts=tuple(range(20, len(done) - 1, 20))),  # the rebuilt turn, slowly
           Reply(done)]  # the held inputs share one native user message and one reply
with flow(replies) as f:
    server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]

    def send(text):
        return server.post(f, f"/sessions/{session}/input", {"text": text})["entry"]

    ids = [send(asks[0])]
    server.until(f, session, lambda es: any(e.get("call") == "toolu_handover" and e["kind"] == "result" for e in es))
    ids.append(send(asks[1]))
    server.until(f, session, lambda es: any(e["kind"] == "rebuild" for e in es))
    ids.append(send(asks[2]))
    entries = server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 3)
    rebuild = next(e for e in entries if e["kind"] == "rebuild")
    assert rebuild["trigger"] == "handover" and ids[1] < rebuild["id"] < ids[2], (ids, rebuild)
    ends = [e for e in entries if e["kind"] == "turn_end"]
    assert ends[0]["inputs"] == [] and ends[0]["usage"]["output"] == 20, ends
    assert ends[1]["inputs"] == [ids[0], ids[1], rebuild["id"]] and ids[2] < ends[1]["id"], (ids, ends)
    assert ends[2]["inputs"] == ids[2:], ends
    answered = [i for e in ends for i in e["inputs"] if i in ids]
    assert answered == ids and all(e["end"] == "done" for e in ends), ends
    assert len([e for e in entries if e["kind"] == "start"]) == 1, entries
    requests = [json.dumps(json.loads(p.read_bytes())["messages"])
                for p in sorted(f.requests.glob("[0-9]*.body"))]
    context = json.dumps(rebuild["context"][:200])[1:-1]
    for request in requests:
        assert (asks[1] not in request and asks[2] not in request) or context in request, request
    last = requests[-1]
    native = json.loads(sorted(f.requests.glob("[0-9]*.body"))[-1].read_bytes())["messages"]
    held = [m for m in native if m["role"] == "user" and asks[1] in json.dumps(m)]
    assert len(held) == 1, held
    first_fresh = json.loads(sorted(f.requests.glob("[0-9]*.body"))[1].read_bytes())["messages"]
    assert asks[1] in json.dumps([m for m in first_fresh if m["role"] == "user"][-1]) and asks[2] not in json.dumps(first_fresh), first_fresh
    assert [last.count(text) for text in asks] == [1, 1, 1], last
    positions = [last.index(context), last.index(asks[1]), last.index(asks[2])]
    assert positions == sorted(positions), positions

held, news = "ASK_HELD: sent while the batch waits.", "NEWS_34: mail the wait takes."
replies = [reply(("toolu_handover", "mcp__razor__handover", {"note": note}),
                 ("toolu_wait", "mcp__razor__agent", {"action": "wait", "timeout_ms": 30000})),
           Reply(done), Reply(done)]
with flow(replies) as f:
    server.start(f)
    session, sender = (server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"] for _ in "ab")
    server.post(f, f"/sessions/{session}/input", {"text": asks[0]})
    server.until(f, session, lambda es: any(e.get("call") == "toolu_handover" and e["kind"] == "result" for e in es))
    # The mail first, so the wait takes it: the person's input would end the wait with null.
    server.post(f, "/mail", {"to": session, "id": [sender, 1], "kind": "note", "from": "flow",
                             "project": "cwd", "text": news})
    server.until(f, session, lambda es: any(e.get("call") == "toolu_wait" and e["kind"] == "result" for e in es))
    kept = server.post(f, f"/sessions/{session}/input", {"text": held})["entry"]
    entries = server.until(f, session, lambda es: any(e["kind"] == "turn_end" and kept in e["inputs"] for e in es))
    rebuild = next(e for e in entries if e["kind"] == "rebuild")
    assert held not in rebuild["context"] and news in rebuild["context"], rebuild["context"]
    last = json.dumps(json.loads(sorted(f.requests.glob("[0-9]*.body"))[-1].read_bytes())["messages"])
    context = json.dumps(rebuild["context"][:200])[1:-1]
    assert last.count(held) == 1 and last.index(context) < last.index(held), last

gate = lambda name: call("bash", json.dumps({"command": f"while [ ! -e {name} ]; do sleep 0.02; done"}), name)
replies = [response([gate("one"), gate("two")]), response([gate("three")]), responses.done(), responses.done()]
with flow(replies, model="gpt-6.1-sol", config=PLAIN) as f:
    server.start(f)
    session, sender = (server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"] for _ in "ab")
    path = f"/sessions/{session}/input"
    ids = [server.post(f, path, {"text": "Start."})["entry"]]
    server.until(f, session, lambda es: any(e["kind"] == "job" for e in es))
    ids += [server.post(f, path, {"text": f"NOTE_{i}."})["entry"] for i in range(64)]
    (f.cwd / "one").touch()  # a call's end frees nothing: the task holds the 64 for its next request
    server.until(f, session, lambda es: any(e["kind"] == "result" and e["call"] == "one" for e in es))
    try:
        refused = server.post(f, path, {"text": "The 65th unread."})
    except AssertionError as error:
        refused = error
    assert "409" in str(refused), refused
    mail = []
    waiter = threading.Thread(target=lambda: mail.append(server.post(f, "/mail", {
        "to": session, "id": [sender, 1], "kind": "note", "from": "flow", "project": "cwd", "text": "MAIL_34"})))
    waiter.start()
    (f.cwd / "two").touch()  # the second request carries the 64; its response reads them
    waiter.join(10)
    assert mail, "mail waiting for room slept past the reading"
    after = server.post(f, path, {"text": "AFTER_64."})["entry"]  # the turn still runs, at call three
    (f.cwd / "three").touch()
    entries = server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 2)
    two = next(e for e in entries if e["kind"] == "result" and e["call"] == "two")
    assert mail[0]["entry"] > two["id"], (mail, two)
    ends = [(e["end"], e["inputs"]) for e in entries if e["kind"] == "turn_end"]
    # Mail from outside the tree waits for the turn's end; the person's input behind it passes it.
    assert ends == [("done", ids + [after]), ("done", [mail[0]["entry"]])], (ids, ends)
    second = sorted(f.requests.glob("[0-9]*.body"))[1].read_text()
    assert all(f"NOTE_{i}." in second for i in range(64)), "the second request missed a note"

wait = ("mcp__razor__agent", {"action": "wait", "timeout_ms": 30000})
replies = [reply((f"toolu_{i}", *wait)) for i in range(66)] + [Reply(done)]
with flow(replies) as f:
    server.start(f)
    session, sender = (server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"] for _ in "ab")
    path = f"/sessions/{session}/input"
    notes, mails = [server.post(f, path, {"text": "Start."})["entry"]], []
    for i in range(66):
        server.until(f, session, lambda es: any(e["kind"] == "call" and e["call"] == f"toolu_{i}" for e in es))
        # Each wait takes its mail. (The person's input ends a wait with null; flow 18 checks that.)
        mails.append(server.post(f, "/mail", {"to": session, "id": [sender, i + 1], "kind": "note",
                                              "from": "flow", "project": "cwd", "text": f"MAIL_{i}"})["entry"])
    entries = server.until(f, session, lambda es: any(e["kind"] == "turn_end" for e in es))
    assert (took := [e.get("inputs") for e in entries if e["kind"] == "result"]) == [[m] for m in mails], took
    end = next(e for e in entries if e["kind"] == "turn_end")
    assert set(notes) <= set(end["inputs"]) and end["end"] == "done", (notes, end)

# A code-mode cell's waits take at most 64 news the model has not read: the 65th wait is refused
# with a sentence, which throws in the cell, and the news after it waits in `pending`. Taken news
# never counts against admission, so the person's input is admitted while the cell holds the 64.
# The refusal goes uncaught, so the cell fails, and its failed Result still answers what it showed.
cell = ('// @exec: {"yield_time_ms": 60000}\nlet n = 0;\n'
        'try { while (n < 65) { text((await tools.agent({action: "wait", timeout_ms: 20000})).text); n++; } }'
        ' finally { text(`took ${n}`);\n'
        'await tools.exec_command({cmd: "while [ ! -e go ]; do sleep 0.05; done"}); }')
for model in ("opus", "gpt-6.1-sol"):
    run = dict(type="custom_tool_call", id="ct_cell", call_id="cell", name="exec", input=cell)
    replies = ([reply(("cell", "mcp__razor__exec", {"code": cell}))] + [reply(text="flow done")] * 2
               if model == "opus" else [response([run]), responses.done(), responses.done()])
    with flow(replies, model=model, config="[profiles.claude]\ncode_mode = true\n" if model == "opus" else "") as f:
        server.start(f)
        session, sender = (server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"] for _ in "ab")
        server.post(f, f"/sessions/{session}/input", {"text": "Pump the mail."})
        server.until(f, session, lambda es: any(e.get("call") == "cell" for e in es))
        mails = [server.post(f, "/mail", {"to": session, "id": [sender, i + 1], "kind": "note", "from": "flow",
                                          "project": "cwd", "text": f"PUMP_{i}."})["entry"] for i in range(65)]
        server.until(f, session, lambda es: any(e.get("call") == "cell:66" for e in es))
        person = server.post(f, f"/sessions/{session}/input", {"text": "PERSON_34."})["entry"]
        (f.cwd / "go").touch()
        entries = server.until(f, session, lambda es: mails[64] in sum((e.get("inputs", []) for e in es), []))
        result = next(e for e in entries if e["kind"] == "result" and e["call"] == "cell")
        assert "took 64" in (out := result["output"]) and "hold 64 news" in out and result["inputs"] == mails[:64], out
        assert any(e["kind"] == "turn_end" and person in e["inputs"] for e in entries), model
        last = sorted(f.requests.glob("[0-9]*.body"))[-1].read_text()
        assert all(last.count(f"PUMP_{i}.") == 1 for i in range(65)) and last.count("PERSON_34.") == 1, model
