"""Threshold rebuild keeps asks verbatim, reads the startup list again and replaces startup.md,
and recall returns the old entry whole; a turn that grows past the threshold again after its
rebuild gets a second threshold input."""
# Provisional request order, until the controller's order is proven.
import json
from claude import reply
from driver import REPLIES, Reply, flow
from responses import PLAIN, call, done, response
from server import agents

ask = "Remember this exact ask: the copper heron is 37 centimetres tall."
note = "Preserve the user's exact ask; recall entry 2 next."
file = lambda name: Reply((REPLIES / f"{name}.sse").read_bytes())
handover = reply(("toolu_note", "mcp__razor__handover", {"note": note}))
with flow([file("hello"), file("high-usage"), handover, file("done"), file("recall"), file("done")],
          config="[rebuild]\nthreshold = 0.1") as f:
    (f.cwd / "AGENTS.md").write_text("flow rules one")
    first = f.run("--print=json", ask)
    assert first.returncode == 0, first.stderr
    session = json.loads(first.stdout)["session"]
    old = list(f.record(session))[1]
    shown = f.home / ".razor/sessions" / session / "startup.md"
    assert "flow rules one" in shown.read_text(), shown.read_text()
    (f.cwd / "AGENTS.md").write_text("flow rules two")
    filled = f.run("--resume", session, "--print=json", "Fill the context, then keep your note.")
    assert filled.returncode == 0, filled.stderr
    recalled = f.run("--resume", session, "--print=json", "Recall the original ask by its entry id.")
    assert recalled.returncode == 0, recalled.stderr
    assert json.loads(recalled.stdout) == {"session": session, "message": "flow done"}
    entries = list(f.record(session))
    rebuilds = [e for e in entries if e["kind"] == "rebuild"]
    assert len(rebuilds) == 1 and rebuilds[0]["trigger"] == "threshold", rebuilds
    assert ask in rebuilds[0]["context"], rebuilds[0]
    assert rebuilds[0]["note"] == note, rebuilds[0]
    calls = [e for e in entries if e["kind"] == "call" and e["call"] == "toolu_recall"]
    assert len(calls) == 1 and calls[0]["id"] > rebuilds[0]["id"], calls
    recalled_entry = [e for e in entries if e["kind"] == "result" and e["call"] == "toolu_recall"]
    assert len(recalled_entry) == 1 and json.loads(recalled_entry[0]["output"]) == old, recalled_entry
    assert len([e for e in entries if e["kind"] == "start"]) == 1, entries
    assert any(e["kind"] == "input" and e["from"] == "razor" for e in entries), entries
    fresh = json.loads((f.requests / "004.body").read_bytes())
    assert ask in json.dumps(fresh["messages"]), fresh
    assert "flow rules two" in fresh["system"][-1]["text"], fresh["system"]
    assert fresh["messages"][0]["content"][0]["text"].startswith("# This session\n"), fresh["messages"][0]
    assert "flow rules two" in shown.read_text() and "flow rules one" not in shown.read_text(), shown.read_text()

# On GPT, one turn: a large response asks for the handover; in the rebuilt conversation a small
# first response (the rebuilt context fits) lifts the guard, and a later large one asks again.
def step(id, name, arguments, large=False):
    body = response([call(name, json.dumps(arguments), id)]).body
    return Reply(body.replace(b'"input_tokens": 100', b'"input_tokens": 200000') if large else body)


def run(replies, config="[rebuild]\nthreshold = 0.5"):
    with flow(replies, model="gpt-6.1-sol", config=PLAIN + config) as f:
        result = f.run("--print=json", ask)
        assert result.returncode == 0, result.stderr
        entries = list(f.record(json.loads(result.stdout)["session"]))
        asked = [e["id"] for e in entries if e["kind"] == "input" and e["from"] == "razor"]
        return entries, asked, [e for e in entries if e["kind"] == "rebuild"]


entries, asked, rebuilds = run([step("grow1", "bash", {"command": "true"}, large=True),
                                step("hand1", "handover", {"note": note}), step("after1", "bash", {"command": "true"}),
                                step("grow2", "bash", {"command": "true"}, large=True),
                                step("hand2", "handover", {"note": note}), done()])
assert [e["trigger"] for e in rebuilds] == ["threshold", "threshold"], rebuilds
# A reading's threshold input is recorded before its response's calls: the second falls between
# after1's call and grow2's.
at = {e["call"]: e["id"] for e in entries if e["kind"] == "call"}
assert len(asked) == 2 and asked[0] < rebuilds[0]["id"] < at["after1"] < asked[1] < at["grow2"] < rebuilds[1]["id"], \
    (asked, rebuilds)
# A rebuilt context already past the threshold keeps the guard for its turn: no second ask, so no
# rebuild after every request.
entries, asked, rebuilds = run([step("grow1", "bash", {"command": "true"}, large=True),
                                step("hand1", "handover", {"note": note}),
                                step("after1", "bash", {"command": "true"}, large=True),
                                step("grow2", "bash", {"command": "true"}, large=True), done()])
assert len(asked) == 1 and [e["trigger"] for e in rebuilds] == ["threshold"], (asked, rebuilds)

# Each override fires where the global threshold and shipped window would not. A short GPT
# name in config applies to its full model id, including the loop route's Context readings.
for settings, large in (("[rebuild]\nthreshold = 0.9\n[models.sol]\nthreshold = 0.5", True),
                        ("[models.sol]\nwindow = 100", False)):
    entries, asked, rebuilds = run([step("configured", "bash", {"command": "true"}, large=large),
                                   step("configured_note", "handover", {"note": note}), done()], settings)
    assert len(asked) == 1 and [e["trigger"] for e in rebuilds] == ["threshold"], (asked, rebuilds)

# A live Claude override replaces the SDK's window for Razor alone. The edit is read on the
# next admitted turn, after the first turn has supplied an SDK window.
with flow([file("hello"), file("high-usage"), handover, file("done"), file("hello")],
          config="[models.opus]\nwindow = 1000000\nthreshold = 1.0") as f:
    first = f.run("--print=json", ask)
    assert first.returncode == 0, first.stderr
    session = json.loads(first.stdout)["session"]
    path = f.home / ".razor/config.toml"
    path.write_text(path.read_text().replace("window = 1000000", "window = 200").replace("threshold = 1.0", "threshold = 0.5"))
    again = f.run("--resume", session, "--print=json", "Use the edited limits.")
    assert again.returncode == 0, again.stderr
    entries = list(f.record(session))
    assert [e["trigger"] for e in entries if e["kind"] == "rebuild"] == ["threshold"], entries
    later = f.run("--resume", session, "--print=json", "Continue with the SDK window already reported.")
    assert later.returncode == 0, later.stderr
    row = agents(f)[session]
    assert row["window"] == 200 and row["threshold"] == 0.5, row
    before, after = [json.loads((f.requests / f"{n:03}.body").read_bytes()) for n in (1, 2)]
    for key in ("model", "max_tokens", "thinking", "output_config"):
        assert before.get(key) == after.get(key), (key, before.get(key), after.get(key))
