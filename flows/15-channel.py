"""Only a channel mention wakes the root; compact status shows headlines and files; the root
answers to its server's and project's name, not main."""
import json
from driver import flow
from model import agent, message, reply

root, child = "FLOW_ROOT_15", "You are oak15,"
replies = [reply(root, "mcp__razor__status", {}), reply(root, text="Named.")]
with flow(replies, config='[server]\nname = "desk"') as f:
    f.base()
    first = f.run("--print=json", root)
    assert first.returncode == 0, first.stderr
    parent = json.loads(first.stdout)["session"]

    def boards():
        entries = list(f.record(parent))
        calls = [e["call"] for e in entries if e["kind"] == "call" and "status" in e["name"]]
        return [json.loads(e["output"])["agents"] for e in entries if e["kind"] == "result" and e["call"] in calls]

    alone = boards()[0]
    name = alone[0]["name"]
    assert len(alone) == 1 and name == f"desk-{f.cwd.name}", alone
    replies += [agent(root, "start", name="oak15", brief="Commit a marker and post to the tree."),
                agent(root, "wait", children=["oak15"]), reply(root, "mcp__razor__status", {}),
                agent(root, "wait", children=["oak15"]),
                agent(root, "settle", child="oak15", decision="discard"), reply(root),
                agent(child, "headline", text="Working on channel flow."),
                reply(child, "Bash", {"command": "printf owned > owned15; git add -- owned15 && git -c user.name=Flow -c user.email=flow@example.invalid commit -m 'Owned marker'"}),
                message(child, "note", to="channel", text="QUIET_POST_15"),
                message(child, "note", to="channel", text=f"@{name} @{name} MENTION_POST_15"),
                agent(child, "finish", status="finished", report="Channel work finished.")]
    result = f.run("--resume", parent, "--print=json", "Start the lane.")
    assert result.returncode == 0, result.stderr
    entries = list(f.record(parent))
    inputs = [e for e in entries if e["kind"] == "input"]
    assert not any(e["text"] == "QUIET_POST_15" for e in inputs), inputs
    mentions = [e for e in inputs if e["text"] == f"@{name} @{name} MENTION_POST_15"]
    assert len(mentions) == 1, mentions
    assert mentions[0]["from"]["mail"]["kind"] == "mention" and mentions[0]["from"]["mail"]["from"] == "oak15", mentions
    sid = mentions[0]["from"]["mail"]["id"][0]
    channels = list((f.home / ".razor/channels").glob("*.jsonl"))
    assert len(channels) == 1 and channels[0].stem == parent, channels
    posts = [json.loads(line) for line in channels[0].read_text().splitlines()]
    assert [p["text"] for p in posts] == ["QUIET_POST_15", f"@{name} @{name} MENTION_POST_15"], posts
    assert posts[1]["id"] == mentions[0]["from"]["mail"]["id"], (posts, mentions)
    rows = boards()[-1]
    assert len(rows) == 2, rows
    main = next(row for row in rows if row["session"] == parent)
    lane = next(row for row in rows if row["session"] == sid)
    assert main["name"] == name and main["branch"] is None and main["changed_files"] == [], main
    assert lane["headline"] == "Working on channel flow." and lane["changed_files"] == ["owned15"], lane
