"""Bad checksums preserve current; cached installs retain current, previous and Razor.app while pruning aged
releases. The first install enters setup in a terminal, else prints its command; an update prints the restart
line; an unselected install neither. Full archive installation belongs to clean-install checks."""
import fcntl
import os
import select
import shutil
import subprocess
import termios
import time
from pathlib import Path
from driver import BINARY, Reply, flow
from standin import serve
import server

installer = Path(__file__).resolve().parents[1] / "scripts/install-release.sh"
with flow([]) as f:
    root = f.home / ".local/share/razor"
    for version in ("v-old", "v-one", "v-two", "v-live", "Razor.app"):
        bundle = root / version
        bundle.mkdir(parents=True)
        if version == "v-live":
            shutil.copy2(BINARY, bundle / "razor")
        else:
            (bundle / "razor").symlink_to(BINARY)
        os.utime(bundle, (time.time() - 3 * 86400,) * 2)
    (root / "current").symlink_to("v-live")

    def install(version, url, **more):
        return subprocess.run(["sh", str(installer)], env={**f.env, "RAZOR_VERSION": version,
                              "RAZOR_BASE_URL": url, **more}, capture_output=True, text=True, timeout=30)

    programs, unpacked = f.home / "bin", f.home / "unpacked"
    programs.mkdir()
    tar = programs / "tar"
    tar.write_text(f'#!/bin/sh\ntouch "{unpacked}"\nexit 1\n')
    tar.chmod(0o755)
    f.env["PATH"] = f"{programs}:{f.env['PATH']}"
    replies = [Reply(b"damaged archive", match=".tar.gz", method="GET"),
               Reply(("0" * 64 + "  archive\n").encode(), match=".sha256", method="GET")]
    with serve(replies, f.home / "downloads") as url:
        refused = install("v-bad", url)
        assert refused.returncode != 0 and not unpacked.exists() and not (root / "v-bad").exists(), "installer unpacked bytes with a bad checksum"
        assert (root / "current").readlink() == Path("v-live"), "checksum refusal changed current"
    log = (f.home / "bundle-server.log").open("w")
    running = subprocess.Popen([str(root / "current/razor"), "serve"], env=f.env, cwd=f.cwd,
                               stdout=log, stderr=log, start_new_session=True)
    try:
        server.wait(lambda: (f.home / ".razor/server.json").exists(), why="bundled server never started")
        os.utime(root / "v-two", None)
        first = install("v-one", "http://127.0.0.1:1")
        assert first.returncode == 0 and first.stdout == f"{root / 'v-one/razor'}\n", first
        assert "setup in a terminal" in first.stderr and str(f.home / ".local/bin/razor") in first.stderr, first
        selected = install("v-two", "http://127.0.0.1:1")  # Cached: no network is needed.
        assert (root / "v-live/razor").is_file(), "installer pruned a running server's bundle"
    finally:
        running.terminate()
        running.wait(timeout=30)
        log.close()
    assert selected.returncode == 0 and "Run razor restart" in selected.stderr and "setup" not in selected.stderr, selected
    assert (root / "current").readlink() == Path("v-two"), "installer did not select cached release"
    assert (root / "v-one").is_dir() and (root / "v-two").is_dir() and not (root / "v-old").exists(), "installer did not retain current and previous only"
    assert (root / "Razor.app").is_dir(), "installer pruned Razor.app, which macOS's permissions are granted to"
    assert (f.home / ".local/bin/razor").readlink() == root / "current/razor", "local bin link does not follow current"
    selected = install("v-two", "http://127.0.0.1:1")
    assert selected.returncode == 0 and not (root / "v-live").exists(), ("stopped bundle was not pruned", selected.stderr)
    unselected = install("v-two", "http://127.0.0.1:1", RAZOR_SELECT="0")
    assert unselected.returncode == 0 and unselected.stderr == "" and (root / "current").readlink() == Path("v-two"), unselected
    # A first install with a terminal enters setup, here ended at its first question, and that exit
    # status is the installer's, with the install complete.
    (f.home / ".local/bin/razor").unlink()
    master, slave = os.openpty()
    child = subprocess.Popen(["sh", str(installer)], stdin=slave, stdout=slave, stderr=slave,
                             env={**f.env, "RAZOR_VERSION": "v-two", "RAZOR_BASE_URL": "http://127.0.0.1:1"},
                             start_new_session=True, preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
    os.close(slave)
    output, deadline = b"", time.monotonic() + 30
    try:
        while b"Use Claude, ChatGPT or both" not in output:
            assert time.monotonic() < deadline and child.poll() is None, f"the installer did not enter setup: {output!r}"
            if select.select([master], [], [], .1)[0]:
                try:
                    output += os.read(master, 65536)
                except OSError:  # The installer has ended; the check above says what it printed.
                    time.sleep(.05)
        os.write(master, b"\x04")  # Ctrl-D: the end of input
        status = child.wait(timeout=15)
        output += os.read(master, 65536)
    finally:
        child.kill()
        os.close(master)
    assert status != 0 and b"Input stopped" in output, (status, output)
    assert (f.home / ".local/bin/razor").readlink() == root / "current/razor", "setup's failure undid the install"
