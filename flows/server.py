"""The running server, through its socket, for flows that act between or across client calls."""
from contextlib import suppress
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import time


class Local(http.client.HTTPConnection):
    """HTTP through the server's socket in the state directory."""

    def __init__(self, path):
        super().__init__("localhost", timeout=10)
        self.socket_path = str(path)

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.socket_path)


def start(f):
    """Starts the server through a real client call that sends no input; returns its pid."""
    started = f.run("devices")
    assert started.returncode == 0, started.stderr
    return json.loads((f.home / ".razor/server.json").read_text())["pid"]


def send(where, method, path, body=None, headers=None, key=None):
    """Sends `method` /v1<path> through the socket of the server whose state directory is
    `where` (or a flow's), with its local key unless `key` names another; a body that is not
    bytes or text goes as JSON. Returns the response, unread; it holds the connection open."""
    state = where if isinstance(where, Path) else where.home / ".razor"
    connection = Local(state / "server.sock")
    headers = {"Authorization": f"Bearer {key or (state / 'device.key').read_text().strip()}", **(headers or {})}
    if body is not None and not isinstance(body, (bytes, str)):
        body, headers["Content-Type"] = json.dumps(body), "application/json"
    # A route that reads no body (stop) may answer and close before the body, a second write,
    # is sent; its answer is still read below.
    with suppress(BrokenPipeError):
        connection.request(method, "/v1" + path, body, headers)
    return connection.getresponse()


def post(where, path, body):
    """POSTs `body` as JSON; the JSON answer of a 200."""
    answer = send(where, "POST", path, body)
    text = answer.read()
    assert answer.status == 200, (path, answer.status, text)
    return json.loads(text)


def agents(where):
    """The agent rows of the status stream's first line, by session."""
    return {a["session"]: a for a in json.loads(send(where, "GET", "/status").readline())["agents"]}


def listed(where, session):
    """Whether a session appears after the once-a-second layout publication."""
    time.sleep(1.1)
    with send(where, "GET", "/status?views=true") as stream:
        layouts = next(json.loads(line)["layouts"] for line in iter(stream.readline, b"") if json.loads(line)["kind"] == "views")
    return any(row["session"] == session for layout in layouts for section in layout["sections"] for row in section["sessions"])


def until(f, session, done, timeout=30.0):
    """The record, once `done(entries)` holds; fails with the record's tail at the deadline."""
    deadline = time.monotonic() + timeout
    while not done(entries := list(f.record(session))):
        assert time.monotonic() < deadline, f"timed out; record ends {entries[-3:]}"
        time.sleep(0.05)
    return entries


def wait(condition, timeout=30.0, why="timed out"):
    """`condition()`'s first truthy value, polled every 50 ms; fails with `why` at the deadline."""
    deadline = time.monotonic() + timeout
    while not (found := condition()):
        assert time.monotonic() < deadline, why
        time.sleep(0.05)
    return found


def gone(pid, timeout=30.0):
    """Returns once the server's process group (the server, the bridge, the CLI) is empty."""
    deadline = time.monotonic() + timeout
    with suppress(ProcessLookupError):
        while time.monotonic() < deadline:
            os.killpg(pid, 0)
            time.sleep(0.05)
        raise AssertionError(f"server group {pid} still alive")


def stop(where, sig=signal.SIGTERM):
    """Sends `sig` to the server whose state directory is `where` (or a flow's); returns once it is gone."""
    pid = json.loads(((where if isinstance(where, Path) else where.home / ".razor") / "server.json").read_text())["pid"]
    os.kill(pid, sig)
    gone(pid)
