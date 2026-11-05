"""The recap (docs/design/api.md): leaving a session reports the last entry
shown; ten minutes after the last look (or the terminal's own, lost at a restart), not nine, the
terminal recaps what came after it. Haiku at medium writes a Claude or open model session's recap
and Terra at low a GPT session's, from the agent's words, then its newest steps."""
from dataclasses import replace
import json
import claude
from clock import wall_clock
from driver import flow
from model import reply
from responses import reply as turn
import server
from terminal import Terminal

EARLIER, FINAL = "EARLIER69 " + "earlier " * 1500 + "END69", "FINAL69 " + "summary " * 400 + "END69"
replies = [reply("ALPHA69", text="a-first69"), reply("BRAVO69", text="b-first69"),
           reply("MORE69", text="a-more69"), reply("LATER69", text="a-later69"),
           reply("FINAL69", text="a-final69"),
           claude.reply(*[(None, "Read", {"file_path": f"/nowhere69/STEP_{n:03}_{'x' * 100}"}) for n in range(300)], text=EARLIER, match="STEPS69"),
           reply("STEPS69", text=FINAL), reply("OLD69", text="OLD_WORDS69"), reply("AFTER69", text="b-after69"), reply("SLOW69", text="b-slow69"),
           reply("LONG69", text="LONG69 " + "long " * 8000 + "CONCLUSION69"), replace(reply("b-slow69", text="STALE69"), cuts=tuple(range(1, 200))),
           # The GPT session's recap request also holds the prompt, so its reply comes first.
           turn('"model":"gpt-6.1-terra"', "GPT_WORDS69"),
           *[reply("Tell the person", text="RECAP_WORDS69")] * 6]
config = '[ui]\nmotion = false\n[models."open69"]\nbase_url = "{standin}"\n'
with wall_clock() as (_, env, advance), flow(replies, config=config, env=env) as f:
    server.start(f)

    def root(name, words):
        made = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        server.send(f, "PUT", f"/sessions/{made}/name", name).read()
        server.post(f, f"/sessions/{made}/input", {"text": words})
        server.until(f, made, lambda es: es[-1]["kind"] == "turn_end")
        return made

    def moves(words, said, s=None):  # work in a (or s): not the person's words, nor anyone looking
        server.post(f, f"/sessions/{s or a}/input", {"text": words, "from": "flow"})
        server.until(f, s or a, lambda es: es[-1]["kind"] == "turn_end" and any(e.get("text") == said for e in es))

    a = root("a69", "ALPHA69")
    advance(1000)  # b is the most recent, which --agents opens
    b = root("b69", "BRAVO69")
    with Terminal(f, "--agents") as t:
        def shown(words):  # in the addressed session's transcript, right of the list
            t.until(lambda t: "┐" in (top := t.screen.display[0]) and any(words in l[top.index("┐") + 1:] for l in t.screen.display))
        shown("b-first69")
        t.shown("○ a69", "○ b69")  # the list's layout, published each second, can trail
        t.click("○ a69")
        shown("a-first69")
        # Switching away reports a's last entry shown, with when.
        last = list(f.record(a))[-1]["id"]
        t.click("○ b69")
        shown("b-first69")
        server.wait(lambda: server.agents(f)[a].get("seen") == last, 10, "switching away reported nothing")
        moves("MORE69", "a-more69")
        advance(9 * 60_000)
        t.shown("╰ a-more69")  # the list has moved a's row before it is clicked
        t.click("○ a69")
        shown("a-more69")
        # Nine minutes away from focus, with a's record moved, asks nothing either.
        t.send("\x1b[O")
        server.wait(lambda: server.agents(f)[a]["seen"] > last, 10, "losing focus reported nothing")
        moves("LATER69", "a-later69")
        advance(18 * 60_000)
        t.send("\x1b[I")
        shown("a-later69")
        # Ten minutes away from a, by switching to b, recaps what came after the last entry seen.
        t.click("○ b69")
        moves("FINAL69", "a-final69")
        advance(28 * 60_000)
        t.shown("╰ a-final69")
        t.click("○ a69")
        t.shown("↺ away 10 min · RECAP_WORDS69")
        asked = [r.read_text() for r in f.requests.glob("*.body") if "Tell the person" in r.read_text()]
        assert len(asked) == 1 and "a-final69" in asked[0] and "a-later69" not in asked[0], asked
        # After a restart loses `seen`, b, the root whose stream runs on while a is addressed,
        # recaps from the last entry this terminal showed as it left b.
        server.stop(f)
        server.start(f)
        server.wait(lambda: server.agents(f)[b]["terminal"], 10, "the terminal never reconnected")
        moves("AFTER69", "b-after69", b)
        advance(40 * 60_000)
        t.shown("╰ b-after69")
        t.click("○ b69")
        t.shown("↺ away 12 min")  # since the restart; a's recap said 10
        # A recap that lands after its return has ended, here by leaving and coming back, is not shown.
        t.send("\x1b[O")
        server.wait(lambda: server.agents(f)[b].get("seen") == list(f.record(b))[-1]["id"])
        moves("SLOW69", "b-slow69", b)
        advance(55 * 60_000)
        shown("b-slow69")
        t.send("\x1b[I")
        stale = server.wait(lambda: [r for r in f.requests.glob("*.body") if "b-slow69" in r.read_text()])[0]
        t.send("\x1b[O\x1b[I")
        server.wait(lambda: "sent" in json.loads(stale.with_suffix(".json").read_text()))
        t.pump(3, lambda _: "STALE69" in t.text())  # it lands about 0.3 s after
        assert "STALE69" not in t.text(), t.text()

    def written(session):  # the model request that wrote a recap of all of it
        assert server.post(f, f"/sessions/{session}/recap", {})["text"]
        return json.loads(max(f.requests.glob("[0-9]*.body")).read_text())

    steps = root("steps69", "STEPS69")
    body = written(steps)
    evidence = body["messages"][0]["content"][0]["text"]
    assert "haiku" in body["model"] and body["output_config"] == {"effort": "medium"}, body
    # The agent's words come before its steps: both summaries whole, then the newest steps.
    assert EARLIER in evidence and FINAL in evidence and "STEP_299" in evidence and "STEP_000" not in evidence, evidence
    assert len(evidence.encode()) <= 32 << 10, len(evidence)
    gpt = server.post(f, "/sessions", {"cwd": str(f.cwd), "model": "sol"})["session"]
    body = written(gpt)
    assert body["model"] == "gpt-6.1-terra" and body["reasoning"] == {"effort": "low"}, body
    chat = server.post(f, "/sessions", {"cwd": str(f.cwd), "model": "open69"})["session"]
    body = written(chat)
    assert "haiku" in body["model"] and body["output_config"] == {"effort": "medium"}, body
    # Newest words longer than the whole budget keep their start and their conclusion.
    evidence = written(root("long69", "LONG69"))["messages"][0]["content"][0]["text"]
    assert "words:\nLONG69" in evidence and "[…middle cut…]" in evidence and "CONCLUSION69\n" in evidence and len(evidence.encode()) <= 32 << 10, evidence
    # A recap reads the newest 4,096 entries at most, so words further back never reach it.
    old = root("old69", "OLD69")
    server.stop(f)
    with (f.home / ".razor/sessions" / old / "record.jsonl").open("r+") as record:
        end = json.loads(record.readlines()[-1])
        record.writelines(json.dumps({"id": end["id"] + n, "at": end["at"], "kind": "thinking", "text": "t"}) + "\n" for n in range(1, 4097))
    server.start(f)
    assert "OLD_WORDS69" not in written(old)["messages"][0]["content"][0]["text"]
