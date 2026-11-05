"""The terminal app with another session's ask and its own: a
digit inside 600 ms of a question taking the band is refused; the ask being answered in words
holds the band when another arrives; once it closes, Enter only says so; an option click keeps
the typed words. Before the asks: a handed skill draws one line, no row shows JSON, the status
pane lists deliveries newest first and copies a file's absolute path, no control in a delivery
reaches the terminal raw, the clock's octants go at phone width, a trailing backslash makes a
new line, and a Finder-escaped path and a file URL each attach. Then, through a socket that
holds the app's stream back and inside tmux: a new session's first input and one held for a
model change are never shown queued while their turns run, no octant is drawn, :codes: in code
are sent as typed, and with a turn running Esc closes the floating pane, then unpins."""
import base64
import json
from dataclasses import replace
from contextlib import suppress
import re
import socket
import threading
import time
from urllib.parse import urlencode
from driver import flow
import mcp
import server
from responses import PLAIN, reply as turn
from terminal import Terminal
from wcwidth import wcswidth


ESCAPE = "\x1b]52;c;SU5KRUNURUQ1Mg==\x07"  # an OSC 52 that would set the clipboard
HOSTILE = "odd" + ESCAPE + ".html"
LONG = "QUESTION_A " + "words to wrap " * 16 + "A_TAIL"  # wider than the band
replies = [turn("B_FIRST", "deliver", line="LINK_LINE", url="https://example.com/" + ESCAPE),
           turn("B_FIRST", "deliver", line="HOSTILE_LINE" + ESCAPE, path=HOSTILE),
           turn("B_FIRST", "recall", id=1), turn("B_FIRST", "first-turn-visible"),
           turn("B_FIRST", "pasted-visible"),
           turn("B_FIRST", "message", type="question", text="QUESTION_B", options=["b-one", "b-two"]),
           turn("B_FIRST", "b-waits"), turn("B_FIRST", "b-answered"),
           turn("A_ASK", "message", type="question", text=LONG, options=["a-one", "a-two"]),
           turn("A_ASK", "a-waits"), turn("A_ASK", "a-answered")]


def band(t):
    """The question band's rows: those over the editor whose bar runs left of its corner."""
    lines = t.screen.display
    end, x = next((y, line.index("╭─")) for y, line in enumerate(lines) if "╭─" in line)
    start = next(y for y in range(end, -1, -1) if y == 0 or lines[y - 1][x - 1] != "│")
    return "\n".join(lines[start:end])


def find(t, words, y=0, x=0):
    """The row and cell of the first drawn `words` at or below row `y`, from cell `x` on."""
    for n, line in enumerate(t.screen.display[y:], y):
        cells = [m.start() for m in re.finditer(re.escape(words), line)]
        for cell in (wcswidth(line[:at]) for at in cells):
            if cell >= x:
                return n, cell
    raise AssertionError(f"{words!r} is not drawn:\n" + t.text())


def click(t, at):
    y, x = at
    t.send(f"\x1b[<0;{x + 1};{y + 1}M\x1b[<0;{x + 1};{y + 1}m")


def octants(t):
    """Drawn glyphs of the octant and Legacy Computing blocks, which a phone's mosh and tmux
    hosts mismeasure and its font may lack."""
    return [c for c in t.text() if 0x1CC00 <= ord(c) <= 0x1CEBF or 0x1FB00 <= ord(c) <= 0x1FBFF]


def late(path):
    """Serves the server's socket at `path` through a relay that holds the first session
    stream's request 800 ms and the first admission's answer 300 ms, an order a busy Mac
    produces."""
    real, seen = path.with_name("relayed.sock"), set()
    path.rename(real)
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(path))
    listener.listen()

    def relay(source, target, held, seconds):
        with suppress(OSError):
            while chunk := source.recv(65536):
                if held in chunk and held not in seen:
                    seen.add(held)
                    time.sleep(seconds)
                target.sendall(chunk)
            target.shutdown(socket.SHUT_WR)

    def accept():
        while True:
            app, server = listener.accept()[0], socket.socket(socket.AF_UNIX)
            server.connect(str(real))
            threading.Thread(target=relay, args=(app, server, b"/events?after=", 0.8), daemon=True).start()
            threading.Thread(target=relay, args=(server, app, b'{"entry":', 0.3), daemon=True).start()
    threading.Thread(target=accept, daemon=True).start()


