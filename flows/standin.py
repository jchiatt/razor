"""The model stand-in: one loopback HTTP server replaying recorded response bytes unchanged."""

from contextlib import contextmanager
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, HTTPServer
from itertools import count
from pathlib import Path
import json
import sys
import threading
import time


@dataclass(frozen=True)
class Reply:
    """One response body, sent as it is; at each cut (a byte offset) sending pauses `pause` seconds."""

    body: bytes
    cuts: tuple[int, ...] = ()
    pause: float = 0.03
    match: str | None = None  # a phrase the request must hold; None takes requests in turn
    headers: tuple[tuple[str, str], ...] = ()  # actual model response headers, e.g. plan windows
    status: int = 200
    method: str = "POST"  # a GET reply's `match` is held by the request's path


@contextmanager
def serve(replies: list[Reply], requests: Path):
    """Yields the server's URL. Each request is spooled in `requests` as <n>.json (method, path,
    headers, `arrived` and, once its reply is out, `sent`: time.monotonic() in this process) and
    <n>.body: the nth POST as 00n, taking the first unused reply whose `match` it holds, else the
    first unused one with none; any other method as other-00n, answered the same way from the
    replies of its method, or 404 when none is left. `replies` is read as
    requests come, so a flow may add to it. A clean exit fails if a request failed or found no
    reply, or a reply went unused."""
    requests.mkdir(exist_ok=True)
    used, errors, others = set(), [], count(1)

    class Handler(BaseHTTPRequestHandler):
        timeout, disable_nagle_algorithm = 10, True  # a cut leaves as its own packet

        def log_message(self, *_):
            pass

        def __getattr__(self, name):  # every method, so every request is spooled
            if name.startswith("do_"):
                return self.replay
            raise AttributeError(name)

        def replay(self):
            meta = {"method": self.command, "path": self.path, "arrived": time.monotonic(),
                    "headers": {key.lower(): value for key, value in self.headers.items()}}
            body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            model = self.command == "POST"
            name = requests / (f"{len(used) + 1:03}" if model else f"other-{next(others):03}")
            name.with_suffix(".json").write_text(json.dumps(meta))
            name.with_suffix(".body").write_bytes(body)
            held = body if model else self.path.encode()
            free = [(reply.match is None, n) for n, reply in enumerate(replies) if n not in used
                    and reply.method == self.command and (reply.match is None or reply.match.encode() in held)]
            if not model and not free:
                return self.send_error(404)
            assert free, f"model request {len(used) + 1} found no unused reply; last message: {[(m.get('role'), str(m.get('content'))[:300]) for m in json.loads(body).get('messages', [])[-1:]]}"
            used.add(index := min(free)[1])
            reply = replies[index]
            self.send_response(reply.status)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(reply.body)))
            for key, value in reply.headers:
                self.send_header(key, value)
            self.end_headers()
            for start, end in zip((0, *reply.cuts), (*reply.cuts, None)):
                time.sleep(reply.pause if start else 0)
                self.wfile.write(reply.body[start:end])
            meta["sent"] = time.monotonic()
            name.with_suffix(".json").write_text(json.dumps(meta))

    with HTTPServer(("127.0.0.1", 0), Handler) as server:
        server.handle_error = lambda *_: errors.append(repr(sys.exc_info()[1]))
        threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True).start()
        try:
            yield f"http://127.0.0.1:{server.server_port}"
        finally:
            server.shutdown()
    assert not errors, "; ".join(errors)
    assert len(used) == len(replies), f"used {len(used)} of {len(replies)} replies"
