"""The session list (docs/design/api.md): roots newest first, narrowed to a folder, the limit counted
after that; a cut first line, a session folder with no record, a child and a stray file are left
out; no sessions folder at all is an empty list."""
import json

from driver import flow
import server
from server import post, send

with flow([]) as f:
    server.start(f)
    sessions = f.home / ".razor/sessions"

    def ask(query=""):
        answer = send(f, "GET", "/sessions" + query)
        body = answer.read()
        assert answer.status == 200, body
        return [(row["name"], row["session"]) for row in json.loads(body)]

    assert not sessions.exists() and ask() == []
    # A real root's first line is every fixture's, with its folder and name changed.
    seed = post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    start = json.loads((sessions / seed / "record.jsonl").read_bytes().split(b"\n")[0])
    a, b = f.cwd / "a", f.cwd / "b"
    a.mkdir()
    b.mkdir()
    kid = {"parent": seed, "name": "kid", "worktree": None, "launched": 1}
    made = {start["name"]: seed}
    # Made in no order but the list's, so a directory's own order cannot pass for it.
    for n in (4, 1, 8, 3, 6, 2, 7, 5):
        folder = b if n in (2, 8) else a
        made[f"{folder.name}{n}"] = session = f"20260101-000000-000{n}"
        (sessions / session).mkdir()
        if n != 6:  # Its folder was made, its record never.
            text = json.dumps({**start, "cwd": str(folder), "name": f"{folder.name}{n}",
                               **({"child": kid} if n == 7 else {})}).encode()
            (sessions / session / "record.jsonl").write_bytes(text[:len(text) // 2] if n == 5 else text + b"\n")
    (sessions / ".DS_Store").write_text("")
    rows = lambda *names: [(name, made[name]) for name in names]
    # Newer roots of `b`, a cut record, a missing one and a child all come before the newest two of `a`.
    assert ask(f"?cwd={a}&limit=2") == rows("a4", "a3"), ask(f"?cwd={a}&limit=2")
    assert ask(f"?cwd={a}") == rows("a4", "a3", "a1")
    assert ask("?limit=100") == rows(start["name"], "b8", "a4", "a3", "b2", "a1")