with flow(replies, model="gpt-6.1-sol", config=PLAIN + '[server]\nname="box"\nrealm="work"\n') as f:
    f.base()
    (f.cwd / HOSTILE).write_text("delivered page")
    (f.cwd / ".agents/skills/plan").mkdir(parents=True)
    (f.cwd / ".agents/skills/plan/SKILL.md").write_text(
        "---\ndescription: Plans the work.\n---\nSKILL_BODY_LINE\n")
    shot = f.cwd / "Screen Shot.png"
    shot.write_bytes(base64.b64decode(mcp.PNG))
    server.start(f)
    a = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    b = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    server.post(f, f"/sessions/{b}/input", {"text": "B_FIRST my own words", "skill": "plan"})
    server.until(f, b, lambda es: es[-1]["kind"] == "turn_end")
    inputs = lambda s: [e for e in f.record(s) if e["kind"] == "input"]
    deliveries = json.loads(server.send(f,"GET","/deliveries").read())
    for query,expected in (("EXAMPLE.COM",1),("ODD",1),("LINK_LINE "+f.cwd.name.upper(),1),("LINK_LINE HOSTILE_LINE",0)):
        rows = json.loads(server.send(f,"GET","/deliveries?"+urlencode({"q":query,"limit":1})).read())
        assert len(rows) == expected,(query,rows)
    file_entry = next(d["entry"] for d in deliveries if d["file"])
    receipt = f"/sessions/{b}/deliveries/{file_entry}"
    with Terminal(f, "--resume", b) as t:
        t.shown("first-turn-visible", "You handed", "plan — Plans the work", "my own words", "#1")
        assert octants(t), "no octant clock at 120 columns outside tmux:\n" + t.text()
        assert "SKILL_BODY_LINE" not in t.text() and '{"' not in t.text(), t.text()
        # The unread chip opens the pane at its inbox, which lists the file above the link delivered
        # before it; a press on the file copies its absolute path; the controls in a name, line or
        # link reach the terminal as nothing.
        t.resize(20, 120)  # too short for the pane to show its inbox unscrolled
        t.until(lambda t: t.screen.display[4].startswith(" CONTEXT"))  # redrawn at 20 rows: the click lands there
        t.click("2 unread")
        t.until(lambda t: any("LINK_LINE" in line[75:] for line in t.screen.display))
        t.shown("INBOX · 2 unread", "LINK_LINE", "HOSTILE_LINE]52")
        top, left = find(t, "── INBOX · 2 unread")
        assert find(t, "HOSTILE_LINE", top, left) < find(t, "LINK_LINE", top, left), t.text()
        click(t, find(t, "HOSTILE_LINE", top, left))
        copies = lambda t: re.findall(rb"\x1b\]52;c;([A-Za-z0-9+/=]+)\x07", t.output)
        t.until(copies)
        copied = base64.b64decode(copies(t)[-1]).decode()
        delivered = f.home / ".razor/sessions" / b / "delivered"
        assert copied == str(next(delivered.glob("*/" + HOSTILE))), copied
        assert ESCAPE.encode() not in t.output, "a delivery's control bytes reached the terminal"
        t.shown("INBOX · 1 unread")
        assert server.send(f,"DELETE",receipt+"/read").status == 200
        t.shown("INBOX · 2 unread")
        assert server.send(f,"PUT",receipt+"/archive").status == 200
        t.until(lambda t: "HOSTILE_LINE" not in "\n".join(line[75:] for line in t.screen.display))
        t.shown("INBOX · 1 unread")
        assert server.send(f,"DELETE",receipt+"/archive").status == 200
        t.shown("INBOX · 2 unread", "HOSTILE_LINE")
        assert server.send(f,"PUT",receipt+"/read").status == 200
        t.shown("INBOX · 1 unread")
        # At phone width the clock and the pane's picture draw in half blocks.
        t.resize(40, 40)
        t.until(lambda t: t.screen.display[0][39] == "┐")
        assert not octants(t), t.text()
        t.resize(40, 120)
        t.send("\x14")
        t.until(lambda t: "INBOX" not in t.text())
        # Enter after a trailing backslash is a new line, not a send; each pasted path attaches.
        t.send("PASTED abc\\")
        t.shown("PASTED abc\\")
        t.send("\rdef")
        t.until(lambda t: "def" in t.screen.display[find(t, "PASTED abc")[0] + 1])
        assert "PASTED abc\\" not in t.text(), t.text()
        t.send("\x1b[200~" + str(shot).replace(" ", "\\ ") + "\x1b[201~")
        t.shown("[Image #1]")
        t.send("\x1b[200~" + shot.as_uri() + "\x1b[201~")
        t.shown("[Image #2]")
        assert len(inputs(b)) == 1, inputs(b)
        t.send("\r")
        server.until(f, b, lambda es: len([e for e in es if e["kind"] == "input"]) == 2)
        sent = inputs(b)[-1]
        assert sent["text"].startswith("PASTED abc\ndef") and len(sent["images"]) == 2, sent
        t.shown("pasted-visible")
        # A digit inside 600 ms of a question taking the band is refused.
        server.post(f, f"/sessions/{a}/input", {"text": "A_ASK"})
        t.until(lambda t: "QUESTION_A" in band(t))
        assert "A_TAIL" in band(t), "the long question is cut:\n" + t.text()
        t.send("1")
        t.shown("NEW QUESTION: READ IT FIRST.")
        # Answering it in words, it keeps the band when this session asks too.
        t.send("\r")
        t.until(lambda t: "answering" in band(t))
        t.send("WORDS_FOR_A")
        t.shown("WORDS_FOR_A")
        server.post(f, f"/sessions/{b}/input", {"text": "B_ASK"})
        t.until(lambda t: " of 2" in band(t))
        assert "QUESTION_A" in band(t) and "answering" in band(t), t.text()
        assert "? box-cwd on box · work +1 · 1 unread" in t.screen.display[-2], t.text()
        # Answered elsewhere, its Enter only says so; the words stay.
        ask = next(e["id"] for e in f.record(a) if e["kind"] == "call")
        server.post(f, f"/sessions/{a}/asks/{ask}", {"option": 0})
        t.until(lambda t: "QUESTION_A" not in band(t) and "QUESTION_B" in band(t))
        assert "? box-cwd-2 on box · work · 1 unread" in t.screen.display[-2], t.text()
        t.send("\r")
        t.shown("THAT QUESTION CLOSED; YOUR WORDS STAY IN THE DRAFT.")
        # An option click past the guard answers it and leaves the words where they were.
        t.pump(0.7)
        click(t, find(t, "b-one", find(t, "YOUR CALL QUESTION_B")[0]))
        server.until(f, b, lambda es: any(isinstance(e["from"], dict) and "answer" in e["from"]
                                          for e in es if e["kind"] == "input"))
        t.until(lambda t: "b-one" not in band(t))
        assert "WORDS_FOR_A" in t.text(), t.text()
        assert not any("WORDS_FOR_A" in e["text"] for e in inputs(a) + inputs(b)), inputs(b)
        assert [e["from"] for e in inputs(a)][1:] == [{"answer": {"ask": ask}}], inputs(a)
        server.until(f, a, lambda es: any(e.get("text") == "a-answered" for e in es))
        server.until(f, b, lambda es: any(e.get("text") == "b-answered" for e in es))

