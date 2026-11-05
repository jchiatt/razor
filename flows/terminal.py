"""The real app under a PTY: keys in, rendered cells out, whole-frame receipt times.
Requires the pinned emulator in flows/requirements.txt; no Razor state is imitated here.
"""
from collections import deque
from contextlib import suppress
import base64
import fcntl
import os
import re
import select
import signal
import struct
import termios
import threading
import time

import pyte
from wcwidth import wcswidth

from driver import BINARY

END = b"\x1b[?2026l"
BOUND = 8 * 1024 * 1024  # bytes of the app's output kept, unread or unframed


class Terminal:
    """A 120-column laptop screen by default; Ctrl-C twice must quit and restore its modes. With
    status, it answers the OSC 7501 query as a terminal that keeps program status records does."""

    def __init__(self, f, *args, rows=40, columns=120, status=False):
        self.f, self.rows, self.columns, self.status = f, rows, columns, status
        self.screen = pyte.Screen(columns, rows)
        self.stream = pyte.ByteStream(self.screen)
        self.pending, self.output, self.queries, self.frame = b"", b"", b"", b""
        self.frames, self.frame_at, self.received = 0, None, None
        self.fd, slave = os.openpty()
        self.original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        self.started = time.monotonic()
        self.pid = os.fork()
        if self.pid == 0:
            os.close(self.fd)
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
            for fd in (0, 1, 2):
                os.dup2(slave, fd)
            os.close(slave)
            os.chdir(f.cwd)
            os.execvpe(BINARY, [BINARY, *args], {**f.env, "TERM": "xterm-256color"})
        os.close(slave)
        self.chunks, self.arrived, self.reading = deque(), threading.Condition(), True
        self.unread, self.overflowed = 0, False
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def __enter__(self):
        return self

    def __exit__(self, kind, error, trace):
        try:
            self.quit()
        except Exception:
            if kind is None:  # else the earlier failure, the one to report, goes on
                raise
        finally:
            # Kept with a failed flow's scratch home, including failures before the first frame.
            (self.f.home / f"terminal-{self.pid}.txt").write_text(self.text())
            (self.f.home / f"terminal-{self.pid}.ansi").write_bytes(self.output)
            self.reading = False
            self.reader.join()
            os.close(self.fd)

    def text(self):
        """Rendered screen cells, not a search of bytes that may since have been erased."""
        return "\n".join(self.screen.display)

    def send(self, text):
        os.write(self.fd, text.encode() if isinstance(text, str) else text)

    def click(self, words):
        """Click the first displayed occurrence, using its actual cell coordinates."""
        row, column = next((y, wcswidth(line[:line.index(words)]))
                           for y, line in enumerate(self.screen.display) if words in line)
        self.send(f"\x1b[<0;{column + 1};{row + 1}M\x1b[<0;{column + 1};{row + 1}m")

    def resize(self, rows, columns):
        self.rows, self.columns = rows, columns
        self.screen.resize(rows, columns)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        os.kill(self.pid, signal.SIGWINCH)

    def answer(self, chunk):
        queries = self.queries + chunk
        answers = {b"\x1b[c": b"\x1b[?62;22c", b"\x1b[0c": b"\x1b[?62;22c",
                   b"\x1b[6n": f"\x1b[{self.screen.cursor.y + 1};{self.screen.cursor.x + 1}R".encode(),
                   b"\x1b[?u": b"\x1b[?0u"}
        if self.status:
            answers[b"\x1b]7501;?\x1b\\"] = b"\x1b]7501;?:states=idle,working,done,blocked,error\x1b\\"
        for query, answer in answers.items():
            for _ in range(queries.count(query)):
                self.send(answer)
        # Only an incomplete query prefix, never a completed query, survives the read.
        self.queries = next((queries[-n:] for n in range(max(map(len, answers)) - 1, 0, -1)
                             if any(query.startswith(queries[-n:]) for query in answers)), b"")

    def read(self):
        """Reads the PTY whether or not the flow is waiting on it, as a terminal does: output left
        unread fills the PTY and blocks the app in its write, stalling its whole event loop. Past
        BOUND unread bytes it still drains the PTY but keeps nothing, and the next pump fails. An
        empty chunk marks the end."""
        while self.reading:
            if not select.select([self.fd], [], [], 0.05)[0]:
                continue
            try:
                chunk = os.read(self.fd, 65536)
            except OSError:
                chunk = b""
            self.answer(chunk)
            with self.arrived:
                self.overflowed |= self.unread + len(chunk) > BOUND
                if not self.overflowed or not chunk:
                    self.unread += len(chunk)
                    self.chunks.append((time.monotonic(), chunk))
                self.arrived.notify()
            if not chunk:
                return

    def pump(self, timeout, predicate=lambda _: False):
        """Take what arrives for at most timeout seconds; test predicate only at each synchronized
        frame end."""
        deadline = time.monotonic() + timeout
        while (left := deadline - time.monotonic()) > 0:
            while END in self.pending:
                frame, self.pending = self.pending.split(END, 1)
                self.frame = frame + END
                self.stream.feed(self.frame)
                self.frames += 1
                self.frame_at = self.received
                if predicate(self):
                    return True
            with self.arrived:
                self.arrived.wait_for(lambda: self.chunks or self.overflowed, left)
                assert not self.overflowed, "app wrote 8 MiB more than the flow read"
                if not self.chunks or not self.chunks[0][1]:
                    break
                self.received, chunk = self.chunks.popleft()
                self.unread -= len(chunk)
            self.output = (self.output + chunk)[-BOUND:]
            self.pending += chunk
            assert len(self.pending) <= BOUND, "app wrote 8 MiB without a frame end"
        return False

    def until(self, predicate, timeout=30):
        """Return the receipt time of the first whole frame satisfying predicate: the one on
        screen, or a later one."""
        assert predicate(self) or self.pump(timeout, predicate), \
            "app never drew the expected screen:\n" + self.text()
        return self.frame_at

    def shown(self, *words, timeout=30):
        """Wait for a whole frame with every named phrase still visible."""
        return self.until(lambda t: all(word in t.text() for word in words), timeout)

    def quit(self):
        """Ctrl-C, its armed line, then Ctrl-C again must leave exit zero and exactly the terminal
        modes from before attach."""
        self.send(b"\x03")
        assert self.pump(10, lambda t: "PRESS CTRL-C AGAIN TO QUIT" in t.text()), \
            "the first Ctrl-C did not ask for a second:\n" + self.text()
        self.send(b"\x03")
        # The app waits up to 30 s for its last draft save before it quits.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.pump(1)  # what the app wrote as it exited
                assert os.waitstatus_to_exitcode(status) == 0, (status, self.output[-2000:])
                assert termios.tcgetattr(self.fd) == self.original, "app left terminal modes changed"
                return
            self.pump(0.02)
        with suppress(ProcessLookupError):
            os.kill(self.pid, signal.SIGKILL)
        os.waitpid(self.pid, 0)
        raise AssertionError("Ctrl-C did not quit the app")

    def records(self):
        """The OSC 7501 records the app's reports leave, kept as the protocol says a terminal does."""
        kept = {}
        for body in re.findall(rb"\x1b\]7501;([^\x07\x1b]*)\x1b\\", self.output):
            pairs = dict(pair.split("=", 1) for pair in body.decode().split(":") if "=" in pair)
            pairs |= {key: base64.b64decode(pairs[key]).decode() for key in ("msg", "title") if key in pairs}
            id = pairs.pop("id", "")
            if pairs.get("state") == "clear":
                kept = {key: value for key, value in kept.items() if id and key != id and not key.startswith(id + "/")}
            elif pairs:
                kept[id] = pairs
        return kept
