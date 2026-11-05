"""Mail does not reopen a finished child; continue reuses its session and branch. A child given
no model or effort runs on its parent's, not the config's defaults. A note sent to a child after
its finishing batch's last request stays unanswered, and its continuation reads it. Each request
of a twice-continued child starts with the one before, so its prompt cache is never written
again from the middle."""
import json
import claude
import server
from driver import flow
from model import agent, message, reply


root, child = "FLOW_ROOT_16", "You are oak16,"
replies = [agent(root, "start", name="oak16", brief="Commit the first marker."),
           agent(root, "wait", children=["oak16"]), reply(root, text="First report received."),
           # The child says what it is doing but never sets a headline.
           claude.reply((None, "Bash", {"command": "printf first > first16; git add -- first16 && git -c user.name=Flow -c user.email=flow@example.invalid commit -m 'First marker'"}),
                        text="\nCommitting the first marker.\nThen reporting.", match=child),
           agent(child, "finish", status="finished", report="First report.")]
with flow(replies, config='startup = ["razor", "~/parent.md"]\n'
                         '[children]\nstartup = ["child.md", "project"]') as f:
    (f.home / "parent.md").write_text("PARENT_STARTUP_16")
    (f.home / ".razor/child.md").write_text("CHILD_STARTUP_16")
    (f.cwd / "AGENTS.md").write_text("PROJECT_STARTUP_16")
    f.base("AGENTS.md")
    f.git("remote", "add", "origin", "git@example.invalid:acme/widget16.git")
    first = f.run("--print=json", "--model", "opus", "--speed", "fast", "--effort", "low", root)
    assert first.returncode == 0, first.stderr
    parent = json.loads(first.stdout)["session"]
    children = [p.parent.name for p in (f.home / ".razor/sessions").glob("*/record.jsonl")
                if list(f.record(p.parent.name))[0].get("child")]
    assert len(children) == 1, children
    sid = children[0]
    shown = (f.home / ".razor/sessions" / sid / "startup.md").read_text()
    assert "CHILD_STARTUP_16\n\nContents of " in shown and "/AGENTS.md:\n\nPROJECT_STARTUP_16" in shown and "PARENT_STARTUP_16" not in shown, \
        "the child did not use its independent startup list"
    named = [(e["model"], e["effort"]) for e in (next(f.record(parent)), next(f.record(sid)))]
    assert named == [("opus", "low")] * 2, f"the child did not inherit its parent's model and effort: {named}"
    assert [next(f.record(s))["speed"] for s in (parent, sid)] == ["fast", "standard"]
    assert server.listed(f, sid), "a helper with branch work left the agents list"
    before = list(f.record(sid))
    branch = f"child/{sid}"
    tip = f.git("rev-parse", branch).strip()
    replies += [message(root, "note", to="oak16", text="Record this without reopening."),
                reply(root, "mcp__razor__status", {}),
                agent(root, "continue", child="oak16", speed="fast", brief="CONTINUE_BRIEF_16: commit the second marker."),
                agent(root, "wait", children=["oak16"]),
                agent(root, "settle", child="oak16", decision="discard"), reply(root),
                reply(child, "Bash", {"command": "printf second > second16; git add -- second16 && git -c user.name=Flow -c user.email=flow@example.invalid commit -m 'Second marker'"}),
                agent(child, "finish", status="partial", report="Second report.")]
    second = f.run("--resume", parent, "--print=json", "Continue the child.")
    assert second.returncode == 0, second.stderr
    after = list(f.record(sid))
    assert server.agents(f)[sid]["speed"] == "fast"
    assert after[:len(before)] == before, "continue rewrote history"
    assert len([e for e in after if e["kind"] == "start"]) == 1, after
    assert f.git("rev-parse", branch).strip() != tip, "continue made no new commit"
    assert f.git("show", f"{branch}:first16") == "first", "continue lost the old branch"
    assert f.git("show", f"{branch}:second16") == "second", "continue did not reuse the branch"
    assert f.git("merge-base", "--is-ancestor", tip, branch) == "", "continue started from another commit"
    entries = list(f.record(parent))
    status = next(e for e in entries if e["kind"] == "call" and "status" in e["name"])
    output = json.loads(next(e["output"] for e in entries if e["kind"] == "result" and e["call"] == status["call"]))
    row = next(row for row in output["agents"] if row["session"] == sid)
    assert row["state"] == "done", "mail reopened a finished child without continue"
    rows = server.agents(f)
    assert rows[parent]["repo"] == rows[sid]["repo"] == "widget16", rows
    assert row["headline"] == "Committing the first marker.", row
    briefs = [e for e in after if e["kind"] == "input" and "CONTINUE_BRIEF_16" in e["text"]]
    assert len(briefs) == 1, briefs
    requests = [p.read_text() for p in sorted(f.requests.glob("*.body")) if p.stem.isdigit()]
    assert any(child in request and "CONTINUE_BRIEF_16" in request for request in requests), "the continuation input never reached the child"
    assert any(f"- Name: oak16 (session id {sid})" in request for request in requests), "the child's facts did not name it"
    reports = [e for e in entries if e["kind"] == "input" and isinstance(e["from"], dict) and "report" in e["from"]]
    assert [e["text"] for e in reports] == ["First report.", "Second report."], reports
    # A person's input to a settled child is refused with a sentence; nothing is recorded.
    answer = server.send(f, "POST", f"/sessions/{sid}/input", {"text": "PERSON_SETTLED_16"})
    assert answer.status == 400 and b"settled child" in answer.read(), answer.status
    assert "PERSON_SETTLED_16" not in json.dumps(list(f.record(sid)))

