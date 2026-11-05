"""The API: enroll a key, list, read pages, attach and send, revoke (docs/design/api.md)."""
import http.client
import json
import resource
import tempfile
import time
from pathlib import Path
from driver import REPLIES, Reply, flow
from model import reply
from responses import done
import server


def open_(connect, method, path, key, body=None):
    connection = connect()
    headers = {"Authorization": f"Bearer {key}"} if key else {}
    if isinstance(body, (dict, list)):
        body, headers["Content-Type"] = json.dumps(body), "application/json"
    connection.request(method, "/v1" + path, body=body, headers=headers)
    return connection.getresponse()


def call(connect, method, path, key, body=None):
    answer = open_(connect, method, path, key, body)
    return answer.status, answer.read()


def lines(stream):
    """The stream's next JSON line; None once it has ended."""
    line = stream.readline()
    return json.loads(line) if line else None


# An entry as a client gets it by default: without `native`, the model path's replay data.
bare = lambda entry: {k: v for k, v in entry.items() if k != "native"}


# Each turn reads 3 of its 4 input tokens from the cache; a pack offers the skill `hi`.
hello = (REPLIES / "hello.sse").read_bytes().replace(b'"cache_read_input_tokens": 0',
                                                     b'"cache_read_input_tokens": 3')
pack = Path(tempfile.mkdtemp()) / "pack"
(pack / "skills/hi").mkdir(parents=True)
(pack / "skills/hi/SKILL.md").write_text("---\ndescription: Says hi.\n---\nSay hi first.\n")
(pack / "config.toml").write_text("")
with flow([reply("Reply with the flow marker.", "Bash", {"command": "true"}),
           reply("Reply with the flow marker.", "Bash", {"command": "exit 1"}),
           *[Reply(hello) for _ in range(3)]],
          config=f'pack = {json.dumps(str(pack))}\n[server]\naddress = "127.0.0.1:0"') as f:
    # 1. --print starts the server through the socket and makes a session. It starts under a
    # soft limit of 64 open files and raises it, since twenty agents' pipes pass macOS's 256.
    hard = resource.getrlimit(resource.RLIMIT_NOFILE)[1]
    resource.setrlimit(resource.RLIMIT_NOFILE, (64, hard))
    result = f.run("--print=json", "Reply with the flow marker.")
    resource.setrlimit(resource.RLIMIT_NOFILE, (min(hard, 65_536), hard))
    assert result.returncode == 0, result.stderr
    session = json.loads(result.stdout)["session"]
    state = f.home / ".razor"
    key = (state / "device.key").read_text().strip()
    running = json.loads((state / "server.json").read_text())
    host, port = running["address"].rsplit(":", 1)
    soft = next(line.split()[3] for line in open(f"/proc/{running['pid']}/limits") if line.startswith("Max open files"))
    assert int(soft) == min(hard, 65_536), soft

    def local():
        return server.Local(state / "server.sock")

    def remote():
        return http.client.HTTPConnection(host, int(port), timeout=10)

    # 2. razor devices enrolls a phone and shows its key once; the list never shows a key.
    added = f.run("devices", "add", "phone")
    assert added.returncode == 0, added.stderr
    phone = added.stdout.splitlines()[1]
    assert len(phone) == 64 and phone != key, added.stdout
    assert f.run("devices").stdout == "phone\n"
    status, body = call(local, "GET", "/devices", key)
    assert [d["key"] for d in json.loads(body)] == [None], body

    # 3. Over TCP the phone's key lists the session; no key, or a wrong one, gets 401.
    status, body = call(remote, "GET", "/sessions", phone)
    assert status == 200 and [s["session"] for s in json.loads(body)] == [session], body
    for wrong in (None, "0" * 64):
        status, body = call(remote, "GET", "/sessions", wrong)
        assert status == 401 and json.loads(body)["error"], (status, body)

    # 4. A page with `native=true` is the record's own lines, byte for byte (step 10 reads one
    # without); entry 1 alone is Start.
    stored = (state / "sessions" / session / "record.jsonl").read_bytes().splitlines(True)
    status, body = call(remote, "GET", f"/sessions/{session}/entries?limit=2&native=true", phone)
    assert status == 200 and body == b"".join(stored[-2:]), (status, body)
    status, body = call(remote, "GET", f"/sessions/{session}/entries?before=2&limit=1",
                        phone)
    assert status == 200 and json.loads(body)["kind"] == "start", (status, body)

    # 5. A draft reads back; attach after the last entry; an input handed the skill `hi` holds
    # it ahead of its words, its turn arrives as recorded, and admitting it cleared the draft.
    status, body = call(remote, "GET", f"/sessions/{session}/skills", phone)
    assert json.loads(body) == [{"name": "hi", "description": "Says hi."}], (status, body)
    draft = f"/sessions/{session}/draft"
    status, body = call(remote, "PUT", draft, phone, "half a thought")
    assert status == 200, (status, body)
    assert call(remote, "GET", draft, phone) == (200, b"half a thought")
    events = open_(remote, "GET", f"/sessions/{session}/events?after={len(stored)}",
                   phone)
    attached = lines(events)
    assert attached == {"kind": "attached", "session": session, "last": len(stored), "pending": []}, attached
    status, body = call(remote, "POST", f"/sessions/{session}/input", phone,
                        {"text": "Again.", "skill": "hi"})
    assert status == 200, (status, body)
    entry = json.loads(body)["entry"]
    streamed = []
    while not streamed or streamed[-1]["kind"] != "turn_end":
        line = lines(events)
        assert line is not None, streamed
        if line["kind"] not in ("delta", "taken", "tail", "attached"):
            streamed.append(line)
    assert streamed[0]["id"] == entry and streamed[0]["kind"] == "input", streamed
    skill = '<skill name="hi">\n---\ndescription: Says hi.\n---\nSay hi first.\n\n</skill>\n\n'
    assert streamed[0]["text"] == skill + "Again.", streamed[0]
    assert streamed[-1]["inputs"] == [entry], streamed[-1]
    assert streamed == [bare(e) for e in f.record(session)][len(stored):], streamed
    assert call(remote, "GET", draft, phone) == (200, b"")

    # 6. No file name leaves the session's directory; a file that is not there is 404.
    for name, refused in (("../../device.key", 400), ("..%2F..%2Fdevice.key", 400),
                          ("nothing", 404)):
        status, body = call(remote, "GET", f"/sessions/{session}/files/{name}", phone)
        assert status == refused and json.loads(body)["error"], (name, status, body)

    # 7. Each status stream opens with every agent, with its newest turn's cache share; removing
    # the phone ends its stream and its
    # key, and no other device's.
    watched = open_(remote, "GET", "/status", phone)
    rows = lines(watched)
    assert rows["kind"] == "agents" and session in [a["session"] for a in rows["agents"]], rows
    usage = [e for e in f.record(session) if e["kind"] == "turn_end"][-1]["usage"]
    share = usage["cache_read"] / (usage["input"] + usage["cache_read"] + usage["cache_write"])
    row = next(a for a in rows["agents"] if a["session"] == session)
    assert share > 0 and row["cache"] == share, (usage, row)
    assert row.get("calls_made") == 2 and row["failed"] == 1 and row["calls"] == [], row
    mine = open_(local, "GET", "/status", key)
    assert lines(mine)["kind"] == "agents"
    removed = f.run("devices", "remove", "phone")
    assert removed.returncode == 0, removed.stderr
    assert lines(watched) is None, "the phone's status stream outlived its key"
    assert call(remote, "GET", "/sessions", phone)[0] == 401
    resumed = f.run("--resume", session, "--print=json", "Once more.")
    assert resumed.returncode == 0, resumed.stderr
    line = {}
    while session not in [row.get("session") for row in line.get("agents", [line])]:
        line = lines(mine)
        assert line is not None, "the local key's status stream ended with the phone's"

    # 8. The project route reads this folder now, not the branch recorded for a child.
    f.base()
    project = f"/sessions/{session}/project"
    status, body = call(local, "GET", project, key)
    assert status == 200 and json.loads(body) == {"commits": 1, "merges": 0, "branch": "main", "changed": 0}, (status, body)
    # Uncommitted files count from git (Trey, 2026-10-30), so a fresh session in a dirty tree shows them.
    f.git("checkout", "--detach")
    (f.cwd / "loose").write_text("x")
    status, body = call(local, "GET", project, key)
    assert status == 200 and json.loads(body) == {"commits": 1, "merges": 0, "changed": 1}, (status, body)
    (f.cwd / "loose").unlink()
    outside = f.home / "outside"
    outside.mkdir()
    status, body = call(local, "POST", "/sessions", key, {"cwd": str(outside)})
    assert status == 200, (status, body)
    plain = json.loads(body)["session"]
    assert call(local, "GET", f"/sessions/{plain}/project", key) == (200, b"{}")
    assert server.agents(f)[plain].get("calls_made") == 0
    # 9. A reader's report of the last entry it saw sits on the row, with when, until a restart.
    last, before = list(f.record(session))[-1]["id"], int(time.time() * 1000)
    assert call(local, "PUT", f"/sessions/{session}/seen?entry={last}", key) == (200, b"{}")
    row = server.agents(f)[session]
    assert row.get("seen") == last and row.get("looked_at", 0) >= before, row
    server.stop(f)
    server.start(f)
    rows = server.agents(f)
    assert rows[session].get("calls_made") == 2 and rows[session]["failed"] == 1, rows[session]
    assert "seen" not in rows[session] and "looked_at" not in rows[session], rows[session]
    assert rows[plain].get("calls_made") == 0, rows[plain]

