"""A child's question interrupts a wait; the parent's answer reaches the child, under a header naming
the parent and the question. A researcher outside git works in its parent's folder and is told
Razor's researcher text."""
import json
import re
from pathlib import Path
from driver import flow
from model import agent, message, reply

root, child = "FLOW_ROOT_13", "You are asker13,"
replies = [agent(root, "start", name="asker13", kind="researcher", brief="Ask which marker to use."),
           agent(root, "wait", children=["asker13"], timeout_ms=600000), reply(root, text="Question received."),
           message(child, "question", text="Which exact marker?"), reply(child, text="Waiting for the parent.")]
with flow(replies) as f:
    first = f.run("--print=json", root, timeout=30)
    assert first.returncode == 0, first.stderr
    parent = json.loads(first.stdout)["session"]
    children = [p.parent.name for p in (f.home / ".razor/sessions").glob("*/record.jsonl")
                if list(f.record(p.parent.name))[0].get("child")]
    sid = children[0]
    start = next(f.record(sid))
    assert start["cwd"] == str(f.cwd.resolve()), "a researcher outside git left its parent's folder"
    researcher = (Path(__file__).parents[1] / "src/children/text/researcher.md").read_text().strip()
    first = next(e["text"] for e in f.record(sid) if e["kind"] == "input")
    assert first.index(child) < first.index(researcher) < first.index("Ask which marker to use."), first
    questions = [e for e in f.record(sid) if e["kind"] == "call" and e["input"].get("type") == "question"]
    assert len(questions) == 1, questions
    question = [sid, questions[0]["id"]]
    admitted = [e for e in f.record(parent) if e["kind"] == "input" and e["text"] == "Which exact marker?"]
    assert len(admitted) == 1 and admitted[0]["from"]["mail"]["id"] == question, admitted
    waits = [e for e in f.record(parent) if e["kind"] == "call" and e["input"].get("action") == "wait"]
    ended = next(e for e in f.record(parent) if e["kind"] == "result" and e["call"] == waits[0]["call"])
    assert ended["at"] - waits[0]["at"] < 30000, "a question did not interrupt the ten-minute wait"
    replies += [message(root, "answer", to="asker13", text="Use copper-heron-13.", question=question),
                agent(root, "wait", children=["asker13"]),
                agent(root, "settle", child="asker13", decision="discard"), reply(root),
                agent(child, "finish", status="finished", report="Used copper-heron-13.")]
    second = f.run("--resume", parent, "--print=json", "Answer the child.")
    assert second.returncode == 0, second.stderr
    answers = [e for e in f.record(sid) if e["kind"] == "input" and e["text"] == "Use copper-heron-13."]
    assert len(answers) == 1, answers
    assert answers[0]["from"]["mail"]["kind"] == {"answer": {"question": question}}, answers
    requests = [p.read_text() for p in sorted(f.requests.glob("*.body")) if p.stem.isdigit()]
    assert any(child in request and "Use copper-heron-13." in request for request in requests), "the admitted answer never reached the model"
    header = rf"{f.cwd.name} \(your parent\) answers #{question[1]}: Use copper-heron-13\."
    assert any(re.search(header, request) for request in requests), "the answer reached the model without its header"
