"""A pack directory supplies a resident, a skill, a brand and display settings: the person's config
is laid over the pack's key by key, the pack's relative paths count from the pack, a new
conversation lists the pack's and the project's skills with descriptions and the `skills` key's
folders are omitted, `skill` reads from the last of them, and the status stream carries the brand and the settings,
again when they change, and keeps the last good ones when the pack goes missing
(docs/design/packs.md)."""
import json
import sys
import tempfile
import time
import urllib.request
from pathlib import Path
from claude import reply
from driver import REPLIES, Reply, flow
from server import send, until, wait

ask, body = "Read the lore skill.", "Tell the flow's old story."
done = (REPLIES / "done.sse").read_bytes()
replies = [reply(("toolu_pack_skill", "mcp__razor__skill", {"name": "lore"}), match=ask),
           Reply(done), Reply(done, match="Scout heartbeat.")]
with tempfile.TemporaryDirectory() as scratch:
    pack = Path(scratch) / "pack"
    (pack / "work").mkdir(parents=True)
    (pack / "skills/greet").mkdir(parents=True)
    (pack / "skills/greet/SKILL.md").write_text(
        "---\nname: greet\ndescription: >-\n  Greets the flow\n  in one line.\n---\nSay hello.\n")
    (pack / "skills/plan").mkdir()  # no SKILL.md, so the project's `plan` is still listed
    (pack / "library/lore").mkdir(parents=True)
    (pack / "library/lore/SKILL.md").write_text(f"---\ndescription: Tells old stories.\n---\n{body}\n")
    (pack / "scout.md").write_text("You are Scout, this pack's resident.")
    due = time.strftime("%H:%M:%S", time.localtime(time.time() + 6))
    config = ('skills = ["library"]\n[brand]\nname = "Acme"\n[brand.colours]\nviolet = "#336699"\n'
              '[ui]\nmotion = false\n'
              '[residents.scout]\ncwd = "work"\nstartup = ["scout.md", "razor"]\n'
              f'[residents.scout.heartbeat]\nat = ["{due}"]\ntext = "Scout heartbeat."\n')
    (pack / "config.toml").write_text(config)
    with flow(replies, config=f'pack = {json.dumps(str(pack))}\n[brand]\nname = "Mine"\n'
                              '[ui]\nblocks = "half"\n'
                              '[server]\naddress = "127.0.0.1:0"') as f:
        # 1. The pack's and the project's skills are listed with their descriptions, the `skills`
        # folder's are omitted from the first request; `skill` still reads the last.
        (f.cwd / ".agents/skills/plan").mkdir(parents=True)
        (f.cwd / ".agents/skills/plan/SKILL.md").write_text("---\ndescription: Plans the flow.\n---\n")
        result = f.run("--print=json", ask)
        assert result.returncode == 0, result.stderr
        entries = list(f.record(json.loads(result.stdout)["session"]))
        output = next(e for e in entries if e["kind"] == "result")
        assert output["outcome"] == "success" and body in output["output"], output
        first = next(r.read_text() for r in sorted(f.requests.glob("*.body")) if ask in r.read_text())
        assert "- greet: Greets the flow in one line." in first, "the pack's skill was not listed"
        assert "- razor-settings:" not in first, "the named pack kept the starter skills"
        assert "- plan: Plans the flow." in first, "the project's skill was not listed"
        assert "\\n- lore" not in first and "Tells old stories" not in first, "the opening listed a library skill"
        session = json.loads(result.stdout)["session"]
        assert "lore" in [s["name"] for s in json.loads(send(f, "GET", f"/sessions/{session}/skills").read())]
        # 2. The pack's resident starts on its own, in and with paths relative to the pack.
        until(f, "scout", lambda es: any(e["kind"] == "turn_end" for e in es), timeout=40)
        start = next(f.record("scout"))
        assert start["cwd"] == str(pack / "work"), start
        beat = next(r.read_text() for r in f.requests.glob("*.body") if "Scout heartbeat." in r.read_text())
        assert "You are Scout" in beat, "the pack's persona missed the model"
        # 3. The status stream opens with the brand: the person's name over the pack's colours.
        server = json.loads((f.home / ".razor/server.json").read_text())
        key = (f.home / ".razor/device.key").read_text().strip()
        status = urllib.request.Request(f"http://{server['address']}/v1/status",
                                        headers={"Authorization": f"Bearer {key}"})
        with urllib.request.urlopen(status, timeout=10) as stream:
            line = json.loads(stream.readline())
            assert line["kind"] == "agents", line
            assert line["brand"] == {"name": "Mine", "colours": {"violet": "#336699"}}, line["brand"]

            def snapshot(test, why):
                """The next `agents` line passing `test`, within 3 s."""
                deadline = time.monotonic() + 3
                try:
                    while True:
                        line = json.loads(stream.readline())
                        assert time.monotonic() < deadline, why
                        if line["kind"] == "agents" and test(line):
                            return line
                except TimeoutError:
                    raise AssertionError(why) from None

            # 4. A changed pack file reaches the stream without a restart.
            (pack / "config.toml").write_text(config.replace("#336699", "#993366"))
            snapshot(lambda l: l["brand"]["colours"] == {"violet": "#993366"},
                     "the changed brand was not sent within 3 s")
            # 5. A missing pack is refused by name, and the whole last good config stays.
            person = f.home / ".razor/config.toml"
            missing = Path(scratch) / "gone"
            person.write_text(person.read_text().replace(str(pack), str(missing)))
            line = snapshot(lambda l: str(missing) in (l["config"] or ""),
                            "the missing pack was not named within 3 s")
            assert line["brand"] == {"name": "Mine", "colours": {"violet": "#993366"}}, line["brand"]

