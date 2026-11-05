"""Thinking summaries in the terminal: Claude's are asked for, since the API omits their text
unless a request asks, and both makers' are drawn between the calls as an aside under no cap."""
from dataclasses import replace
import json

import claude
from driver import flow
from responses import PLAIN, call, done, response
import server
from terminal import Terminal

ASKED = '"display":"summarized"'  # the API returns thinking text only to a request holding it
LONG = " ".join(f"T{i:03}" for i in range(300))
replies = [claude.reply((None, "mcp__razor__status", {}), thinking="CLAUDE_THOUGHT_65 before the call.", match=ASKED),
           claude.reply(text="claude-reply-65", thinking=LONG, match=ASKED)]
with flow(replies) as f:
    result = f.run("--print=json", "Think, look, reply.")
    assert result.returncode == 0, result.stderr
    session = json.loads(result.stdout)["session"]
    assert json.loads((f.requests / "001.body").read_text())["thinking"]["display"] == "summarized"
    with Terminal(f, "--resume", session, rows=60) as t:
        t.shown("CLAUDE_THOUGHT_65", "claude-reply-65", "more lines · click to open")
        lines = t.screen.display
        at = lambda words: next(y for y, line in enumerate(lines) if words in line)
        # The thought sits on its dotted rail above the call; the reply after the long one keeps
        # its own cap, and the long one shows its first three rows.
        assert lines[at("CLAUDE_THOUGHT_65")][17:].startswith("  ┊ CLAUDE_THOUGHT_65"), t.text()  # past the tape
        assert at("CLAUDE_THOUGHT_65") < at("status") < at("T000"), t.text()
        assert "Opus" in lines[at("claude-reply-65") - 1] and "T299" not in t.text(), t.text()
        assert at("more lines") - at("T000") == 3, t.text()
        t.click("more lines · click to open")
        t.shown("T299")
        assert "more lines" not in t.text(), t.text()

# GPT returns one title per summary part, joined by line breaks: each its own row.
reasoning = {"type": "reasoning", "id": "rs_65", "summary": [
    {"type": "summary_text", "text": "**GPT_TITLE_A**"}, {"type": "summary_text", "text": "**GPT_TITLE_B**"}]}
with flow([response([reasoning, call("status", "{}")]), done()], model="gpt-6.1-sol", config=PLAIN) as f:
    result = f.run("--print=json", "Think, look, reply.")
    assert result.returncode == 0, result.stderr
    with Terminal(f, "--resume", json.loads(result.stdout)["session"]) as t:
        t.shown("GPT_TITLE_A", "GPT_TITLE_B", "flow done")
        rows = [line[17:].strip() for line in t.screen.display]  # past the tape
        assert "┊ GPT_TITLE_A" in rows and "┊ GPT_TITLE_B" in rows, t.text()

# A thought the model has written nothing after is live: its rail cell spins, a cursor follows its
# last letter that no drag copies, and the newest letters cool from violet-white to gray, until the
# entry after it settles it to the plain rail.
held = claude.reply(("toolu_live65", "mcp__razor__status", {}), thinking="LIVE_65", match="Think slowly.")
held = replace(held, cuts=tuple(range(held.body.index(b"toolu_live65"), len(held.body), 10)))  # the model takes about a second more
with flow([held, claude.reply(text="settled-65")]) as f:
    server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    with Terminal(f, "--resume", session, rows=60) as t:
        server.post(f, f"/sessions/{session}/input", {"text": "Think slowly."})
        spins, lasts = [], []  # in each frame the thought is live: its spinner's colour and its last letter's

        def look(t):
            y = next((y for y, l in enumerate(t.screen.display) if "LIVE_65" in l), 0)
            row, x = t.screen.buffer[y], t.screen.display[y].find("LIVE_65")
            if "\u2800" <= row[x - 2].data <= "\u28ff":
                spins.append(row[x - 2].fg), lasts.append(row[x + 6].fg)
            return y, x, t.screen.display[y][x - 2:].rstrip()
        t.until(lambda t: look(t)[2].endswith("▍"))
        y, x, _ = look(t)
        t.send(f"\x1b[<0;{x + 1};{y + 1}M\x1b[<32;{x + 8};{y + 1}M\x1b[<0;{x + 8};{y + 1}m")
        t.until(lambda t: look(t) and "COPIED 7 CHARS" in t.text())  # LIVE_65, not its cursor
        t.until(lambda t: look(t)[2] == "┊ LIVE_65" and "settled-65" in t.text())
        y, x, _ = look(t)
        hot, gray, bright = spins[-1], t.screen.buffer[y][x + 6].fg, lambda c: sum(bytes.fromhex(c))
        assert len(set(spins)) == 1, spins  # the warm letters never recolour the rail cell
        assert bright(gray) < bright(hot) and any(bright(gray) < bright(c) < bright(hot) for c in lasts), lasts
