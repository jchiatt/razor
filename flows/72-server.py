"""`razor stop` and `razor restart` act on their own home's server alone: an agent reading server.lock
cannot free it, a lock with no PID is refused, a second home named .razor starts its own server beside
another home's service, and install removes a unit it wrote when no manager answers (docs/design/setup.md)."""
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from driver import BINARY, flow
from responses import PLAIN, call, done, response
from server import gone, start

lock_read = response([call("read", json.dumps({"path": "../home/.razor/server.lock"}), "call_lock")])
with tempfile.TemporaryDirectory() as fake, flow([lock_read, done()], model="gpt-6.1-sol", config=PLAIN,
                                                 env={"PATH": f"{fake}:{os.environ['PATH']}"}) as f:
    state = f.home / ".razor"
    pid = lambda home: json.loads((home / "server.json").read_text())["pid"]
    # 1. Reading server.lock through the agent's own file tool, in the server, keeps the lock.
    first = start(f)
    assert "Server: running." in f.run("doctor").stdout
    answer = f.run("--print=json", "Read the lock file.")
    read = next(e for e in f.record(json.loads(answer.stdout)["session"]) if e["kind"] == "result")
    assert read["outcome"] == "success" and str(first) in read["output"], read
    try:
        second = subprocess.run([BINARY, "serve"], env=f.env, cwd=f.cwd, capture_output=True, text=True, timeout=15)
    except subprocess.TimeoutExpired:
        raise AssertionError("a second server started: reading server.lock freed its lock") from None
    assert second.returncode != 0 and "already running" in second.stderr, second
    os.kill(first, 0)
    # 2. Stop ends that server and is a success when none runs; restart replaces it.
    assert f.run("stop").returncode == 0
    gone(first)
    assert f.run("stop").returncode == 0
    first = start(f)
    assert f.run("restart").returncode == 0
    gone(first)
    assert pid(state) != first and f.run("devices").returncode == 0, "restart left no server"
    assert f.run("stop").returncode == 0
    gone(pid(state))
    # A detached server that cannot start fails the next command at once, naming server.log.
    config = state / "config.toml"
    good, began = config.read_text(), time.monotonic()
    config.write_text("not valid [")
    failed = f.run("devices")
    config.write_text(good)
    assert failed.returncode == 1 and time.monotonic() - began < 5 and str(state / "server.log") in failed.stderr, failed
    # 3. A lock held without a PID: refused, and its holder is never signalled.
    holder = subprocess.Popen([sys.executable, "-c", "import fcntl, sys, time\n"
                               "held = open(sys.argv[1], 'w')\nfcntl.flock(held, fcntl.LOCK_EX)\nprint('held', flush=True)\n"
                               "time.sleep(60)", str(state / "server.lock")], stdout=subprocess.PIPE, text=True)
    try:
        assert holder.stdout.readline() == "held\n"
        refused = f.run("stop")
        assert refused.returncode == 1 and "no PID" in refused.stderr, refused
        assert holder.poll() is None and (state / "server.lock").read_text() == "", "stop signalled a holder it cannot name"
    finally:
        holder.kill()
        holder.wait()
    # 4. Service managers that answer by the exit status in `status`.
    calls, status = Path(fake) / "calls", Path(fake) / "status"
    for manager in ("systemctl", "launchctl"):
        (Path(fake) / manager).write_text(f'#!/bin/sh\necho "{manager} $*" >> "{calls}"\nexit "$(cat "{status}")"\n')
        (Path(fake) / manager).chmod(0o755)
    service = f.home / ("Library/LaunchAgents/razor.plist" if sys.platform == "darwin"
                        else ".config/systemd/user/razor.service")
    status.write_text("1")
    installed = f.run("install")
    assert installed.returncode == 0 and "removed" in installed.stdout and not service.exists(), installed
    status.write_text("0")
    assert f.run("install").returncode == 0 and service.exists()
    unit = service.read_bytes()
    status.write_text("1")
    kept = f.run("install")
    assert kept.returncode == 0 and "was kept" in kept.stdout and service.read_bytes() == unit, kept
    assert "Service: installed." in f.run("doctor").stdout
    # 5. Another home, in a folder with the same name, shares the service file's name but not the
    # service: its server starts detached, the manager is never asked to start the service, and
    # doctor says the file is taken.
    status.write_text("0")
    calls.unlink()
    other = f.home / "other/.razor"
    run = lambda *argv: subprocess.run([BINARY, *argv], env={**f.env, "RAZOR_HOME": str(other)},
                                       cwd=f.cwd, capture_output=True, text=True, timeout=60)
    try:
        assert run("restart").returncode == 0 and (other / "server.json").exists(), "the other home has no server"
        assert not calls.exists() and service.read_bytes() == unit, "the other home touched the service"
        line = next(l for l in run("doctor").stdout.splitlines() if l.startswith("Service:"))
        assert line.startswith("Service: invalid.") and "another Razor home" in line, line
        server = pid(other)
    finally:
        stopped = run("stop")
    assert stopped.returncode == 0, stopped
    gone(server)
