"""Mail between roots on one server (docs/design/mail.md): `list` shows the other root by name and
project and not the caller; mail by name arrives under `<name> in <project>`, a reply by project
comes back, a name that is also two roots' project reaches the root of that name, a project two
roots share that is no session's name is refused with both named, and after a restart a new root
takes no name a root of this week holds."""
import json
from dataclasses import replace
from driver import flow
from model import agent, message, reply
import server
from server import until

a, b, c, g1, g2 = "FLOW_A43", "FLOW_B43", "FLOW_C43", "FLOW_G1_43", "FLOW_G2_43"
replies = [reply(a, "mcp__razor__status", {}), reply(a, text="Named."),
           message(b, "list"), reply(b, text="Listed.")]
with flow(replies) as f:
    alpha, beta = replace(f, cwd=f.cwd / "alpha43"), replace(f, cwd=f.cwd / "beta43")
    alpha.cwd.mkdir(), beta.cwd.mkdir()

    def run(where, *words):
        result = where.run("--print=json", *words)
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)["session"]

    def output(session, n=-1):
        entries = list(f.record(session))
        return json.loads([e for e in entries if e["kind"] == "result"][n]["output"])

    first = run(alpha, a)
    name = output(first)["agents"][0]["name"]
    second = run(beta, b)
    listed = output(second)
    assert listed["silent"] == [] and [r["to"] for r in listed["agents"]] == [first], listed
    assert (listed["agents"][0]["name"], listed["agents"][0]["project"]) == (name, "alpha43"), listed
    replies += [message(b, "note", to=name, text="GPU_43"), reply(b, text="Sent."),
                message("GPU_43", "note", to="beta43", text="ACK_43"), reply(a, text="Acked."),
                reply("ACK_43", text="Thanks.")]
    run(beta, "--resume", second, "Send it.")
    assert output(second) == {"id": [second, output(second)["id"][1]], "to": first}, output(second)
    sent = [e for e in f.record(second) if e["kind"] == "mail_sent"]
    assert [(e["to"], e["mail_kind"]) for e in sent] == [(first, "note")], sent

    def answered(text):
        def done(entries):
            ids = [e["id"] for e in entries if e["kind"] == "input" and e["text"] == text]
            return ids and any(e["kind"] == "turn_end" and ids[0] in e["inputs"] for e in entries)
        return done

    gpu = next(e for e in until(f, first, answered("GPU_43")) if e.get("text") == "GPU_43")
    ack = next(e for e in until(f, second, answered("ACK_43")) if e.get("text") == "ACK_43")
    other = gpu["from"]["mail"]["from"].removesuffix(" in beta43")
    assert other == "beta43" and other != name and gpu["from"]["mail"]["kind"] == "note", gpu
    assert ack["from"]["mail"]["from"] == f"{name} in alpha43", ack
    requests = [p.read_text() for p in sorted(f.requests.glob("*.body")) if p.stem.isdigit()]
    for words in (f"{other} in beta43: GPU_43", f"{name} in alpha43: ACK_43"):
        assert any(words in r for r in requests), f"no request held {words!r}"
    replies += [reply(c, text="Here.")]
    third = run(alpha, c)
    # alpha43 is the first root's name and both alpha roots' project: the name wins, where the
    # send was refused as ambiguous before names were stored.
    replies += [message(b, "note", to="alpha43", text="EXACT_43"), reply(b, text="Sent."),
                reply("EXACT_43", text="Got it.")]
    run(beta, "--resume", second, "Send to the name.")
    until(f, first, answered("EXACT_43"))
    assert not any(e.get("text") == "EXACT_43" for e in f.record(third)), "a project match beat a name"
    gamma = replace(f, cwd=f.cwd / "gamma.43")  # its roots are gamma-43 and gamma-43-2
    gamma.cwd.mkdir()
    replies += [reply(g1, text="Here."), reply(g2, text="Here.")]
    fourth, fifth = run(gamma, g1), run(gamma, g2)
    replies += [message(b, "note", to="gamma.43", text="AMBIGUOUS_43"), reply(b, text="Refused.")]
    run(beta, "--resume", second, "Send to the project.")
    refused = [e for e in f.record(second) if e["kind"] == "result"][-1]
    assert refused["outcome"] == "error" and "names 2 agents" in refused["output"], refused
    assert fourth in refused["output"] and fifth in refused["output"], refused
    assert not any(e.get("text") == "AMBIGUOUS_43" for s in (fourth, fifth) for e in f.record(s)), "a refused send was admitted"
    # A root the person closed reopens only for the person: mail to it is refused (Trey, 2026-11-01).
    server.post(f, f"/sessions/{third}/quit", {})
    closed = until(f, third, lambda es: es[-1].get("closed"))
    assert closed[-1]["close_reason"] == "person" and server.agents(f)[third]["close_reason"] == "person", closed[-1]
    replies += [message(b, "note", to=third, text="CLOSED_43"), reply(b, text="Refused.")]
    run(beta, "--resume", second, "Send to the closed root.")
    refused = [e for e in f.record(second) if e["kind"] == "result"][-1]
    assert refused["outcome"] == "error" and ("ended" in refused["output"] or "closed" in refused["output"]), refused
    assert not any(e.get("text") == "CLOSED_43" for e in f.record(third)), "mail reopened a closed root"
    server.stop(f)
    replies += [reply("RESTART_43", text="Here.")]
    start = next(f.record(run(alpha, "RESTART_43")))
    assert start["name"] == "alpha43-3", start

