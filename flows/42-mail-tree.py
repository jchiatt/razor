"""Mail in a tree reaches the model under a header naming its sender (docs/design/mail.md): a
child's note and question, and a long mention cut to 2,000 characters; the record keeps the
sender's words alone, and the channel file holds the whole post."""
import json
from driver import flow
from model import agent, message, reply
from server import until


def texts(value):
    """Every string in a request body: the words the model was sent."""
    if isinstance(value, dict):
        return [t for v in value.values() for t in texts(v)]
    if isinstance(value, list):
        return [t for v in value for t in texts(v)]
    return [value] if isinstance(value, str) else []


root, child = "FLOW_ROOT_42", "You are wren42,"
replies = [reply(root, "mcp__razor__status", {}), reply(root, text="Named.")]
with flow(replies) as f:
    first = f.run("--print=json", root)
    assert first.returncode == 0, first.stderr
    parent = json.loads(first.stdout)["session"]
    status = next(e for e in f.record(parent) if e["kind"] == "result")
    name = json.loads(status["output"])["agents"][0]["name"]
    assert name == f.cwd.name, name
    long = f"@{name} " + "y" * 10_000 + " TAIL_42"
    pause = {"command": "sleep 2"}  # so each mail opens a turn of its own
    replies += [agent(root, "start", name="wren42", kind="researcher", brief="Write to your parent."),
                reply(root, text="Started."),
                reply(child, "Bash", pause), message(child, "note", text="NOTE_42"),
                reply(child, "Bash", pause), message(child, "question", text="Which marker?"),
                reply(child, "Bash", pause), message(child, "note", to="channel", text=long),
                reply(child, text="Posted."),
                reply("NOTE_42", text="Noted."), reply("Which marker?", text="Asked."),
                reply("(the rest: message read)", text="Mentioned.")]
    second = f.run("--resume", parent, "--print=json", "Start the child.")
    assert second.returncode == 0, second.stderr

    def mentioned(entries):
        ids = [e["id"] for e in entries if e["kind"] == "input" and e["text"] == long]
        return ids and any(e["kind"] == "turn_end" and ids[0] in e["inputs"] for e in entries)

    inputs = {e["text"]: e for e in until(f, parent, mentioned) if e["kind"] == "input"}
    note, question, mention = (inputs[text]["from"]["mail"] for text in ("NOTE_42", "Which marker?", long))
    sid = note["id"][0]
    assert (note["kind"], note["from"]) == ("note", "wren42"), note
    assert (question["kind"], question["from"]) == ("question", "wren42"), question
    assert (mention["kind"], mention["from"]) == ("mention", "wren42"), mention
    seen = ["\n".join(texts(json.loads(p.read_bytes()))) for p in sorted(f.requests.glob("*.body"))
            if p.stem.isdigit() and root.encode() in p.read_bytes()]
    shown = ["wren42 (your child): NOTE_42",
             f'wren42 (your child) asks ["{sid}",{question["id"][1]}]: Which marker?',
             "wren42 in the channel: " + long[:2000] + " ... (the rest: message read)"]
    for words in shown:
        assert any(words in text for text in seen), f"no request held {words[:60]!r}"
    assert not any("TAIL_42" in text for text in seen), "the model read past the mention's 2,000 characters"
    channel = (f.home / ".razor/channels" / f"{parent}.jsonl").read_text()
    assert json.loads(channel.splitlines()[-1])["text"] == long, "the channel file lost the post's end"

# A parent's mail reaches its working child at the child's next tool boundary, as the person's
# words do (Trey, 2026-10-29): the child's one turn answers its brief and the note together.
root, child = "FLOW_ROOT_42B", "You are fern42,"
replies = [agent(root, "start", name="fern42", brief="Work, then reply."), reply(root, text="Started."),
           reply(child, "Bash", {"command": "sleep 6"}), reply("(your parent): STEER_42", text="Steered."),
           message("SEND_42", "note", to="fern42", text="STEER_42"), reply("SEND_42", text="Sent."),
           reply(root, text="Heard.")]
with flow(replies) as f:
    started = f.run("--print=json", root)
    assert started.returncode == 0, started.stderr
    parent = json.loads(started.stdout)["session"]
    sid = next(p.parent.name for p in (f.home / ".razor/sessions").glob("*/record.jsonl")
               if (es := list(f.record(p.parent.name)))[0].get("child", {}).get("name") == "fern42")
    until(f, sid, lambda es: any(e["kind"] == "job" for e in es))
    sent = f.run("--resume", parent, "--print=json", "SEND_42")
    assert sent.returncode == 0, sent.stderr

    def answered(entries):
        note = [e["id"] for e in entries if e["kind"] == "input" and e["text"] == "STEER_42"]
        return note and any(e["kind"] == "turn_end" and note[0] in e["inputs"] for e in entries)

    entries = until(f, sid, answered)
    ends = [e for e in entries if e["kind"] == "turn_end"]
    assert len(ends) == 1 and len(ends[0]["inputs"]) == 2, ends
    until(f, parent, lambda es: es[-1]["kind"] == "turn_end" and any(e.get("text") == "Steered." for e in es))
