"""A shared brief reaches a fresh child; its committed branch merges and disappears with its
worktree, whose uncommitted files move to the child's session folder, and a channel mention it held
when it finished ends with the settle, so nothing keeps the child live."""
import json
from driver import flow
from model import agent, message, reply

root, child = "FLOW_ROOT_11", "You are oak11,"
replies = [agent(root, "start", name="oak11", brief="Commit the child marker.", shared_brief="shared.md"),
           message(root, "note", to="channel", text="@oak11 HELD_NOTE_11"),
           agent(root, "wait", children=["oak11"]),
           agent(root, "settle", child="oak11", decision="merge"), reply(root),
           reply(child, "Bash", {"command": "sleep 2; printf child > child-marker; git add -- child-marker && git -c user.name=Flow -c user.email=flow@example.invalid commit -m 'Child marker'; mkdir __pycache__ && printf pyc > __pycache__/m.pyc && printf ' edited' >> shared.md"}),
           agent(child, "finish", status="finished", report="Committed child-marker.")]
with flow(replies) as f:
    (f.cwd / "shared.md").write_text("SHARED_BRIEF_11: keep this exact sentence.")
    f.base("shared.md")
    (f.cwd / "base").write_text("uncommitted parent edit")
    result = f.run("--print=json", root)
    assert result.returncode == 0, result.stderr
    parent = json.loads(result.stdout)["session"]
    records = [(p.parent.name, list(f.record(p.parent.name)))
               for p in (f.home / ".razor/sessions").glob("*/record.jsonl")]
    children = [(sid, entries) for sid, entries in records if entries[0].get("child")]
    assert len(children) == 1, children
    sid, entries = children[0]
    start = entries[0]
    assert start["child"]["parent"] == parent and start["child"]["name"] == "oak11", start
    first = next(e for e in entries if e["kind"] == "input")
    assert first["text"].index("SHARED_BRIEF_11") < first["text"].index("Commit the child marker."), first
    assert (f.cwd / "child-marker").read_text() == "child", "merge did not land the child commit"
    assert (f.cwd / "base").read_text() == "uncommitted parent edit", "merge overwrote the person's edit"
    assert f.git("show", "HEAD:base") == "base", "the child inherited the parent's dirty edit"
    assert not (f.home / ".razor/sessions" / sid / "worktree").exists(), "settle kept the worktree"
    kept = f.home / ".razor/sessions" / sid / "uncommitted"
    assert (kept / "__pycache__/m.pyc").read_text() == "pyc", "settle lost an untracked file"
    assert (kept / "shared.md").read_text().endswith(" edited"), "settle lost an uncommitted edit"
    settle = next(e for e in f.record(parent) if e["kind"] == "call" and e["input"].get("action") == "settle")
    said = json.loads(next(e["output"] for e in f.record(parent) if e["kind"] == "result" and e["call"] == settle["call"]))["text"]
    assert f"Moved into {kept}, at their paths: `shared.md`, `__pycache__/`." in said, said
    assert f"child/{sid}" not in f.git("branch", "--list", f"child/{sid}"), "merged branch survived"
    held = [e["id"] for e in entries if e["kind"] == "input" and "HELD_NOTE_11" in e["text"]]
    ended = [i for e in entries if e["kind"] == "turn_end" for i in e["inputs"]]
    withdrawn = [i for e in entries if e["kind"] == "withdrawn" for i in e["inputs"]]
    assert len(held) == 1 and held[0] in withdrawn and held[0] not in ended, (
        "the held note was not withdrawn at the settle", held, withdrawn, ended)
    assert not (f.home / ".razor/live" / sid).exists(), "a settled child stayed live"
    reports = [e for e in f.record(parent) if e["kind"] == "input" and isinstance(e["from"], dict) and "report" in e["from"]]
    assert len(reports) == 1 and reports[0]["text"] == "Committed child-marker.", reports
    requests = [p.read_text() for p in f.requests.glob("*.body") if p.stem.isdigit()]
    shown = [request for request in requests if root in request and "Committed child-marker." in request]
    assert shown, "the waiting parent never saw its child's report"
    assert all(request.count("Committed child-marker.") == 1 for request in shown), "wait and the input notice delivered the report twice"