# The note joins at the Bash result, after the request that called finish.
root, child = "FLOW_ROOT_16B", "You are elm16,"
replies = []
with flow(replies) as f:
    f.base()
    go = f.cwd / "go16"
    replies += [agent(root, "start", name="elm16", speed="fast", brief="Wait, then finish."),
                message(root, "note", to="elm16", text="UNREAD_NOTE_16"),
                reply(root, "Bash", {"command": f"sleep 1; touch {go}"}),
                agent(root, "wait", children=["elm16"]), reply(root, text="Done."),
                claude.reply((None, "Bash", {"command": f"until [ -e {go} ]; do sleep 0.1; done"}),
                             (None, "mcp__razor__agent", {"action": "finish", "status": "finished", "report": "Finished."}),
                             match=child)]
    run = f.run("--print=json", root)
    assert run.returncode == 0, run.stderr
    child_sid = next(p.parent.name for p in (f.home / ".razor/sessions").glob("*/record.jsonl") if list(f.record(p.parent.name))[0].get("child"))
    assert not server.listed(f, child_sid), "a finished read-only helper stayed in the agents list"
    entries = list(f.record(child_sid))
    assert entries[0]["speed"] == "fast"
    note = next(e["id"] for e in entries if e["kind"] == "input" and "UNREAD_NOTE_16" in e["text"])
    answered = [i for e in entries if e["kind"] == "turn_end" for i in e["inputs"]]
    assert note not in answered, "the finish answered a note the child never read"
    parent = json.loads(run.stdout)["session"]
    replies += [agent(root, "continue", child="elm16", brief="CONTINUE_16B"),
                agent(root, "wait", children=["elm16"]), reply(root, text="Again."),
                reply(child, "Bash", {"command": "true"}), agent(child, "finish", status="finished", report="Read it.")]
    again = f.run("--resume", parent, "--print=json", "Continue elm16.")
    assert again.returncode == 0, again.stderr
    requests = [p.read_text() for p in sorted(f.requests.glob("*.body")) if p.stem.isdigit()]
    assert any(child in r and "CONTINUE_16B" in r and "UNREAD_NOTE_16" in r for r in requests), "the continuation lost the unread note"
    answered = [i for e in f.record(child_sid) if e["kind"] == "turn_end" for i in e["inputs"]]
    assert note in answered, "the continuation left the note unanswered"
    # With no origin remote, a session's repository is its main checkout's folder, a child's too.
    rows = server.agents(f)
    linked = f.cwd.parent / "linked16"
    f.git("worktree", "add", "-q", "-b", "linked16", str(linked))
    made = server.post(f, "/sessions", {"cwd": str(linked)})["session"]
    rows = server.agents(f)
    assert rows[parent]["repo"] == rows[child_sid]["repo"] == rows[made]["repo"] == f.cwd.name, rows
    # A person's input to the finished, unsettled child continues it, as the parent's continue does.
    replies += [agent(child, "finish", status="finished", report="Heard the person."), reply(root, text="Heard.")]
    server.post(f, f"/sessions/{child_sid}/input", {"text": "PERSON_16B"})
    server.until(f, child_sid, lambda es: sum(e["kind"] == "turn_end" for e in es) == 3)
    heard = server.until(f, parent, lambda es: any(e["kind"] == "text" and e["text"] == "Heard." for e in es))
    continued = [e for e in heard if e["kind"] == "child" and e.get("event") == "continued"]
    assert len(continued) == 2, continued
    # A cache mark moves to the newest blocks; the newest message may gain blocks at its end.
    bare = lambda v: {k: bare(x) for k, x in v.items() if k != "cache_control"} if isinstance(v, dict) \
        else [bare(x) for x in v] if isinstance(v, list) else v
    sent = [bare(json.loads(p.read_bytes())["messages"]) for p in sorted(f.requests.glob("*.body"))
            if p.stem.isdigit() and child.encode() in p.read_bytes()]
    assert len(sent) == 4 and all(b[:len(a) - 1] == a[:-1] for a, b in zip(sent, sent[1:])), \
        "a continuation changed the child's history before its newest message"

    server.post(f, f"/sessions/{parent}/quit", {})
    closed = server.until(f, parent, lambda es: any(e["kind"] == "child" and isinstance(e["event"], dict) and "settled" in e["event"] for e in es))
    assert any(e["kind"] == "child" and e["session"] == child_sid and e.get("event") == {"settled": "discard"} for e in closed), "close did not discard the read-only helper"
