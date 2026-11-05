"""Child prose reports, pending jobs, mail grace/nudge, a person's split input and errors."""
from dataclasses import replace
import json
import os
from pathlib import Path
import signal
import time
from driver import flow
from model import agent, message, reply
from responses import done, response
import server
from standin import Reply
from terminal import Terminal

PLAIN, JOB, MAIL, PERSON, ERROR, LONG, CRASH, HELD = [f"ROOT_40_{name}" for name in ("PLAIN", "JOB", "MAIL", "PERSON", "ERROR", "LONG", "CRASH", "HELD")]
plain, job, mail, sibling, person, error, long, crash, held = [f"You are {name}40," for name in ("plain", "job", "mail", "sibling", "person", "error", "long", "crash", "held")]
closing = ["First closing paragraph40.", "Closing report40. " + "🪒" * 17000]
long_text = "\n\n".join(closing)
replies = [
    agent(PLAIN, "start", name="plain40", brief="End in prose, without finish.", placement="in_place"),
    agent(PLAIN, "wait"), reply(PLAIN, text="plain-report-received40"),
    reply(plain, text=closing),
    agent(JOB, "start", name="job40", brief="Leave a job running, then report.", placement="in_place"),
    reply(JOB, text="job-parent-ready40"), reply(JOB, text="job-report-received40"),
    reply(job, "Bash", {"command": "while [ ! -f release40 ]; do sleep 0.05; done", "run_in_background": True}),
    reply(job, text="job-waiting40"), reply(job, text="job-closing40"),
    agent(MAIL, "start", name="sibling40", brief="Do not reply to mentions.", placement="in_place"),
    agent(MAIL, "start", name="mail40", brief="Mention your sibling and wait.", placement="in_place"),
    reply(MAIL, text="mail-parent-ready40"), reply(MAIL, text="mail-report-received40"),
    reply(sibling, "Bash", {"command": "sleep 120", "run_in_background": True}),
    reply(sibling, text="sibling-ready40"), reply(sibling, text="sibling-no-reply40"),
    message(mail, "note", to="channel", text="@sibling40 Please look at this."),
    reply(mail, text="mail-waiting40"), reply(mail, text="mail-nudged-report40"),
    agent(PERSON, "start", name="person40", brief="Send your parent a note, then wait.", placement="in_place"),
    reply(PERSON, text="person-parent-ready40"), reply(PERSON, text="person-parent-ready40"),
    message(person, "note", text="Ready for steering40."),
    reply(person, text="person-visible40"), reply(person, text="person-answer40"),
]

