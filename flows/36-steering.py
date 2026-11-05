"""Click a real child card, steer that child, return: no input reaches its parent or sibling."""
from driver import flow
from model import agent, reply
import server
from terminal import Terminal

ROOT = "TERMINAL_STEER_ROOT"
replies = [agent(ROOT, "start", name="oak", brief="Read the brief and reply."),
           agent(ROOT, "start", name="elm", brief="Read the brief and reply."),
           reply(ROOT, text="parent-visible"),
           reply("You are oak,", "Bash", {"command": "sleep 120", "run_in_background": True}),
           reply("You are oak,", text="oak-visible"),
           reply("You are elm,", "Bash", {"command": "sleep 120", "run_in_background": True}),
           reply("You are elm,", text="elm-visible"), reply("You are oak,", text="oak-steered"),
           agent(ROOT, "stop", child="oak"), agent(ROOT, "stop", child="elm"), reply(ROOT)]
with flow(replies) as f:
    f.base()
    server.start(f)
    parent = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    server.post(f, f"/sessions/{parent}/input", {"text": ROOT})
    server.until(f, parent, lambda es: es[-1]["kind"] == "turn_end")
    with Terminal(f, "--resume", parent) as t:
        t.shown("parent-visible", "oak-visible", "elm-visible")
        children = {es[0]["child"]["name"]: path.parent.name
                    for path in (f.home / ".razor/sessions").glob("*/record.jsonl")
                    if (es := list(f.record(path.parent.name)))[0].get("child")}
        assert set(children) == {"oak", "elm"}, children
        for sid in children.values():
            jobs = [e for e in f.record(sid) if e["kind"] == "job"]
            assert len(jobs) == 1 and jobs[0]["event"] == "started", jobs
        before = {s: [e for e in f.record(s) if e["kind"] == "input"]
                  for s in [parent, *children.values()]}
        t.click("oak-visible")
        t.shown("talking to oak", "oak-visible", "as you")
        assert "parent-visible" in "\n".join(line[:60] for line in t.screen.display), t.text()
        t.send("Steer only oak.\r")
        t.shown("oak-steered")
        after = {s: [e for e in f.record(s) if e["kind"] == "input"] for s in before}
        assert after[parent] == before[parent] and after[children["elm"]] == before[children["elm"]]
        added = after[children["oak"]][len(before[children["oak"]]):]
        assert len(added) == 1 and added[0]["from"] == "person" and added[0]["text"] == "Steer only oak.", added
        t.send(b"\x1b")
        t.until(lambda t: "talking to oak" not in t.text() and "parent-visible" in t.text())
    cleanup = f.run("--resume", parent, "--print=json", "Stop both children's pending jobs.")
    assert cleanup.returncode == 0, cleanup.stderr