# 10. A GPT response entry, about 100 KB of replay data on the sign-in, reaches a stream and a
# page without `native` by default and whole with `native=true`.
with flow([done()], model="gpt-6.1-sol") as f:
    server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    streams = [server.send(f, "GET", f"/sessions/{session}/events?after=0{flag}") for flag in ("", "&native=true")]
    server.post(f, f"/sessions/{session}/input", {"text": "Say done."})
    stored = next(e for e in server.until(f, session, lambda es: es[-1]["kind"] == "turn_end") if e["kind"] == "response")
    got = [next(line for line in iter(lambda s=s: lines(s), None) if line["kind"] == "response") for s in streams]
    assert stored["native"]["responses"]["items"] and got == [bare(stored), stored], got
    line = (f.home / ".razor/sessions" / session / "record.jsonl").read_bytes().splitlines(True)[stored["id"] - 1]
    page = f"/sessions/{session}/entries?before={stored['id'] + 1}&limit=1"
    assert server.send(f, "GET", page + "&native=true").read() == line
    assert json.loads(server.send(f, "GET", page).read()) == bare(stored)

# Paired enrollment remains authoritative if its file becomes unreadable at runtime;
# search still reports the unavailable peer, and revocation immediately removes it.
with flow([], config='[server]\naddress="127.0.0.1:0"') as f:
    server.start(f)
    assert f.run("devices", "add", "down", "--address", "127.0.0.1:1").returncode == 0
    path = f.home / ".razor/devices.json"
    original = path.read_bytes()
    for failure in ("parse", "read"):
        if failure == "parse":
            path.write_text("{broken")
        else:
            path.unlink()
            path.mkdir()
        answer = json.loads(server.send(f, "GET", "/search?q=enrollment").read())
        assert answer["partial"], answer
        if failure == "read":
            path.rmdir()
        path.write_bytes(original)
    assert f.run("devices", "remove", "down").returncode == 0
    assert not json.loads(server.send(f, "GET", "/search?q=enrollment").read())["partial"]

# Invalid enrollment must refuse startup rather than silently forget paired servers.
for failure in ("parse", "read"):
    with flow([]) as f:
        path = f.home / ".razor/devices.json"
        if failure == "parse":
            path.write_text("{broken")
        else:
            path.mkdir()
        result = f.run("serve", timeout=5)
        assert result.returncode != 0 and "devices.json" in result.stderr, result