# Open messaging (Trey, 2026-11-01): a child sees past the 100 newest agents with a query, its
# own tree marked; it reaches a child of another tree by id; and a name its sibling shares with
# that child is refused, naming both.
r1, r2, mine_twin, other_twin, scout = "FLOW_R1_43", "FLOW_R2_43", "SIB_CHILD_43", "OTHER_CHILD_43", "You are scout43,"
hold, wait = ({"command": f"while [ ! -f {go} ]; do sleep 0.05; done"} for go in ("go43", "done43"))
replies = [agent(r1, "start", name="twin43", brief=mine_twin, placement="in_place"),
           agent(r1, "start", name="scout43", brief="Look around.", placement="in_place"),
           reply(r1, text="Started."), agent(r2, "start", name="twin43", brief=other_twin, placement="in_place"),
           reply(r2, text="Started."), reply(mine_twin, "Bash", wait), reply(other_twin, "Bash", wait),
           reply(scout, "Bash", hold)]
with flow(replies) as f:
    first, second = run(f, r1), run(f, r2)
    starts = {p.parent.name: next(f.record(p.parent.name)) for p in (f.home / ".razor/sessions").glob("*/record.jsonl")}
    kids = {(s["child"]["name"], s["child"]["parent"]): sid for sid, s in starts.items() if "child" in s}
    mine, theirs, me = kids["twin43", first], kids["twin43", second], kids["scout43", first]
    for sid in (mine, theirs):  # each busy until done43, so neither reports
        until(f, sid, lambda es: any(e["kind"] == "job" for e in es))
    for _ in range(101):  # newer than every child, so the newest 100 hold no child
        server.post(f, "/sessions", {"cwd": str(f.cwd)})
    replies += [message(scout, "list"), message(scout, "list", query="TWIN43"),
                message(scout, "note", to=mine, text="SIB_43"), message(scout, "note", to=theirs, text="TWIN_43"),
                message(scout, "note", to="twin43", text="BOTH_43"), reply(scout, text="Sent."),
                reply(mine_twin, text="Here."), reply(other_twin, text="Here."), reply("SIB_43", text="Got it."),
                reply("TWIN_43", text="Got it."), reply(r1, text="Heard."), reply(r2, text="Heard.")]
    (f.cwd / "go43").touch()
    until(f, me, lambda es: es[-1]["kind"] == "turn_end" and sum(e["kind"] == "result" for e in es) == 6)
    everyone, found, *sent = (output(me, n) for n in range(1, 5))
    refused = [e for e in f.record(me) if e["kind"] == "result"][5]["output"]
    assert len(everyone["agents"]) == 100 and everyone["more"] > 0 and theirs not in str(everyone), everyone
    assert {(r["to"], r["tree"]) for r in found["agents"]} == {(mine, True), (theirs, False)}, found
    assert [r["to"] for r in sent] == [mine, theirs], sent
    assert "names 2 agents" in refused and mine in refused and theirs in refused, refused
    (f.cwd / "done43").touch()
    for sid, text in ((mine, "SIB_43"), (theirs, "TWIN_43")):
        entries = until(f, sid, answered(text))
        end = next(e for e in entries if e["kind"] == "turn_end")
        assert next(e for e in entries if e.get("text") == text)["id"] not in end["inputs"], "mail joined a sibling's turn"
    assert any("scout43 in cwd: SIB_43" in p.read_text() for p in f.requests.glob("*.body")), "no request held the note"
    assert not any(e.get("text") == "BOTH_43" for s in (mine, theirs) for e in f.record(s)), "a refused send was admitted"
    for sid in (first, second):
        until(f, sid, lambda es: es[-1]["kind"] == "turn_end" and any("report" in e.get("from", "") for e in es))
