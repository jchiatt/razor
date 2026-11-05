#!/usr/bin/env python3
"""The performance budgets of SPEC.md section 6, measured on their workloads against the flows'
model stand-in (docs/design/perf.md): one line per budget with its number and unit, then exit 1
if a number is past its budget or SPEC.md holds none for it."""
from pathlib import Path
import json
import re
import statistics
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / "flows"), str(ROOT / "perf")]
from driver import flow  # noqa: E402
from responses import PLAIN  # noqa: E402
import server  # noqa: E402
from terminal import Terminal  # noqa: E402
from workloads import ANSWER, LINE, busy, history, page_text, run, say, turn  # noqa: E402

MODEL = "gpt-6.1-sol"
MEASURED, CALLS, STARTS = 10, 3, 10
budgets = {name: float(limit) for name, limit in re.findall(
    r"^\| ([^|]+?) \| ([\d.]+) (?:ms|MiB) \|$",
    (ROOT / "SPEC.md").read_text().split("## 6. Budgets", 1)[1].split("## 7.", 1)[0], re.M)}


def model(f, n):
    """When the stand-in got the nth model request, and when it had sent all of its reply. It
    rewrites the file with `sent` after the reply's last byte, so the turn can end first."""
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        try:
            meta = json.loads((f.requests / f"{n:03}.json").read_text())
        except ValueError:  # read between the rewrite's truncation and its write
            meta = {}
        if "sent" in meta:
            return meta["arrived"], meta["sent"]
        time.sleep(0.005)
    raise AssertionError(f"model request {n} never recorded its reply as sent")


def posts(f):
    """How many model requests the stand-in has had."""
    return sum(path.stem.isdigit() for path in f.requests.glob("*.body"))


def anonymous(pid):
    """The process's anonymous resident memory, and all of its resident memory, in MiB."""
    status = Path(f"/proc/{pid}/status").read_text().splitlines()
    fields = dict(line.split(":", 1) for line in status)
    return (int(fields[key].split()[0]) / 1024 for key in ("RssAnon", "VmRSS"))


def median(name, values, where=""):
    """A budget's line from its samples in seconds: the median in ms, and how it was taken."""
    values = sorted(value * 1000 for value in values)
    return (name, statistics.median(values), "ms",
            f"median of {len(values)}{where}, {values[0]:.1f} to {values[-1]:.1f}")


# The scratch home lives in memory: a shared disk's sync times swing tenfold
# with other tenants' writes (docs/design/perf.md).
tempfile.tempdir = "/dev/shm"
replies = []
with flow(replies, model=MODEL, config=PLAIN) as f:
    (f.cwd / "persona.md").write_text("You are Ember, the measured resident.")
    beat = time.strftime("%H:%M:%S", time.localtime(time.time() + 12 * 3600))  # never due here
    with (f.home / ".razor/config.toml").open("a") as config:
        config.write(f"[residents.ember]\ncwd = {json.dumps(str(f.cwd))}\n"
                     f"persona = {json.dumps(str(f.cwd / 'persona.md'))}\n"
                     f'[residents.ember.heartbeat]\nat = ["{beat}"]\ntext = "Heartbeat."\n')
    page, stop = f.cwd / "page.txt", f.cwd / "stop"
    page.write_text(page_text())
    pid = server.start(f)
    long = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    built = last = history(f, long, replies, lambda t: [run("read", {"path": str(page)}, t), say(ANSWER, t)])
    # Another session stays busy from here to the end.
    busy_session = busy(f, replies, stop)
    time.sleep(1)  # settled: four of the busy session's 250 ms tail updates
    anon, rss = anonymous(pid)
    attaches, starts = [], []
    for _ in range(STARTS):
        with Terminal(f) as t:
            starts.append(t.shown("YOU", "Message the agent") - t.started)
        with Terminal(f, "--resume", long) as t:
            attaches.append(t.shown("57. " + LINE, "Message the agent") - t.started)
    calls, turns = [], []
    for t in range(MEASURED):
        first = posts(f) + 1
        replies += [run("read", {"path": str(page)}, f"{t}_{c}") for c in range(CALLS)]
        replies.append(say(3, f"m{t}"))
        sent, ended, last = turn(f, long, last, f"Measured turn {t}: read the page {CALLS} times.")
        steps = [model(f, first + i) for i in range(CALLS + 1)]
        calls += [steps[i + 1][0] - steps[i][1] for i in range(CALLS)]
        turns.append(steps[0][0] - sent + ended - steps[-1][1])
    stop.touch()
    server.until(f, busy_session, lambda entries: entries[-1]["kind"] == "turn_end")
# The flow has passed (no request failed, no process outlived the server); now the numbers.
failed = []
for name, number, unit, how in (
        median("Cold start to the first prompt", starts),
        ("Idle memory", anon, "MiB", f"anonymous, of {rss:.1f} MiB resident"),
        median("Overhead per tool call", calls, f" from entry {built} to {last}"),
        median("Overhead per turn", turns),
        median("Draw of a 10,000-line transcript", attaches, ", attach to first full transcript frame")):
    limit = budgets.get(name)
    verdict = "FAIL" if limit is None or number > limit else "PASS"
    print(f"{verdict} perf {name}: {number:.1f} {unit} ({how}); budget "
          f"{'none in SPEC.md' if limit is None else f'{limit:g} {unit}'}")
    failed += [name] * (verdict == "FAIL")
sys.exit(bool(failed))