# Each bash call runs until the flow makes its file, so the checks between run mid-turn.
until = "until [ -e {} ]; do sleep 0.05; done".format
big = lambda r: replace(r, body=r.body.replace(b'"input_tokens": 100,', b'"input_tokens": 60000,'))
replies = [turn(None, "bash", command=until("first")), turn(None, "PIN_47 pinned words"),
           turn(None, "bash", command=until("release")),
           turn(None, "bash", command=until("again")), big(turn(None, "kept-visible")),
           turn(None, "handover", note="NOTE_47"), turn(None, "after-visible")]
CODES = "codes :x: `a[1:x:2]`\n```\nb[1:x:2]\n```"
with flow(replies, model="gpt-6.1-sol", config=PLAIN) as f:
    server.start(f)
    late(f.home / ".razor/server.sock")
    f.env["TMUX"] = "/tmp/tmux-1000/default,4242,0"
    pinned = lambda t: t.text().count("PIN_47") == 2
    # An input is a transcript row once taken; the band's row for one queued says so.
    shown = lambda t, words: any(words in line and "queued" not in line for line in t.screen.display)
    with Terminal(f) as t:
        t.shown("Message the agent")
        # The first input starts its turn, so it is never queued, though the new session's
        # stream attaches after the admission took it.
        t.send("FIRST_47\r")
        t.shown(until("first"))
        assert shown(t, "FIRST_47"), t.text()
        (f.cwd / "first").touch()
        t.shown("PIN_47")
        t.send("/pin\r")
        t.until(pinned)
        # A :code: becomes its emoji in prose only, not in inline code or a fenced block.
        t.send(f"\x1b[200~{CODES}\x1b[201~\r")
        t.shown(until("release"))
        session = next((f.home / ".razor/sessions").iterdir()).name
        sent = [e["text"] for e in f.record(session) if e["kind"] == "input"]
        assert sent == ["FIRST_47", CODES.replace(":x:", "❌️", 1)], sent
        # Inside tmux neither the clock nor the pane's picture draws an octant.
        t.send("\x14")
        t.shown("STATUS")
        assert not octants(t), t.text()
        # With the turn running, Esc closes the floating pane, then unpins.
        t.send("\x1b")
        t.until(lambda t: "STATUS" not in t.text())
        t.send("\x1b")
        t.until(lambda t: not pinned(t))
        # Words staged with a model change join the running turn on its current model,
        # and appear taken while it runs.
        t.send("/model gpt-6-sol\r")
        t.send("SWITCH_47\r")
        server.until(f, session, lambda es: any(e.get("text") == "SWITCH_47" for e in es))
        (f.cwd / "release").touch()
        t.shown(until("again"))
        assert shown(t, "SWITCH_47"), t.text()
        assert json.loads(sorted(f.requests.glob("[0-9]*.body"))[-1].read_bytes())["model"] == "gpt-6.1-sol"
        (f.cwd / "again").touch()
        t.shown("kept-visible")
        # Over the floor the picker asks; `2` stages a handover, which the input carries.
        t.send("/model\r")
        t.shown("60k tokens uncached")
        t.send("\x1b[A\r")
        t.shown("1  keep the conversation", "2  start from a handover note")
        t.send("2")
        t.shown("GPT-6-LUNA · HIGH · START FROM A HANDOVER NOTE, ABOUT")
        t.send("AFTER_47\r")
        entries = server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 4)
    # No Esc stopped a turn; the staged words joined the second and the handover rebuilt.
    assert [e["end"] for e in entries if e["kind"] == "turn_end"] == ["done"] * 4, entries
    assert [(e["trigger"], e["note"]) for e in entries if e["kind"] == "rebuild"] == [("change", "NOTE_47")], entries
    assert next(e for e in entries if e.get("text") == "AFTER_47")["handover"] is True, entries
