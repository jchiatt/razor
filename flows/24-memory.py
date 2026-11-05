"""Memory in Razor's own files: a write is searchable by whole words, a replacement or an expiry
retires the old file, a fact needs a description, a new session's first request holds the state
note and each ruling, and a helper's worktree shares the main checkout's memory. Tasks shared by two
homes through a git remote named or given by URL; a list of one task's steps."""
from dataclasses import replace
import json
import os
from pathlib import Path
import shutil
import subprocess
from claude import reply
from driver import flow
from server import post, stop, until

replies = []
with flow(replies) as f:
    f.base()
    session = None

    def use(tool, where=f, resume=None, **arguments):
        """A turn of `where`'s session `resume`, else a new one, calling `tool`: the session and its result."""
        replies.extend([reply((None, f"mcp__razor__{tool}", arguments)), reply(text="saved")])
        result = where.run("--print=json", *(["--resume", resume] if resume else []), "Keep RULING_FILES_24 in the project.")
        assert result.returncode == 0, result.stderr
        sid = json.loads(result.stdout)["session"]
        return sid, [e for e in where.record(sid) if e["kind"] == "result"][-1]

    def memory(**arguments):
        global session
        session, output = use("memory", resume=session, **arguments)
        assert output["outcome"] == "success", output
        return json.loads(output["output"])

    old = memory(action="write", kind="fact", text="Amber lamplight.", description="A light.")["id"]
    assert [h["id"] for h in memory(action="search", query="lamplight")["hits"]] == [old], "write was not searchable"
    assert not memory(action="search", query="lamp")["hits"], "search matched part of a word"
    new = memory(action="write", kind="fact", text="Indigo lantern.", description="A light.", replaces=old)["id"]
    retired = f.home / ".razor/memory/retired"
    assert use("memory", resume=session, action="write", kind="fact", text="Bare.")[1]["outcome"] == "error", "a fact needs a description"
    faded = memory(action="write", kind="fact", text="Faded ember.", description="Old.", expires="2020-01-01")["id"]
    assert not memory(action="search", query="faded")["hits"] and list(retired.rglob(f"{faded}.md")), "an expired memory stayed"
    assert not memory(action="search", query="lamplight")["hits"] and list(retired.rglob(f"{old}.md")), "replacement did not retire the old file"
    assert [h["id"] for h in memory(action="search", query="lantern")["hits"]] == [new], "replacement was not searchable"
    memory(action="write", kind="fact", replaces=new)
    assert not memory(action="search", query="lantern")["hits"] and list(retired.rglob(f"{new}.md")), "retirement did not move the file out of search"
    memory(action="write", kind="state", text="STATE_FILES_24 ready for a new session.", replaces="state")
    ruling = memory(action="write", kind="ruling", text="Keep the marker.")["id"]
    replies.append(reply(text="opened"))
    opened = f.run("--print=json", "Read project memory.")
    assert opened.returncode == 0, opened.stderr
    page = sorted(f.requests.glob("[0-9]*.body"))[-1].read_text()
    assert "STATE_FILES_24" in page and "RULING_FILES_24" in page and ruling in page, "new session lost state or ruling"
    replies.extend([reply((None, "mcp__razor__agent", {"action": "start", "name": "helper",
                         "brief": "Read the project's memory and answer."}), match="START_HELPER_24"),
                    reply(text="started", match="START_HELPER_24"),
                    reply(text="read", match="You are helper,"),
                    reply(text="received", match="START_HELPER_24")])
    started = f.run("--print=json", "START_HELPER_24")
    assert started.returncode == 0, started.stderr
    root = json.loads(started.stdout)["session"]
    call = next(e for e in f.record(root) if e["kind"] == "result")
    child = json.loads(call["output"])["session"]
    entries = until(f, child, lambda es: any(e["kind"] == "turn_end" for e in es))
    until(f, root, lambda es: sum(e["kind"] == "turn_end" for e in es) >= 2)
    assert entries[0]["cwd"] != str(f.cwd) and (Path(entries[0]["cwd"]) / ".git").is_file(), "helper was not in a git worktree"
    page = next(p.read_text() for p in f.requests.glob("*.body") if "You are helper," in p.read_text())
    assert "STATE_FILES_24" in page and ruling in page, "helper lost main checkout's memory"
    offered = {t["name"]: t["input_schema"] for t in json.loads(page)["tools"]}
    assert [offered[f"mcp__razor__{n}"]["properties"]["action"]["enum"] for n in ("memory", "task")] == [["search", "get"], ["list", "get", "write"]], "a helper cannot claim or comment"

    # Two homes share tasks through a bare repository's hidden ref: a claim on one is refused on
    # the other, a write while the remote is unreachable is refused, and closing releases a claim.
    remote = f.home / "tasks.git"
    there = lambda *a: subprocess.run(["git", "--git-dir", remote, *a], capture_output=True, text=True).stdout
    there("init", "--bare")
    f.git("remote", "add", "origin", "~/tasks.git")  # git's own home form, which it expands
    (f.cwd / ".razor").mkdir()
    (f.cwd / ".razor/config.local.toml").write_text('[tasks]\nremote = "origin"\n')
    b = replace(f, home=f.home / "b", env={**f.env, "RAZOR_HOME": str(f.home / "b/.razor")})
    shutil.copytree(f.home / ".razor", b.home / ".razor", ignore=lambda _, names: set(names) - {"config.toml", "auth.json"})
    mine, claimed = use("task", action="write", title="Ship it", claim=True)
    f.git("remote", "set-url", "origin", f"{remote}-gone")
    assert use("task", b, action="write", title="Offline")[1]["outcome"] == "error", "an offline write landed"
    f.git("remote", "set-url", "origin", "~/tasks.git")
    # A home whose first read failed has fetched nothing yet, so its page still points to the list.
    theirs, refused = use("task", b, action="write", id=1, claim=True)
    assert json.loads(claimed["output"])["claimed"] and refused["outcome"] == "error", (claimed, refused)
    assert "keeps its tasks on its git remote" in sorted(f.requests.glob("[0-9]*.body"))[-1].read_text(), "a page hid unfetched tasks"
    post(f, f"/sessions/{mine}/quit", {})
    until(f, mine, lambda _: "claim:" not in there("show", "refs/razor/tasks:1.md"))
    assert json.loads(use("task", b, theirs, action="write", id=1, claim=True)[1]["output"])["claimed"]
    use("task", b, theirs, action="write", id=1, close="Shipped.")
    assert "Shipped." in there("show", "refs/razor/tasks:closed/1.md") and not there("show", "refs/razor/tasks:2.md")
    assert there("for-each-ref", "--format=%(refname)") == "refs/razor/tasks\n", "pushed more than the tasks ref"
    # A remote named by its path, read from the checkout, keeps the tasks there when origin moves; a
    # list keeps one task's steps.
    (f.cwd / ".razor/config.local.toml").write_text(f'[tasks]\nremote = "{os.path.relpath(remote, f.cwd)}"\n')
    f.git("remote", "set-url", "origin", f"{remote}-gone")
    pinned = [use("task", b, theirs, action="write", title=t, parent=n)[1]["outcome"] for t, n in [("Step", 1), ("Other", 0)]]
    assert pinned == ["success"] * 2, f"a relative path as the remote did not take the tasks: {pinned}"
    listed = json.loads(use("task", b, theirs, action="list", parent=1)[1]["output"])["tasks"]
    assert [t["id"] for t in listed] == [2] and "Step" in there("show", "refs/razor/tasks:2.md"), listed
    # A claim made on a view fetched within 30 seconds, which the other home has since changed, is
    # refused once its push is; a push that cannot reach the remote refuses, nothing written.
    os.utime(next((f.home / ".razor/tasks").rglob("FETCH_HEAD")), (0, 0))
    use("task", action="list")
    assert json.loads(use("task", b, theirs, action="write", id=3, claim=True)[1]["output"])["claimed"]
    assert "is claimed by" in (late := use("task", action="write", id=3, claim=True)[1])["output"] and there("show", "refs/razor/tasks:3.md").count("claim:") == 1, late
    (f.cwd / ".razor/config.local.toml").write_text(f'[tasks]\nremote = "{remote}-gone"\n')
    gone = use("task", action="write", title="Unpushed")[1]
    assert "did not answer" in gone["output"] and not there("show", "refs/razor/tasks:4.md"), gone
    stop(b)

