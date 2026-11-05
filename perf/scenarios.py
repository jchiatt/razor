#!/usr/bin/env python3
"""Razor's repeatable performance scenarios (docs/design/performance-harness.md): the binary
--binary names does ordinary work (file reads, short shell commands, code-mode cells, children,
stored-session startup, streamed text) in a fresh scratch home per run, against flows/standin.py,
on the Responses or the Claude route. One JSON file holds every sample with its unit, the work
done with every tool result checked whole, and process and machine state at the edges of each
measured window.

The stand-in is a loopback replay of response bytes, not a provider and not a network: nothing
here separates provider time from network time or says how a live provider behaves. The
stand-in, the API client, the PTY reader and pyte share this Python process (one GIL), so each
window records this process's own CPU beside Razor's. Run it on a quiet machine under an
exclusive-benchmark wrapper; it takes no lock and adds no load of its own. Python 3.10 or later,
as flows/ needs; the flows' environment (flows/requirements.txt) for pyte.

  perf/scenarios.py --binary B --binary-revision REV --output O.json [--route claude] [--scenario NAME ...]
  perf/scenarios.py --binary B --binary-revision unknown --output O.json --validate
"""
import argparse
import contextlib
from dataclasses import replace
import hashlib
import io
import itertools
import json
import os
from pathlib import Path
import platform
import re
import resource
import shlex
import statistics
import subprocess
import sys
import tempfile
import time
import traceback
from contextlib import contextmanager
from datetime import datetime, timezone
from urllib.parse import urlsplit

# One row per scenario: what a measured turn does (`kind`), the history under it (a SHAPES key) and
# the order of base (A) and planted (B) turns where a delay is planted.
SCENARIOS = {
    "read": dict(kind="read", history="short"),
    "shell": dict(kind="shell", history="short"),
    "exec": dict(kind="exec", history="short"),
    "history-long": dict(kind="read", history="long"),
    "child-in-place": dict(kind="child", placement="in_place"),
    "child-worktree": dict(kind="child", placement="worktree"),
    "startup": dict(kind="startup"),
    "stream": dict(kind="rendered"),
    "validate-tool": dict(kind="shell", history="short", order="ABBA", planted="tool"),
    "validate-stream": dict(kind="stream", history="short", order="ABBA", planted="stream"),
}
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("--binary", required=True, help="the razor build to measure")
ap.add_argument("--binary-revision", required=True, help="the source revision the binary was built from, or `unknown`")
ap.add_argument("--output", required=True, help="the JSON file, outside the checkout")
ap.add_argument("--repo", help="the checkout whose flows/ and perf/workloads.py to use (default: this file's)")
ap.add_argument("--route", choices=["responses", "claude"], default="responses")
ap.add_argument("--scenario", action="append", choices=SCENARIOS, help="repeatable; default: all but validate-*")
ap.add_argument("--validate", action="store_true", help="run only the validate-* scenarios")
ap.add_argument("--profile", choices=["tuned", "heldout", "smoke"], default="tuned")
ap.add_argument("--runs", type=int, default=1, help="fresh scratch homes and servers per scenario")
ap.add_argument("--scratch-root", help="where scratch homes go; the only way to choose tmpfs")
ap.add_argument("--expect-sha256", help="refuse a binary with another hash")
args = ap.parse_args()
repo = Path(args.repo or Path(__file__).resolve().parents[1]).resolve()
if not (repo / "flows/driver.py").is_file() or not (repo / "perf/workloads.py").is_file():
    ap.error(f"{repo} holds no flows/driver.py and perf/workloads.py; pass --repo")
if Path(args.output).resolve().is_relative_to(repo):
    ap.error("--output must be outside the checkout")
os.environ["RAZOR_BIN"] = args.binary  # flows/driver.py reads it when imported
sys.path[:0] = [str(repo / "flows"), str(repo / "perf")]
import claude  # noqa: E402
from driver import REPLIES, flow  # noqa: E402
from responses import PLAIN, call, response  # noqa: E402
from standin import Reply  # noqa: E402
import server  # noqa: E402
from terminal import Terminal  # noqa: E402
from workloads import ANSWER, LINE, PAGE, TURNS, answer_text, history, message, page_text, turn  # noqa: E402