# With no named pack a stranger gets the starter, including the about word, skills and actual
# notification/speech programs. Both desktop commands record here, ahead of driver's no-ops.
with flow([reply((None, "mcp__razor__message", {"to": "parent", "type": "question",
                 "text": "QUESTION_STARTER_30", "options": ["yes", "no"]})),
           reply(text="waiting")]) as f:
    programs, notice = f.home / "bin", f.home / "notice.json"
    programs.mkdir()
    for name in ("osascript", "notify-send"):
        program = programs / name
        program.write_text(f'#!{sys.executable}\nimport json,sys\nfrom pathlib import Path\nPath({str(notice)!r}).write_text(json.dumps(sys.argv[1:]))\n')
        program.chmod(0o755)
    f.env["PATH"] = f"{programs}:{f.env['PATH']}"
    (f.home / ".razor/about.md").write_text("ABOUT_STARTER_30 knows the project.")
    result = f.run("--print=json", "Ask the person.")
    assert result.returncode == 0, result.stderr
    first = json.loads((f.requests / "001.body").read_text())
    opening = json.dumps(first)
    starter = Path(__file__).resolve().parents[1] / "packs/starter"
    # These are prompt data, compared to the files, so editing prose does not break the flow.
    for file in ("persona.md", "rules.md"):
        assert (starter / file).read_text().strip() in "\n".join(
            block.get("text", "") for block in first["system"]), f"starter {file} missed the model"
    assert "ABOUT_STARTER_30" in opening, "about.md missed the model"
    for skill in ("agent-browser", "razor-settings", "writing", "review", "new-steward"):
        assert f"- {skill}:" in opening, f"starter skill {skill} was not listed"
    wait(notice.exists, 10, "starter question never reached the desktop program")
    assert "QUESTION_STARTER_30" in " ".join(json.loads(notice.read_text())), "question text missed notifier"
    session = json.loads(result.stdout)["session"]
    said = next(e for e in f.record(session) if e["kind"] == "text")
    answer = send(f, "GET", f"/sessions/{session}/speech/{said['id']}")
    why = answer.read().decode()
    assert answer.status == 500 and "razor keys" in why and "ElevenLabs" in why, why