# A helper claims for itself, comments, releases and reclaims; settlement releases that claim
# and records its report. Forbidden fields are refused before any part of a write applies.
root, helper = "CHILD_TASKS_24", "You are writer24,"


def tool(marker, tool_name, **arguments):
    return reply((None, f"mcp__razor__{tool_name}", arguments), match=marker)


forbidden = [{"title": "New task"}, *({"id": 1, k: v} for k, v in [
    ("close", "Closed"), ("title", "Changed"), ("text", "Changed"), ("priority", 0),
    ("due", "2026-12-31"), ("parent", 2), ("blocked_by", [2]), ("title", None), ("extra", True)])]
replies = [tool(root, "task", action="write", title="Helper work"),
           tool(root, "task", action="write", title="Parent work", claim=True),
           tool(root, "agent", action="start", name="writer24", task=2, brief="Claim and comment on task 1."),
           tool(root, "agent", action="wait"),
           tool(root, "agent", action="settle", child="writer24", decision="merge"),
           tool(root, "task", action="get", id=1),
           tool(root, "task", action="list", filter="open"), reply(text="done", match=root),
           tool(helper, "task", action="write", id=1, claim=True, comment="Claimed by helper."),
           tool(helper, "task", action="write", id=1, comment="Evidence from helper."),
           tool(helper, "task", action="write", id=1, claim=False),
           tool(helper, "task", action="get", id=1),
           tool(helper, "task", action="write", id=1, claim=True),
           tool(helper, "task", action="write", id=3, claim=True),
           tool(helper, "task", action="write", id=3, claim=False, comment="Must not land."),
           *(tool(helper, "task", action="write", comment="Must not land.", **fields) for fields in forbidden),
           tool(helper, "agent", action="finish", status="finished", report="Helper task evidence.\nKept in full.")]
