"""A reviewer gets its own worktree and branch and the person's reviewer text after its contract;
discard closes it, moves its uncommitted files to its session folder and removes its worktree,
not its committed branch."""
import json
from driver import flow
from model import agent, reply
from server import agents

root, child = "FLOW_ROOT_12", "You are oak12,"
replies = [agent(root, "start", name="oak12", kind="reviewer", brief="Commit the discard marker."),
           agent(root, "wait", children=["oak12"]),
           agent(root, "settle", child="oak12", decision="discard"), reply(root),
           reply(child, "Bash", {"command": "printf discarded > discard-marker; git add -- discard-marker && git -c user.name=Flow -c user.email=flow@example.invalid commit -m 'Discard marker'; mkdir __pycache__ && printf pyc > __pycache__/m.pyc && printf ' edited' >> base"}),
           agent(child, "finish", status="partial", report="Partial work committed.")]
with flow(replies, config='[children]\nreviewer = "reviewer-12.md"\n') as f:
    (f.home / ".razor/reviewer-12.md").write_text("REVIEWER_TEXT_12: review what the brief names.\n")
    f.base()
    before = f.git("rev-parse", "HEAD")
    result = f.run("--print=json", root)
    assert result.returncode == 0, result.stderr
    children = [p.parent.name for p in (f.home / ".razor/sessions").glob("*/record.jsonl")
                if list(f.record(p.parent.name))[0].get("child")]
    sid = children[0]
    first = next(e["text"] for e in f.record(sid) if e["kind"] == "input")
    assert first.index(child) < first.index("REVIEWER_TEXT_12") < first.index("Commit the discard marker."), first
    assert f.git("rev-parse", "HEAD") == before, "discard changed the parent's commit"
    assert f.git("show", f"child/{sid}:discard-marker") == "discarded", "discard deleted the committed branch"
    assert not (f.cwd / "discard-marker").exists(), "discard merged the child's file"
    assert not (f.home / ".razor/sessions" / sid / "worktree").exists(), "discard kept the worktree"
    kept = f.home / ".razor/sessions" / sid / "uncommitted"
    assert (kept / "__pycache__/m.pyc").read_text() == "pyc", "discard lost an untracked file"
    assert (kept / "base").read_text() == "base edited", "discard lost an uncommitted edit"
    parent = json.loads(result.stdout)["session"]
    entries = list(f.record(parent))
    settle = next(e for e in entries if e["kind"] == "call" and e["input"].get("action") == "settle")
    said = json.loads(next(e["output"] for e in entries if e["kind"] == "result" and e["call"] == settle["call"]))["text"]
    assert f"Moved into {kept}, at their paths: `base`, `__pycache__/`." in said, said
    reports = [e for e in entries if e["kind"] == "input" and isinstance(e["from"], dict) and "report" in e["from"]]
    assert len(reports) == 1 and reports[0]["from"]["report"]["status"] == "partial", reports
    rows = agents(f)
    assert rows[sid]["settled"] == "discard", rows

    # A name used only under a settled helper is free again; a settled child's own name is not.
    elm, ash = "You are elm12,", "You are ash12,"
    replies += [agent(root, "start", name="elm12", placement="in_place", brief="Start one."), agent(root, "wait"),
                agent(root, "settle", child="elm12", decision="discard"),
                *(agent(root, "start", name=n, placement="in_place", brief="Finish.") for n in ("ash12", "elm12")),
                reply(root, "mcp__razor__status", {}), agent(root, "wait"),
                agent(root, "settle", child="ash12", decision="discard"), reply(root),
                agent(elm, "start", name="ash12", placement="in_place", brief="Finish."), agent(elm, "wait"),
                agent(elm, "finish", status="finished", report="Elm done."),
                *(agent(ash, "finish", status="finished", report="Ash done.") for _ in "12")]
    assert f.run("--resume", parent, "--print=json", "Grow the tree.").returncode == 0
    entries = list(f.record(parent))
    outputs = {e["call"]: e for e in entries if e["kind"] == "result"}
    calls = [e for e in entries if e["kind"] == "call"]
    starts = [outputs[e["call"]] for e in calls if e["input"].get("action") == "start"]
    assert [s["outcome"] for s in starts[1:]] == ["success", "success", "error"], starts
    assert "`elm12` is taken in this tree" in starts[3]["output"], starts[3]
    board = json.loads(next(outputs[e["call"]]["output"] for e in calls if "status" in e["name"]))["agents"]
    assert sorted(a["session"] for a in board) == sorted([parent, json.loads(starts[2]["output"])["session"]]), board