# The model each route's config names, the path its requests go to, the config a plain and a
# code-mode session runs with, and the names Razor's tools have there. The Responses alias's wire
# id is Razor's own table's (src/profiles/models.rs); the Claude CLI resolves its alias itself, so
# its id is read from the requests.
ROUTES = {
    "responses": dict(model="sol", endpoint="/responses", wire="gpt-6.1-sol", plain=PLAIN, code="[profiles.codex]\ncode_mode = true\n", path="path",
                      names={"read": "read", "bash": "bash", "agent": "agent", "exec": "exec"}),
    "claude": dict(model="opus", endpoint="/v1/messages", wire=None, plain="", code="[profiles.claude]\ncode_mode = true\n", path="file_path",
                   names={"read": "Read", "bash": "Bash", "agent": "mcp__razor__agent", "exec": "mcp__razor__exec"}),
}
ROUTE, CLAUDE = ROUTES[args.route], args.route == "claude"
# Work counts per profile. `tuned` history is the gate's long session (perf/workloads.py);
# `heldout` differs from `tuned` in every count and in its wording.
SHAPES = {
    "tuned": dict(short=4, long=TURNS, page=PAGE, answer=ANSWER, text=80, calls=3, turns=10, kids=8,
                  stored=700, words=24, files=150, line=LINE),
    "heldout": dict(short=7, long=97, page=37, answer=21, text=400, calls=4, turns=8, kids=6,
                    stored=1100, words=40, files=400,
                    line="Held out of tuning: these words, this length and this count were never measured before."),
    "smoke": dict(short=2, long=4, page=20, answer=5, text=40, calls=2, turns=3, kids=2,
                  stored=20, words=6, files=20, line="Smoke page, never reported."),
}
TICK = os.sysconf("SC_CLK_TCK") if sys.platform == "linux" else 100
COMPILERS = {"rustc", "cargo", "cc", "cc1", "cc1plus", "gcc", "g++", "c++", "clang", "clang++", "ld", "ld.lld", "lld", "mold"}
DEFINITIONS = {
    "input_to_arrival": "just before the API input is sent to the stand-in reading the next model request's headers",
    "tool_gap": "last byte of a response holding a call to the arrival of the next request (the tool, Razor's recording and rendering)",
    "endpoint_span": "the stand-in's first slice of a reply to its last byte written, including planted 30 ms pauses",
    "standin_handling": "request headers read to the reply's first slice: the stand-in's own body read and spool, not Razor",
    "final_to_turn_end": "last byte of the turn's last response to turn_end reaching an API client attached to the session",
    "turn_total": "just before the input is sent to turn_end reaching the attached client",
    "child_start": "end of the start call's response to the child's first request arriving (worktree creation included)",
    "start_result_to_next_request": "end of the start call's response to the parent's next request",
    "child_report_to_parent_request": "end of the child's finish response to the parent's request after its wait ends",
    "settle": "end of the settle call's response to the parent's next request (worktree removal included)",
    "cold_to_first_request": "just before forking `razor --print` with no server running to the first model request's headers",
    "cold_total": "the same client from fork to exit",
    "client_cpu": "that client's CPU seconds and those of what it waited for, from this process's waited-children counters",
    "server_cpu_since_start": "the CPU seconds of the server that client started, from its start to just after the client exited",
    "server_rss_after_client": "that server's resident KiB just after the client exited",
    "warm_create": "an API POST /sessions on a warm server, round trip from this process",
    "delta_to_frame": "the stand-in's slice before writing one streamed word to the receipt of the first whole terminal frame showing it",
    "first_delta_to_frame": "the same for a reply's first word",
    "input_to_first_frame": "just before the API input to the first whole terminal frame showing the reply's first word",
}


def sh(*argv):
    return subprocess.check_output(argv, text=True)


def clock(text):
    """ps TIME, [[h:]m:]s.cc, in seconds."""
    return sum(float(part) * 60 ** i for i, part in enumerate(reversed(text.split(":"))))


def usage(pid):
    """One process's CPU seconds and memory in KiB. Linux: utime+stime from /proc/<pid>/stat
    (ticks), cutime+cstime (the CPU of its children it has waited for, and theirs, as the kernel
    propagates it), each live thread's exact on-CPU ns from task/*/schedstat, VmRSS, RssAnon, VmHWM
    (the lifetime peak) and Pss/Pss_Anon from smaps_rollup where readable. macOS: `ps` TIME (10 ms)
    and RSS only: it has no waited-children, anonymous or proportional figure, and none is invented."""
    if sys.platform != "linux":
        rss, cpu = sh("ps", "-o", "rss=,time=", "-p", str(pid)).split()
        return {"cpu_s": clock(cpu), "cpu_precision_s": 0.01, "rss_kib": int(rss)}
    stat = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    status = dict(line.split(":", 1) for line in Path(f"/proc/{pid}/status").read_text().splitlines())
    kib = lambda key: int(status[key].split()[0])
    out = {"cpu_s": (int(stat[11]) + int(stat[12])) / TICK, "waited_children_cpu_s": (int(stat[13]) + int(stat[14])) / TICK,
           "cpu_precision_s": 1 / TICK, "rss_kib": kib("VmRSS"), "rss_anon_kib": kib("RssAnon"),
           "rss_lifetime_peak_kib": kib("VmHWM"), "threads_oncpu_ns": {}}
    for task in Path(f"/proc/{pid}/task").iterdir():
        with contextlib.suppress(OSError):  # a thread that exits mid-read
            out["threads_oncpu_ns"][task.name] = int((task / "schedstat").read_text().split()[0])
    with contextlib.suppress(OSError):
        rollup = {k: int(v.split()[0]) for k, v in (line.split(":", 1) for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines()[1:])}
        out |= {"pss_kib": rollup["Pss"], "pss_anon_kib": rollup.get("Pss_Anon")}
    return out


def cpu_between(a, b):
    """CPU seconds between two usages of one process: the tick counts' difference and, on Linux,
    its waited children's and the summed per-thread on-CPU time; threads_gone counts threads that
    ended in between, whose time is then missing from oncpu_s (a lower bound)."""
    out = {"cpu_s": b["cpu_s"] - a["cpu_s"], "cpu_precision_s": a["cpu_precision_s"]}
    if "threads_oncpu_ns" in a:
        before, after = a["threads_oncpu_ns"], b["threads_oncpu_ns"]
        out |= {"waited_children_cpu_s": b["waited_children_cpu_s"] - a["waited_children_cpu_s"],
                "oncpu_s": sum(ns - before.get(tid, 0) for tid, ns in after.items()) / 1e9,
                "threads_gone": len(before.keys() - after.keys())}
    return out