with flow(replies) as f:
    f.base()
    other = replace(f, cwd=f.cwd / "other")
    other.cwd.mkdir()
    other.base()
    replies[2:2] = [tool(root, "task", action="write", title="Unassigned work", claim=True)]
    replies[6:6] = [tool(root, "agent", action="start", name="other24", cwd=str(other.cwd), brief="Claim task 1."),
                    tool(root, "agent", action="wait"),
                    tool(root, "agent", action="settle", child="other24", decision="discard")]
    replies += [tool("OTHER_TASK_24", "task", action="write", title="Other work"),
                tool("OTHER_TASK_24", "task", action="write", title="Nested work"), reply(text="saved", match="OTHER_TASK_24"),
                tool("You are other24,", "task", action="write", id=1, claim=True),
                tool("You are other24,", "agent", action="start", name="nested24", brief="Claim task 2 and commit your work."),
                tool("You are other24,", "agent", action="wait"),
                tool("You are nested24,", "task", action="write", id=2, claim=True),
                reply((None, "Bash", {"command": "printf nested > nested-marker && git add nested-marker && git -c user.name=Flow -c user.email=flow@example.invalid commit -m Nested"}), match="You are nested24,"),
                tool("You are nested24,", "agent", action="finish", status="finished", report="Nested project report."),
                tool("You are other24,", "agent", action="finish", status="finished", report="Other project report.")]
    assert other.run("--print=json", "OTHER_TASK_24").returncode == 0
    result = f.run("--print=json", root)
    assert result.returncode == 0, result.stderr
    parent = json.loads(result.stdout)["session"]
    results = [e for e in f.record(parent) if e["kind"] == "result"]
    child = json.loads(results[3]["output"])["session"]
    writes = [e for e in f.record(child) if e["kind"] == "result"][:-1]
    assert [e["outcome"] for e in writes[:5]] == ["success"] * 5, writes[:5]
    assert all(json.loads(writes[n]["output"])["claimed"] for n in (0, 4)), writes[:5]
    assert "claim:" not in json.loads(writes[3]["output"])["file"], writes[3]
    assert all(e["outcome"] == "error" and "is claimed by cwd on" in e["output"] for e in writes[5:7]), writes[5:7]
    assert all(e["outcome"] == "error" and "claim" in e["output"] and "comment" in e["output"] for e in writes[7:]), writes[7:]
    task = json.loads(results[-2]["output"])
    assert "claim:" not in task["file"] and "closed:" not in task["file"], task
    assert sorted(c["text"].strip() for c in task["comments"]) == ["Claimed by helper.", "Evidence from helper.", "Helper task evidence.\nKept in full."], task
    tasks = json.loads(results[-1]["output"])["tasks"]
    assert [(t["title"], t["state"]) for t in tasks] == [("Helper work", "ready"), ("Unassigned work", "in progress")], tasks
    assigned = next(p for p in (f.home / ".razor/tasks").rglob("closed/2.md"))
    assert "claim:" not in assigned.read_text() and "Helper task evidence." in assigned.read_text(), assigned.read_text()
    other_task = next(p for p in (f.home / ".razor/tasks").rglob("1.md") if "title: Other work" in p.read_text())
    assert "claim:" not in other_task.read_text(), other_task.read_text()
    assert [p.read_text().split("---\n")[-1].strip() for p in other_task.parent.glob("comments/1/*.md")] == ["Other project report."]
    nested_task = other_task.with_name("2.md")
    stop(f)
    assert f.run("devices").returncode == 0
    until(f, parent, lambda _: "claim:" not in nested_task.read_text(), timeout=75)
    assert "closed:" not in nested_task.read_text(), nested_task.read_text()
    assert [p.read_text().split("---\n")[-1].strip() for p in nested_task.parent.glob("comments/2/*.md")] == ["Nested project report."]
