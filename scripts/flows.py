#!/usr/bin/env python3
"""Run selected numbered flows, eight at a time, printing each file's output when it exits."""
from contextlib import suppress
from pathlib import Path
import argparse
import json
import os
import signal
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--shard", help="one-based INDEX/TOTAL, balanced by recorded durations")
parser.add_argument("--repeat", type=int, default=1, help="complete repetitions, including after failures")
parser.add_argument("selectors", nargs="*")
args = parser.parse_args()
if args.repeat < 1:
    parser.error("--repeat must be positive")
files, running, stopped, failed = set(), {}, 0, 0
for arg in args.selectors or ["all"]:
    selector = Path(arg).name.removesuffix(".py")
    files |= {p for p in Path("flows").glob("[0-9][0-9]-*.py") if selector in
              ("all", p.stem, p.stem[3:], p.name[:2], str(int(p.name[:2])))} or sys.exit(
        f"No flow matches '{selector}'; use a flow number or name.")
if args.shard:
    try:
        index, total = map(int, args.shard.split("/"))
        if not 1 <= index <= total <= len(files):
            raise ValueError
    except ValueError:
        parser.error("--shard must be INDEX/TOTAL with 1 <= INDEX <= TOTAL <= flow count")
    durations = json.loads(Path("scripts/flow-times.json").read_text())
    shards, loads = [[] for _ in range(total)], [0] * total
    for file in sorted(files, key=lambda p: (-durations.get(p.name, max(durations.values())), p.name)):
        shard = min(range(total), key=lambda i: loads[i])
        shards[shard].append(file)
        loads[shard] += durations.get(file.name, max(durations.values()))
    files = list(reversed(shards[index - 1]))
else:
    files = sorted(files)
# Each repetition finishes before the next starts, so no file races its own fixtures.
files = [(file, repeat) for repeat in reversed(range(1, args.repeat + 1)) for file in files]


def stop(signum, _):
    global stopped
    stopped = stopped or 128 + signum
    for pid in running:
        with suppress(ProcessLookupError):
            os.killpg(pid, signal.SIGTERM)


for signum in (signal.SIGINT, signal.SIGTERM):
    signal.signal(signum, stop)
while running or files and not stopped:
    if (files and len(running) < 8 and not stopped
            and (not running or files[-1][1] == next(iter(running.values()))[1])):
        (file, repeat), output = files.pop(), tempfile.TemporaryFile()
        process = subprocess.Popen([sys.executable, "-u", file], stdin=subprocess.DEVNULL,
                                   stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        running[process.pid] = file, repeat, output, process, time.monotonic()
        if stopped:  # A signal during Popen must also stop the newly registered child.
            stop(stopped - 128, None)
        continue
    pid, status = os.wait()
    file, repeat, output, process, started = running.pop(pid)
    process.returncode = code = os.waitstatus_to_exitcode(status)
    output.seek(0)
    sys.stdout.buffer.write(output.read() + (f"{file} exited {code}\n".encode() if code else b""))
    sys.stdout.flush()
    print(f"FLOW {file.name} repeat={repeat} seconds={time.monotonic() - started:.3f} exit={code}", flush=True)
    failed = failed or code != 0
sys.exit(stopped or failed)