def snapshot(named=None):
    """The machine's load and compiler count now, and each named process (the server, an attached
    terminal) and every descendant of theirs alive now. A descendant that came and went between
    two snapshots is in neither."""
    rows = [(int(p), int(pp), (rest or [""])[0]) for p, pp, *rest in (line.split(None, 2) for line in sh("ps", "-axwwo", "pid=,ppid=,args=").splitlines())]
    out = {"monotonic_s": time.monotonic(), "load1": os.getloadavg()[0],
           "compilers": sum(Path(argv.split(" ", 1)[0]).name in COMPILERS for *_, argv in rows)}
    if named:
        members, grown = set(named.values()), True
        while grown:
            found = {p for p, parent, _ in rows if parent in members} - members
            members, grown = members | found, bool(found)
        out["processes"] = {name: usage(pid) for name, pid in named.items()}
        out["descendants"] = []
        for p, _, argv in rows:
            if p in members - set(named.values()):
                with contextlib.suppress(OSError, ValueError, KeyError, subprocess.CalledProcessError):  # gone, or exited unwaited
                    out["descendants"].append({"pid": p, "args": argv, **{k: v for k, v in usage(p).items() if k != "threads_oncpu_ns"}})
    return out


def rusage_cpu(a, b):
    return (b.ru_utime - a.ru_utime) + (b.ru_stime - a.ru_stime)


@contextmanager
def window(rec, **named):
    """The measured work's edges: machine and process state taken before and after it, never in
    it, with the CPU of the processes this one waited for and of this process itself in between."""
    start = snapshot(named)
    kids, me = resource.getrusage(resource.RUSAGE_CHILDREN), resource.getrusage(resource.RUSAGE_SELF)
    yield
    kids2, me2 = resource.getrusage(resource.RUSAGE_CHILDREN), resource.getrusage(resource.RUSAGE_SELF)
    end = snapshot(named)
    both = {(d["pid"], d["args"]): d for d in start.get("descendants", [])}
    rec["window"] = {"start": start, "end": end, "seconds": end["monotonic_s"] - start["monotonic_s"],
                     "clients_cpu_s": rusage_cpu(kids, kids2), "harness_cpu_s": rusage_cpu(me, me2),
                     "cpu": {name: cpu_between(start["processes"][name], end["processes"][name]) for name in named},
                     "descendants_at_both_edges_cpu_s": [{"pid": d["pid"], "args": d["args"], "cpu_s": d["cpu_s"] - both[d["pid"], d["args"]]["cpu_s"]}
                                                         for d in end.get("descendants", []) if (d["pid"], d["args"]) in both]}


class Timed(bytes):
    """A reply body that notes the time before each slice the stand-in writes (the endpoint's
    emission of that part), with what the reply is: its label, the check each of its calls'
    results must pass, and the text it ends the turn with."""

    def __new__(cls, data, label, checks=(), text=None):
        self = super().__new__(cls, data)
        self.label, self.checks, self.text, self.marks = label, checks, text, []
        return self

    def __getitem__(self, key):
        self.marks.append(time.monotonic())
        return super().__getitem__(key)


_ids = itertools.count()


def tool(label, name, arguments, check, match=None, inner=()):
    """A reply holding one call of Razor's tool `name`, by the route's name for it. `check` is
    what the call's result must pass and `inner` what a cell's nested calls' results must, in
    order."""
    n = next(_ids)
    id, name = (f"toolu_perf_{n}" if CLAUDE else f"call_{n}"), ROUTE["names"][name]
    if CLAUDE:
        reply = claude.reply((id, name, arguments), match=match)
    elif name == "exec":
        reply = replace(response([{"type": "custom_tool_call", "id": f"ct_{n}", "call_id": id, "name": name, "input": arguments["code"]}]), match=match)
    else:
        reply = replace(response([call(name, json.dumps(arguments), id)]), match=match)
    checks = ((id, check), *((f"{id}:{k}", c) for k, c in enumerate(inner, 1)))
    return replace(reply, body=Timed(reply.body, label, checks))


def say(label, text, match=None):
    reply = claude.reply(text=text, match=match) if CLAUDE else replace(response([message(text, next(_ids))]), match=match)
    return replace(reply, body=Timed(reply.body, label, text=text))