with flow(replies, config='[children]\ngrace = "1h"\n') as f:
    server.start(f)

    def start(text):
        sid = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        server.post(f, f"/sessions/{sid}/input", {"text": text})
        return sid

    def child(parent, name):
        entries = server.until(f, parent, lambda es: any(e["kind"] == "child" and e["event"].get("started", {}).get("name") == name for e in es))
        return next(e["session"] for e in entries if e["kind"] == "child" and e["event"].get("started", {}).get("name") == name)

    def reports(parent, sid):
        return [e for e in f.record(parent) if e["kind"] == "input"
                and isinstance(e["from"], dict) and e["from"].get("report", {}).get("child") == sid]

    def ended(sid, text):
        return server.until(f, sid, lambda es:
                            max((e["id"] for e in es if e["kind"] == "turn_end"), default=0) >
                            max((e["id"] for e in es if e["kind"] == "text" and e["text"] == text), default=float("inf")))

    # Nothing pending: the recorded prose, bounded at a UTF-8 boundary, is the report.
    parent = start(PLAIN)
    sid = child(parent, "plain40")
    server.until(f, parent, lambda _: bool(reports(parent, sid)))
    report = reports(parent, sid)
    assert len(report) == 1 and report[0]["from"]["report"]["status"] == "finished", report
    texts = [e for e in f.record(sid) if e["kind"] == "text"]
    assert [e["text"] for e in texts] == closing and len(long_text.encode()) > 65536, texts
    suffix = f"\nFull closing text: session {sid}, record entries {texts[0]['id']}–{texts[-1]['id']}.\n"
    expected = long_text.encode()[:65536 - len(suffix.encode())].decode("utf-8", "ignore") + suffix
    actual = report[0]["text"]
    assert actual == expected and len(actual.encode()) <= 65536, (len(actual.encode()), actual[:100], actual[-128:])
    assert len([e for e in f.record(sid) if e["kind"] == "turn_end"]) == 1
    ended(parent, "plain-report-received40")
    replies += [agent(PLAIN, "continue", child="plain40", brief="End as one long block40."),
                agent(PLAIN, "wait"), reply(PLAIN, text="single-report-received40"),
                reply(plain, text=closing[1])]
    server.post(f, f"/sessions/{parent}/input", {"text": "Continue plain40 with one block."})
    server.until(f, parent, lambda _: len(reports(parent, sid)) == 2)
    texts = [e for e in f.record(sid) if e["kind"] == "text"]
    assert len(texts) == 3 and texts[-1]["text"] == closing[1], texts
    suffix = f"\nFull closing text: session {sid}, record entry {texts[-1]['id']}.\n"
    expected = closing[1].encode()[:65536 - len(suffix.encode())].decode("utf-8", "ignore") + suffix
    assert reports(parent, sid)[-1]["text"] == expected

    # A live job holds completion; only its notice turn's closing text reaches the parent.
    parent = start(JOB)
    sid = child(parent, "job40")
    entries = ended(sid, "job-waiting40")
    assert any(e["kind"] == "job" and e["event"] == "started" for e in entries) and not reports(parent, sid), entries
    assert not any(e["kind"] == "job" and e["event"] == "ended" for e in entries), entries
    # It ends while the server is down: its notice still opens the turn that reports.
    server.stop(f)
    (f.cwd / "release40").touch()
    server.gone(next(e["pid"] for e in entries if e["kind"] == "job" and e["event"] == "started"))
    server.start(f)
    server.until(f, parent, lambda _: bool(reports(parent, sid)))
    assert [e["text"] for e in reports(parent, sid)] == ["job-closing40"]
    entries = list(f.record(sid))
    # Its notice is recorded before its end, so no stop between the two leaves the child idle.
    assert [(e["kind"], e.get("event", e.get("from"))) for e in entries if e["kind"] in ("input", "job")][-2:] == [("input", "razor"), ("job", "ended")], entries
    assert len([e for e in entries if e["kind"] == "turn_end"]) == 2, entries

    # No sibling reply: changing the real live setting shortens the existing grace.
    parent = start(MAIL)
    sid = child(parent, "mail40")
    entries = ended(sid, "mail-waiting40")
    assert any(e["kind"] == "mail_sent" for e in entries) and not reports(parent, sid), entries
    config = f.home / ".razor/config.toml"
    config.write_text(config.read_text().replace('grace = "1h"', 'grace = "1s"'))
    server.until(f, parent, lambda _: bool(reports(parent, sid)))
    entries = list(f.record(sid))
    nudges = [e for e in entries if e["kind"] == "input" and e["from"] == "razor"]
    assert len(nudges) == 1, entries
    assert [e["text"] for e in reports(parent, sid)] == ["mail-nudged-report40"]

    # Continue clears the nudge as well: sent mail must receive a fresh grace, not finish now.
    config.write_text(config.read_text().replace('grace = "1s"', 'grace = "1h"'))
    replies += [agent(MAIL, "continue", child="mail40", brief="A new task40: mention the sibling again."),
                reply(MAIL, text="continued-parent-ready40"), reply(MAIL, text="continued-report-received40"),
                message(mail, "note", to="channel", text="@sibling40 New task40."),
                reply(mail, text="continued-waiting40"), reply(mail, text="continued-nudged-report40"),
                reply(sibling, text="sibling-still-no-reply40")]
    server.post(f, f"/sessions/{parent}/input", {"text": "Continue mail40 on a new task."})
    ended(sid, "continued-waiting40")
    assert len(reports(parent, sid)) == 1, reports(parent, sid)
    config.write_text(config.read_text().replace('grace = "1h"', 'grace = "1s"'))
    server.until(f, parent, lambda _: len(reports(parent, sid)) == 2)
    assert reports(parent, sid)[-1]["text"] == "continued-nudged-report40"
    assert len([e for e in f.record(sid) if e["kind"] == "input" and e["from"] == "razor"]) == 2

    ended(parent, "continued-report-received40")
    replies += [agent(MAIL, "stop", child="sibling40"), reply(MAIL, text="mail-cleaned40")]
    server.post(f, f"/sessions/{parent}/input", {"text": "Stop the sibling's pending job."})
    ended(parent, "mail-cleaned40")

    # No job, question or queued input: a person's split turn alone forbids a report.
    config.write_text(config.read_text().replace('grace = "1s"', 'grace = "1h"'))
    parent = start(PERSON)
    sid = child(parent, "person40")
    ended(sid, "person-visible40")
    with Terminal(f, "--resume", parent) as t:
        t.shown("person-visible40", "person-parent-ready40")
        t.click("person-visible40")
        t.shown("talking to person40", "as you")
        t.send("Person asks for progress40.\r")
        t.shown("person-answer40")
        entries = ended(sid, "person-answer40")
        asks = [e for e in entries if e["kind"] == "input" and e["from"] == "person"]
        assert len(asks) == 1 and asks[0]["id"] in entries[-1]["inputs"], entries
        assert not any(e["kind"] in ("job", "question") or (e["kind"] == "mail_sent" and e["mail_kind"] == "question") for e in entries), entries
        answered = {i for e in entries if e["kind"] == "turn_end" for i in e["inputs"]}
        assert all(e["id"] in answered for e in entries if e["kind"] == "input"), entries
        time.sleep(1.2)
        assert not reports(parent, sid), reports(parent, sid)

    # Continue reaches the idle child that did not report, whatever its last turn's end.
    replies += [agent(PERSON, "continue", child="person40", brief="Again40."), reply(PERSON, text="person-continued40"),
                reply(PERSON, text="person-report40"), reply("Again40."), Reply(b'{"type": "error", "error": {"type": "permission_error", "message": "Failed40."}}', status=403, match=person)]
    server.post(f, f"/sessions/{parent}/input", {"text": "Continue person40."})
    ended(parent, "person-report40")
    # A failed turn that took the person's input does not report: its end carries no report.
    server.post(f, f"/sessions/{sid}/input", {"text": "Fail40."})
    end = server.until(f, sid, lambda es: "error" in es[-1].get("end", ""))[-1]
    assert "report" not in end and len(reports(parent, sid)) == 1, end

    # A turn ending in an error reports blocked at once with its sentence; continue runs it again.
    lost = response([{"type": "message", "id": "msg_lost40", "role": "assistant", "phase": "final_answer",
                      "content": [{"type": "output_text", "text": "lost40"}]}], completed=False)
    replies += [agent(ERROR, "start", name="error40", brief="Fail once.", model="gpt-6.1-sol", placement="in_place"),
                agent(ERROR, "wait"), reply(ERROR, text="error-report-received40"), *[replace(lost, match=error)] * 6]
    parent = start(ERROR)
    sid = child(parent, "error40")
    entries = ended(parent, "error-report-received40")
    blocked = "The model response did not complete; send another input to try again.\n\nContinue error40 to try again."
    wait = next(e["call"] for e in entries if e["kind"] == "call" and e["input"].get("action") == "wait")
    waited = json.loads(next(e["output"] for e in entries if e["kind"] == "result" and e["call"] == wait))
    assert waited["from"]["report"]["status"] == "blocked" and waited["text"] == blocked, waited
    replies += [agent(ERROR, "continue", child="error40", brief="Try again40."), agent(ERROR, "wait"),
                reply(ERROR, text="retry-report-received40"), replace(done(), match=error)]
    server.post(f, f"/sessions/{parent}/input", {"text": "Continue error40."})
    ended(parent, "retry-report-received40")
    assert [(r["from"]["report"]["status"], r["text"]) for r in reports(parent, sid)] == [
        ("blocked", blocked), ("finished", "flow done")], reports(parent, sid)
    ends = [e["end"] for e in f.record(sid) if e["kind"] == "turn_end"]
    assert ends == [{"error": blocked.split("\n")[0]}, "done"], ends

    # An error sentence past 64 KiB is cut so the report fits, the retry line kept.
    big = json.dumps({"type": "response.incomplete", "response": {"incomplete_details": {"reason": "🪒" * 20000}}}, ensure_ascii=False)
    replies += [agent(LONG, "start", name="long40", brief="Fail long.", model="gpt-6.1-sol", placement="in_place"),
                agent(LONG, "wait"), reply(LONG, text="long-report-received40"), Reply(f"data: {big}\n\n".encode(), match=long)]
    parent = start(LONG)
    sid = child(parent, "long40")
    ended(parent, "long-report-received40")
    line = "\n\nContinue long40 to try again."
    cut = f"The model's reply stopped early ({'🪒' * 20000}).".encode()[:65536 - len(line)].decode("utf-8", "ignore")
    assert [(r["from"]["report"]["status"], r["text"]) for r in reports(parent, sid)] == [("blocked", cut + line)]

    # A conversation that dies with a job's end notice queued runs that notice's turn, whose end reports.
    # The notice stays queued while a model reply streams (`agent wait` would take it); the CLI is
    # stopped before it reads that reply's end and killed once the stand-in has sent it.
    (f.cwd / "crash40").mkdir()
    replies += [agent(CRASH, "start", name="crash40", brief="Wait on a job.", cwd="crash40", placement="in_place"),
                agent(CRASH, "wait"), reply(CRASH, text="crash-report-received40"),
                reply(crash, "Bash", {"command": "while [ ! -f go40 ]; do sleep 0.05; done", "run_in_background": True}),
                replace(reply(crash, text="unread40"), cuts=tuple(range(5, 850, 5)))]  # about 5 s
    parent = start(CRASH)
    sid = child(parent, "crash40")
    server.until(f, sid, lambda es: any(e["kind"] == "result" for e in es))
    replies.append(reply(f"{sid}/job-1</output-file>", text="crash-recovered40"))
    (f.cwd / "crash40/go40").touch()
    server.until(f, sid, lambda es: any(e["kind"] == "job" and e["event"] == "ended" for e in es))
    # The child's Claude Code process works in crash40.
    pids = [int(proc.name) for proc in Path("/proc").glob("[0-9]*") if os.path.realpath(proc / "cwd") == str((f.cwd / "crash40").resolve())]
    for sig in signal.SIGSTOP, signal.SIGKILL:
        for pid in pids:
            os.kill(pid, sig)
        server.until(f, sid, lambda _: all('"sent"' in p.read_text() for p in f.requests.glob("[0-9]*.json")))
    ended(parent, "crash-report-received40")
    assert [(r["from"]["report"]["status"], r["text"]) for r in reports(parent, sid)] == [("finished", "crash-recovered40")]
    assert len([e for e in f.record(sid) if e["kind"] == "turn_end" and "error" in e["end"]]) == 1

    # A report survives a stop the moment its child's turn end is written. Words the person sent
    # as the child streamed its finish wait for its next task, which the person's first message
    # after the restart opens with both.
    replies += [agent(HELD, "start", name="held40", brief="Finish at once.", placement="in_place"),
                reply(HELD, text="held-parent-ready40"), reply(HELD, text="held-report-heard40"),
                replace(agent(held, "finish", status="finished", report="held-report40"), cuts=tuple(range(1, 60))),
                reply("HELD_LATER40", text="held-answer40")]
    parent = start(HELD)
    sid = child(parent, "held40")
    server.wait(lambda: any(held in p.read_text() for p in f.requests.glob("*.body")))
    early = server.post(f, f"/sessions/{sid}/input", {"text": "HELD_EARLY40"})["entry"]
    while b'"kind":"turn_end"' not in (f.home / ".razor/sessions" / sid / "record.jsonl").read_bytes():
        pass
    os.kill(json.loads((f.home / ".razor/server.json").read_text())["pid"], signal.SIGSTOP)
    server.stop(f, signal.SIGKILL)
    server.start(f)
    ended(parent, "held-report-heard40")
    assert [r["text"] for r in reports(parent, sid)] == ["held-report40"], reports(parent, sid)
    later = server.post(f, f"/sessions/{sid}/input", {"text": "HELD_LATER40"})["entry"]
    assert {early, later} <= set(ended(sid, "held-answer40")[-1]["inputs"])
