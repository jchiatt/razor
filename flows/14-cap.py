"""The twenty-first unfinished child is refused; a live cap increase permits it."""
import json
from driver import flow
from model import agent, reply

root = "FLOW_ROOT_14"
names = [f"cap{n:02}" for n in range(1, 22)]
replies = [agent(root, "start", name=name, kind="researcher", brief="Read, then wait without finishing.")
           for name in names[:20]]
replies += [agent(root, "start", name=names[20], kind="researcher", brief="The over-cap launch."),
            reply(root, "Bash", {"command": "python3 -c 'import os; from pathlib import Path; p=Path(os.environ[\"RAZOR_HOME\"])/\"config.toml\"; p.write_text(p.read_text()+\"\\n[children]\\nmax = 21\\n\")'"}),
            agent(root, "start", name=names[20], kind="researcher", brief="The now-permitted launch.")]
replies += [reply(root, "Bash", {"command": "python3 - <<'PY'\nimport json, os, time\nfrom pathlib import Path\nend = time.monotonic() + 120\nwhile time.monotonic() < end:\n    rows = [[json.loads(line) for line in p.read_text().splitlines(keepends=True) if line.endswith('\\n')] for p in Path(os.environ['RAZOR_HOME']).glob('sessions/*/record.jsonl')]\n    children = [r for r in rows if r and r[0].get('child')]\n    if len(children) == 21 and all(any(e['kind'] == 'turn_end' for e in r) for r in children):\n        break\n    time.sleep(.05)\nelse:\n    raise SystemExit('children did not all complete their first turns')\nPY"})]
replies += [agent(root, "stop", child=name) for name in names]
replies += [agent(root, "settle", child=name, decision="discard") for name in names]
replies += [reply(root)]
replies += [part for name in names for part in (
    reply(f"You are {name},", "Bash", {"command": "sleep 180", "run_in_background": True}),
    reply(f"You are {name},", text="Read complete; waiting for further work."))]
with flow(replies) as f:
    result = f.run("--print=json", root, timeout=180)
    assert result.returncode == 0, result.stderr
    parent = json.loads(result.stdout)["session"]
    entries = list(f.record(parent))
    starts = [e for e in entries if e["kind"] == "call" and e["input"].get("action") == "start"]
    assert len(starts) == 22, starts
    failed = [e for e in entries if e["kind"] == "result" and e["call"] == starts[20]["call"]]
    assert len(failed) == 1 and failed[0]["outcome"] == "error", failed
    assert "children.max" in failed[0]["output"] and "20" in failed[0]["output"], failed
    accepted = [e for e in entries if e["kind"] == "result" and e["call"] == starts[21]["call"]]
    assert len(accepted) == 1 and accepted[0]["outcome"] == "success", accepted
    children = [list(f.record(p.parent.name))[0] for p in (f.home / ".razor/sessions").glob("*/record.jsonl")
                if list(f.record(p.parent.name))[0].get("child")]
    assert len(children) == 21, children
    assert sorted(e["child"]["name"] for e in children) == names, children
    assert "max = 21" in (f.home / ".razor/config.toml").read_text(), "the raised cap was not live config"