def stream(label, words, cut):
    """An answer streamed one word per delta; with `cut` the stand-in pauses 30 ms after each."""
    text, parts = " ".join(words), [words[0], *(" " + w for w in words[1:])]
    if CLAUDE:  # the recorded message's one text delta, once per word
        events = claude.reply(text=text).body.split(b"\n\n")
        at = next(i for i, e in enumerate(events) if b'"text_delta"' in e)
        kind, data = events[at].split(b"\ndata: ", 1)
        delta = json.loads(data)
        deltas = [kind + b"\ndata: " + json.dumps({**delta, "delta": {**delta["delta"], "text": p}}).encode() + b"\n\n" for p in parts]
        head, tail = b"".join(e + b"\n\n" for e in events[:at]), b"".join(e + b"\n\n" for e in events[at + 1:] if e)
    else:
        item = message(text, next(_ids))
        marker = b'data: {"type": "response.output_item.done"'
        head, tail = response([item]).body.split(marker, 1)
        deltas = [f"data: {json.dumps({'type': 'response.output_text.delta', 'item_id': item['id'], 'output_index': 0, 'content_index': 0, 'delta': p})}\n\n".encode() for p in parts]
        tail = marker + tail
    pieces = [head + deltas[0], *deltas[1:], tail]
    return Reply(Timed(b"".join(pieces), label, text=text), tuple(itertools.accumulate(map(len, pieces[:-1]))) if cut else ())


def job(text):
    """A shell job's result as Razor's own tools give it: its JSON, ended with exit 0, holding `text`."""
    return lambda output: (j := json.loads(output))["output"] == text and j["end"] == {"exit": 0}


def one_call(kind, page, shape, command="printf perf-ok"):
    """A reply holding one call of `kind`, checked whole: a read of `page`, a shell command, or a
    code-mode cell running a shell command and a read of `page` through the shell."""
    text = page_text(shape["page"], shape["line"])
    if kind == "shell":  # Claude's plain Bash gets Claude Code's result shape
        check = (lambda output: output == "Exit code 0\nperf-ok") if CLAUDE else job("perf-ok")
        return tool("bash", "bash", {"command": command}, check)
    if kind == "read":  # Claude's Read is Claude Code's own: every line, in order, in its framing
        lines = [f"{i}: {shape['line']}" for i in range(shape["page"])]
        pattern = rf"(?<!\d)(\d+): {re.escape(shape['line'])}$"
        check = ((lambda output: re.findall(pattern, output, re.M) == [str(i) for i in range(shape["page"])]) if CLAUDE else
                 (lambda output: json.loads(output) == {"path": str(page), "first": 1, "lines": lines, "next": None}))
        return tool("read", "read", {ROUTE["path"]: str(page)}, check)
    # In code mode `read` is the image viewer; text is read through the shell, as Codex does.
    source = ('text(JSON.stringify(await tools.exec_command({cmd:"printf perf-ok"})));'
              'text(JSON.stringify(await tools.exec_command({cmd:' + json.dumps(f"cat {shlex.quote(str(page))}") + '})));')

    def cell(output):  # Codex's status line, then each text() line
        status, *lines = output.split("\n")
        return status == "Script completed" and lines[-1] == "" and [json.loads(line)["output"] for line in lines[:-1]] == ["perf-ok", text]
    return tool("exec", "exec", {"code": source}, cell, inner=(job("perf-ok"), job(text)))


def checked(entries, replies):
    """Checks every call's result against its own call's check: none missing, none extra, each a
    success holding its whole expected output. Returns how many were checked."""
    expected = dict(check for r in replies for check in r.body.checks)
    every = [e for e in entries if e["kind"] == "result"]
    results = {e["call"]: e for e in every}
    assert len(every) == len(results), f"a call has more than one result: {sorted(e['call'] for e in every)}"
    assert results.keys() == expected.keys(), f"results without a check or checks without a result: {sorted(results.keys() ^ expected.keys())}"
    for id, check in expected.items():
        with contextlib.suppress(ValueError, KeyError, TypeError):
            if results[id]["outcome"] == "success" and check(results[id]["output"]):
                continue
        raise AssertionError(f"result {id} is not the expected whole output: {results[id]}")
    return len(results)


def meta(path):
    try:
        return json.loads(path.read_text())
    except ValueError:  # read between the stand-in's truncation and its write
        return {}


def spool(f, replies):
    """Every model request's metadata, in order, with the reply that answered it (the replies'
    first slices in time order: the stand-in serves one request at a time), and the wire model
    every request named: on Claude, the model each session's CLI reported for the alias. Waits for
    each reply's last byte to be recorded as sent."""
    paths = sorted(f.requests.glob("[0-9]*.json"), key=lambda p: int(p.stem))
    assert len(paths) == len(replies) and all(r.body.marks for r in replies), (len(paths), len(replies))
    reqs = [server.wait(lambda p=p: (m := meta(p)).get("sent") and m, 10, f"{p.name} never recorded its reply as sent") for p in paths]
    sent = {(m["method"], urlsplit(m["path"]).path) for m in reqs}  # the Claude CLI adds a query, such as ?beta=true
    assert sent == {("POST", ROUTE["endpoint"])}, f"requests went to {sorted(sent)}"
    served = [json.loads(p.with_suffix(".body").read_bytes())["model"] for p in paths]
    wire = ROUTE["wire"] or served[0]
    assert set(served) == {wire}, f"requests were served as {sorted(set(served))}"
    if CLAUDE:
        reported = set(re.findall(r"Claude Code \S+, model (\S+),", (f.home / ".razor/server.log").read_text()))
        assert reported == {wire} and wire.startswith("claude-") and ROUTE["model"] in wire, f"the CLI reported {reported}; requests named {wire}"
    return reqs, sorted((r.body for r in replies), key=lambda b: b.marks[0]), wire


def put(m, name, value, unit="ms"):
    m.setdefault(name, {"unit": unit, "definition": DEFINITIONS.get(re.split(r"[:\[]", name)[0]), "samples": []})
    m[name]["samples"].append(value * 1000 if unit == "ms" else value)


def texts(entries):
    return [e["text"] for e in entries if e["kind"] == "text"]


def finished(entries, count):
    ends = [e for e in entries if e["kind"] == "turn_end"]
    assert len(ends) == count and all(e["end"] == "done" for e in ends), ends[-3:]


def turn_replies(kind, variant, shape, page):
    """The replies of one measured turn: the planted variant B pauses the answer's stream, or adds
    40 ms to the same shell command."""
    if kind == "stream":
        return [stream("answer", [f"w{k}" for k in range(shape["words"])], variant == "B")]
    command = "sleep 0.04; " * (variant == "B") + "printf perf-ok"
    return [one_call(kind, page, shape, command) for _ in range(shape["calls"])] + [say("answer", answer_text(3, shape["line"]))]


def conversation(rec, f, replies, shape, row):
    """One session with `history` turns of a read and an answer behind it, then the measured
    turns: input, calls of one kind, answer, each segment timed against the stand-in's clocks."""
    kind, order = row["kind"], row.get("order", "A")
    page = f.cwd / "page.txt"
    page.write_text(page_text(shape["page"], shape["line"]))
    pid = server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    setup = lambda t: [one_call("exec" if kind == "exec" else "read", page, shape), say("setup", answer_text(shape["answer"], shape["line"]))]
    last = history(f, session, replies, setup, shape[row["history"]])
    time.sleep(1)  # maintenance after the last setup turn settles before the window opens
    turns, count = [], -(-shape["turns"] // len(order)) * len(order)
    with window(rec, server=pid):
        for t in range(count):
            variant = order[t % len(order)]
            step = turn_replies(kind, variant, shape, page)
            first = len(replies)
            replies += step
            text = (f"Measured turn {t}: " + "read the page and answer. " * 40)[:shape["text"]]
            sent, ended, last = turn(f, session, last, text)
            turns.append((variant, first, len(step), sent, ended))
    reqs, used, wire = spool(f, replies)
    m, tag = {}, lambda v: "" if order == "A" else {"A": "[base]", "B": "[planted]"}[v]
    for variant, a, k, sent, ended in turns:
        put(m, "input_to_arrival" + tag(variant), reqs[a]["arrived"] - sent)
        put(m, "final_to_turn_end" + tag(variant), ended - reqs[a + k - 1]["sent"])
        put(m, "turn_total" + tag(variant), ended - sent)
        for i in range(a, a + k):
            put(m, f"endpoint_span:{used[i].label}" + tag(variant), reqs[i]["sent"] - used[i].marks[0])
            put(m, "standin_handling" + tag(variant), used[i].marks[0] - reqs[i]["arrived"])
            if i + 1 < a + k:
                put(m, f"tool_gap:{used[i].label}" + tag(variant), reqs[i + 1]["arrived"] - reqs[i]["sent"])
    entries = list(f.record(session))
    results = checked(entries, replies)
    assert texts(entries) == [r.body.text for r in replies if r.body.text is not None], texts(entries)[-2:]
    finished(entries, shape[row["history"]] + len(turns))
    rec["work"] = {"history_turns": shape[row["history"]], "measured_turns": len(turns), "calls_per_turn": shape["calls"] * (kind != "stream"),
                   "results_checked": results, "requests": len(reqs), "last_request_bytes": (f.requests / f"{len(reqs):03}.body").stat().st_size,
                   "record_entries": len(entries), "record_bytes": (f.home / ".razor/sessions" / session / "record.jsonl").stat().st_size,
                   "wire_model": wire}
    rec["metrics"] = m
    if "planted" in row:
        validate(rec, m, shape, row)


def validate(rec, m, shape, row):
    """The planted delay must register in the segment it was planted in, by about what was planted,
    and in no other segment. Tolerance -3 to +10 ms: a sleep is never short, and the process and
    scheduling around it add a few ms."""
    key, expected = ("tool_gap:bash", 40.0) if row["planted"] == "tool" else ("endpoint_span:answer", 30.0 * shape["words"])
    med = lambda name: statistics.median(m[name]["samples"])
    diff = lambda name: med(name + "[planted]") - med(name + "[base]")
    others = {name[:-6]: diff(name[:-6]) for name in m if name.endswith("[base]")}
    observed = others.pop(key)
    total = others.pop("turn_total")
    rec["validation"] = {"segment": key, "expected_ms": expected, "observed_ms": observed, "tolerance_ms": [-3, 10],
                         "turn_total_added_ms": total, "unplanted_added_ms": others, "unplanted_limit_ms": 10,
                         "medians_of": {k: len(m[k]["samples"]) for k in (key + "[base]", key + "[planted]")}}
    assert -3 <= observed - expected <= 10, f"{key} grew {observed:.1f} ms, planted {expected:.0f}"
    assert all(abs(d) <= 10 for d in others.values()), f"a segment other than {key} moved: {others}"


def children(rec, f, replies, shape, row):
    """A parent turn that starts a child, waits for its report and settles it, `kids` times, in a
    git checkout of `files` small files; the child's own request is found by its contract's words."""
    root, worktree = "PERF_PARENT", row["placement"] == "worktree"
    for i in range(shape["files"]):
        path = f.cwd / "src" / f"d{i % 10}" / f"f{i}.txt"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(("x" * 60 + "\n") * 30)
    f.base("src")
    pid = server.start(f)
    session, last, turns = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"], 1, []
    with window(rec, server=pid):
        for i in range(shape["kids"]):
            name, first, report = f"kid{i}", len(replies), f"Child {i} is done."
            replies += [tool("agent_start", "agent", {"action": "start", "name": name, "brief": "Finish at once.", "placement": row["placement"]},
                             lambda output: bool(json.loads(output)["branch"]) == worktree, root),
                        tool("agent_wait", "agent", {"action": "wait"}, lambda output, r=report: r in output, root),
                        tool("agent_settle", "agent", {"action": "settle", "child": name, "decision": "discard"}, lambda output: "text" in json.loads(output), root),
                        say("answer", "Done.", root),
                        tool("agent_finish", "agent", {"action": "finish", "status": "finished", "report": report}, lambda output: json.loads(output) == {}, f"You are {name},")]
            sent, ended, last = turn(f, session, last, f"{root}: start child {i}.")
            turns.append((first, sent, ended))
    reqs, used, wire = spool(f, replies)
    m = {}
    for first, sent, ended in turns:
        at = {used[j].label: j for j in range(first, first + 5)}
        s, w, c, d, e = (at[label] for label in ("agent_start", "agent_wait", "agent_finish", "agent_settle", "answer"))
        put(m, "input_to_arrival", reqs[s]["arrived"] - sent)
        put(m, "child_start", reqs[c]["arrived"] - reqs[s]["sent"])
        put(m, "start_result_to_next_request", reqs[w]["arrived"] - reqs[s]["sent"])
        put(m, "child_report_to_parent_request", reqs[d]["arrived"] - reqs[c]["sent"])
        put(m, "settle", reqs[e]["arrived"] - reqs[d]["sent"])
        put(m, "final_to_turn_end", ended - reqs[e]["sent"])
        put(m, "turn_total", ended - sent)
    parent = list(f.record(session))
    kids = [entries for p in (f.home / ".razor/sessions").glob("*/record.jsonl") if (entries := list(f.record(p.parent.name)))[0].get("child")]
    results = checked(parent + [e for entries in kids for e in entries], replies)
    starts = [json.loads(e["output"]) for e in parent if e["kind"] == "result" and e["call"] in {r.body.checks[0][0] for r in replies if r.body.label == "agent_start"}]
    assert len(kids) == len(turns) and all(bool(k[0]["child"].get("worktree")) == worktree for k in kids), [k[0] for k in kids[:1]]
    assert not any(Path(s["cwd"]).exists() for s in starts if worktree), "a discarded child's worktree is still there"
    assert texts(parent) == ["Done."] * len(turns), texts(parent)
    finished(parent, len(turns))
    for k in kids:
        finished(k, 1)
    rec["work"] = {"children": len(turns), "placement": row["placement"], "repo_files": shape["files"], "results_checked": results,
                   "requests": len(reqs), "wire_model": wire}
    rec["metrics"] = m


def startup(rec, f, replies, shape, row):
    """`razor --print` against a stopped server, `turns` times, then warm session creates, with
    `stored` sessions already in the home: copies of one Start line the product wrote, with
    distinct ids, names and ages over 14 days (their text is not any real session's). Each
    restart's server and its descendants are sampled once, just after its client exits: their
    usage since their own start."""
    server.start(f)
    sessions = f.home / ".razor/sessions"
    template = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    first, now = json.loads((sessions / template / "record.jsonl").read_bytes().split(b"\n")[0]), time.time()
    for i in range(shape["stored"]):
        at = now - i * 14 * 86400 / shape["stored"]
        folder = sessions / (datetime.fromtimestamp(at, timezone.utc).strftime("%Y%m%d-%H%M%S") + f"-{i:04x}")
        folder.mkdir()
        (folder / "record.jsonl").write_text(json.dumps({**first, "at": int(at * 1000), "name": f"fixture-{i}"}) + "\n")
        os.utime(folder / "record.jsonl", (at, at))
    stored, m, rec["restarts"], printed = len(list(sessions.iterdir())), {}, [], []
    with window(rec):
        for r in range(shape["turns"]):
            server.stop(f)
            replies.append(say("answer", "Started."))
            kids = resource.getrusage(resource.RUSAGE_CHILDREN)
            before = time.monotonic()
            done = f.run("--print=json", f"Startup {r}.")
            total = time.monotonic() - before
            put(m, "client_cpu", rusage_cpu(kids, resource.getrusage(resource.RUSAGE_CHILDREN)), "s")
            assert done.returncode == 0, done.stderr
            printed.append(json.loads(done.stdout))
            arrived = server.wait(lambda: meta(f.requests / f"{len(replies):03}.json").get("arrived"), 10, "no model request")
            put(m, "cold_to_first_request", arrived - before)
            put(m, "cold_total", total)
            rec["restarts"].append(snapshot({"server": json.loads((f.home / ".razor/server.json").read_text())["pid"]}))
            put(m, "server_cpu_since_start", rec["restarts"][-1]["processes"]["server"]["cpu_s"], "s")
            put(m, "server_rss_after_client", rec["restarts"][-1]["processes"]["server"]["rss_kib"], "KiB")
        for _ in range(2 * shape["turns"]):
            before = time.monotonic()
            server.post(f, "/sessions", {"cwd": str(f.cwd)})
            put(m, "warm_create", time.monotonic() - before)
    *_, wire = spool(f, replies)
    assert [p["message"] for p in printed] == ["Started."] * len(printed), printed
    for p in printed:
        finished(list(f.record(p["session"])), 1)
    rec["work"] = {"stored_sessions_at_start": stored, "stored_sessions_at_end": len(list(sessions.iterdir())), "restarts": shape["turns"],
                   "creates": 2 * shape["turns"], "wire_model": wire}
    rec["metrics"] = m


def rendered(rec, f, replies, shape, row):
    """Streamed words, 30 ms apart at the stand-in, until each shows in a whole frame of a
    terminal attached to the session: the stand-in's slice stamp to the frame's receipt."""
    pid = server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    replies.append(say("setup", "Ready."))
    turn(f, session, 1, "Setup.")
    m = {}
    time.sleep(1)
    with Terminal(f, "--resume", session) as t:
        t.shown("Ready.", "Message the agent")
        with window(rec, server=pid, terminal=t.pid):
            for r in range(shape["turns"]):
                words = [f"r{r}w{k:02d}" for k in range(shape["words"])]
                replies.append(stream("answer", words, True))
                sent = time.monotonic()
                server.post(f, f"/sessions/{session}/input", {"text": f"Stream {r}."})
                frames = [t.shown(w) for w in words]
                marks = replies[-1].body.marks
                assert all(frame > mark for frame, mark in zip(frames, marks)), "a frame was received before its word was sent"
                for frame, mark in zip(frames, marks):
                    put(m, "delta_to_frame", frame - mark)
                put(m, "first_delta_to_frame", frames[0] - marks[0])
                put(m, "input_to_first_frame", frames[0] - sent)
                server.until(f, session, lambda entries: entries[-1]["kind"] == "turn_end")
    *_, wire = spool(f, replies)
    entries = list(f.record(session))
    assert texts(entries) == [r.body.text for r in replies], texts(entries)[-2:]
    finished(entries, 1 + shape["turns"])
    rec["work"] = {"streams": shape["turns"], "words_per_stream": shape["words"], "pause_ms": 30, "wire_model": wire}
    rec["metrics"] = m


DRIVERS = {"read": conversation, "shell": conversation, "exec": conversation, "stream": conversation,
           "child": children, "startup": startup, "rendered": rendered}


def fstype(path):
    if sys.platform == "linux":
        return sh("stat", "-f", "-c", "%T", path).strip()
    mount = sh("df", "-P", path).splitlines()[-1].split(None, 5)[-1]
    return re.search(rf" on {re.escape(mount)} \((\w+)", sh("mount")).group(1)


def cgroup(name):
    """Each cgroup v2 file `name` (a limit) from this process's cgroup up to the root, where one
    exists; None off Linux."""
    with contextlib.suppress(OSError, IndexError):
        path = Path("/sys/fs/cgroup" + Path("/proc/self/cgroup").read_text().split("::", 1)[1].strip())
        return {str(p): (p / name).read_text().strip() for p in (path, *path.parents) if (p / name).is_file()}


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def bridge(rec):
    """The Claude bridge a run's server started, found by its process's arguments at an edge: its
    files' hashes, whether its installed packages are its lock's, the SDK and CLI package versions
    installed there, its Node, and the Claude Code version and model each session reported."""
    snaps = [rec["window"]["start"], rec["window"]["end"]] if "window" in rec else []
    argvs = [d["args"].split() for s in snaps + rec.get("restarts", []) for d in s.get("descendants", [])]
    found = next(((argv[0], t) for argv in argvs for t in argv if t.endswith("/main.ts")), None)
    if not found:
        return None
    node, folder = found[0], Path(found[1]).parent
    modules, version = folder / "node_modules", lambda p: json.loads(p.read_text())["version"]
    installed = modules / ".razor-package-lock.json"
    return {"main": str(folder / "main.ts"), "main_sha256": sha256(folder / "main.ts"), "package_lock_sha256": sha256(folder / "package-lock.json"),
            "installed_lock_matches": installed.is_file() and installed.read_bytes() == (folder / "package-lock.json").read_bytes(),
            "sdk": version(modules / "@anthropic-ai/claude-agent-sdk/package.json"),
            "cli_packages": {p.parent.name: version(p) for p in modules.glob("@anthropic-ai/claude-agent-sdk-*/package.json")},
            "node": node, "node_version": sh(node, "--version").strip(),
            "sessions_reported": sorted(set(re.findall(r"Claude Code (\S+), model (\S+),", rec.get("server_log") or "")))}


def run_one(name, run, shape):
    row = SCENARIOS[name]
    rec = {"scenario": name, "run": run, "route": args.route, "profile": args.profile, "row": row, "shape": shape}
    replies, out = [], io.StringIO()
    try:
        with contextlib.redirect_stdout(out), flow(replies, model=ROUTE["model"], config=ROUTE["code" if row["kind"] == "exec" else "plain"]) as f:
            try:
                DRIVERS[row["kind"]](rec, f, replies, shape, row)
            except BaseException:
                rec["traceback"] = traceback.format_exc()
                raise
            finally:
                log = f.home / ".razor/server.log"
                rec["server_log"] = log.read_text(errors="replace") if log.exists() else None
        rec["status"] = "ok"
    except SystemExit:  # flow() printed FAIL, its message and the kept scratch folder
        rec["status"], rec["error"] = "failed", out.getvalue().strip()
    if CLAUDE:
        rec["bridge"] = bridge(rec)
    for metric in rec.get("metrics", {}).values():
        metric["median"], metric["min"], metric["max"] = statistics.median(metric["samples"]), min(metric["samples"]), max(metric["samples"])
    return rec


root = args.scratch_root or tempfile.gettempdir()
if not Path(root).is_dir():
    ap.error(f"the scratch root {root} is not a folder")
if len(os.path.abspath(root)) + len("/razor-flow-12345678/home/.razor/server.sock") >= 104:  # macOS's socket path limit, under Linux's
    ap.error(f"the scratch root {root} is too long for the server's socket path; name a shorter one")
fs = fstype(root)
if fs in ("tmpfs", "ramfs") and not args.scratch_root:
    ap.error(f"{root} is {fs}; name it with --scratch-root to measure there")
tempfile.tempdir = root
digest = sha256(args.binary)
if args.expect_sha256 and digest != args.expect_sha256:
    ap.error(f"binary hash {digest} is not the expected {args.expect_sha256}")
names = args.scenario or [n for n in SCENARIOS if n.startswith("validate-") == args.validate]
environment = {
    "binary": {"path": str(Path(args.binary).resolve()), "sha256": digest, "bytes": Path(args.binary).stat().st_size,
               "revision": args.binary_revision},
    "source": {"checkout": str(repo), "revision": sh("git", "-C", str(repo), "rev-parse", "HEAD").strip(),
               "tracked_files_modified": len(sh("git", "-C", str(repo), "status", "--porcelain", "--untracked-files=no").splitlines()),
               # this script and every file of the checkout the run imported, with the recorded Claude reply its replies are built from
               "files": {str(p.relative_to(repo) if p.is_relative_to(repo) else p): sha256(p) for p in sorted(
                   {Path(m.__file__).resolve() for m in list(sys.modules.values()) if getattr(m, "__file__", None)} | {REPLIES.resolve() / "shell-short.sse"})
                   if p.is_relative_to(repo) or p == Path(__file__).resolve()}},
    "route": {"name": args.route, "model": ROUTE["model"], "wire_model": ROUTE["wire"] or "the Claude CLI's id for the alias, read from each request"},
    "platform": {"system": platform.platform(), "machine": platform.machine(), "python": sys.version.split()[0]},
    "cpus": {"count": os.cpu_count(), "usable": len(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None, "cgroup_cpu_max": cgroup("cpu.max")},
    "memory": {"physical_bytes": os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE"), "cgroup_memory_max": cgroup("memory.max")},
    "scratch": {"root": root, "filesystem": fs, "explicit": bool(args.scratch_root)},
    "utc_start": datetime.now(timezone.utc).isoformat(), "start": snapshot(), "profile": args.profile,
    "limitations": ["a loopback stand-in replays recorded bytes: no provider, no network, no live-provider baseline",
                    "endpoint emission is the stand-in's stamp just before it writes a slice; frames are stamped when this process reads the PTY",
                    "the stand-in, API client, PTY reader and pyte share one Python process; harness_cpu_s is that process's CPU",
                    "CPU and memory are sampled only at window edges: each named process (the server, an attached terminal) and the "
                    "descendants alive at an edge; one that came and went in between counts only in its waiting parent's "
                    "waited_children_cpu_s (Linux), so no whole-tree CPU or peak memory is given",
                    "rss_lifetime_peak_kib is a process's peak since it started, not the window's",
                    "macOS: ps TIME (10 ms) and RSS only; no waited-children CPU, anonymous or proportional memory",
                    "startup's window names no server: each restart's server and descendants are sampled once, just after its "
                    "client exits, since their own start",
                    "clients_cpu_s is the CPU of the processes this one started and waited for in the window (clients, git, ps)"],
}
results = []
for name in names:
    for run in range(args.runs):
        results.append(run_one(name, run, SHAPES[args.profile]))
        Path(args.output).write_text(json.dumps({"environment": environment, "results": results}, indent=1))
environment |= {"utc_end": datetime.now(timezone.utc).isoformat(), "end": snapshot()}
Path(args.output).write_text(json.dumps({"environment": environment, "results": results}, indent=1))
for rec in results:
    print(f"{rec['status'].upper()} {rec['scenario']} run {rec['run']} ({rec['route']}, {rec['profile']}){': ' + rec['error'] if 'error' in rec else ''}")
    for name, metric in rec.get("metrics", {}).items():
        print(f"  {name}: median {metric['median']:.2f} {metric['unit']}, n={len(metric['samples'])}, {metric['min']:.2f} to {metric['max']:.2f}")
    if "validation" in rec:
        print(f"  planted {rec['validation']['expected_ms']:.0f} ms in {rec['validation']['segment']}: observed {rec['validation']['observed_ms']:.1f} ms")
sys.exit(not results or any(rec["status"] != "ok" for rec in results))
