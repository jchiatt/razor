"""Phone meetings through the server, against an external recorder stand-in."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import uuid

from driver import flow
from responses import PLAIN, call, reply, response as model_response
import server

standin = Path(__file__).with_name("meetrec.py").resolve()
with tempfile.TemporaryDirectory(prefix="meetrec-bin-") as bin_dir:
    (Path(bin_dir) / "meetrec").symlink_to(standin)
    linux = sys.platform == "linux"
    if linux:
        # Inject sync errors and order a pending write after a separate handle's sync.
        source = Path(bin_dir) / "sync.c"
        source.write_text("#define _GNU_SOURCE\n#include <dlfcn.h>\n#include <errno.h>\n#include <pthread.h>\n#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n#include <sys/stat.h>\n#include <time.h>\n#include <unistd.h>\n" + r"""
    static pthread_mutex_t mu = PTHREAD_MUTEX_INITIALIZER;
    static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
    static int synced, written = -1;
    static int marked(int fd, const char *name) {
        char link[64], path[4096], marker[4096];
        snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
        ssize_t n = readlink(link, path, sizeof(path) - 1);
        if (n > 0) {
            path[n] = 0;
            char *root = getenv("MEETING_SYNC_FAILURE");
            char *tail = strrchr(path, '/');
            if (root && tail && (!name || strcmp(tail, "/room.live.wav") == 0)) {
                snprintf(marker, sizeof(marker), "%s/%s", root, name ? name : tail + 1);
                return access(marker, F_OK) == 0;
            }
        }
        return 0;
    }
    static void event(const char *event, int fd) {
        char path[4096];
        struct stat st;
        fstat(fd, &st);
        snprintf(path, sizeof(path), "%s/order.log", getenv("MEETING_SYNC_FAILURE"));
        FILE *out = fopen(path, "a");
        if (out) { fprintf(out, "%s size=%ld\n", event, (long)st.st_size); fclose(out); }
    }
    static void wait_for(int *done) {
        struct timespec until;
        clock_gettime(CLOCK_REALTIME, &until);
        until.tv_sec += 2;
        while (!*done) if (pthread_cond_timedwait(&changed, &mu, &until)) break;
    }
    ssize_t write(int fd, const void *buf, size_t len) {
        ssize_t (*write_next)(int, const void*, size_t) = dlsym(RTLD_NEXT, "write");
        if (!marked(fd, "order")) return write_next(fd, buf, len);
        pthread_mutex_lock(&mu);
        written = 0;
        event("write entered", fd);
        wait_for(&synced);
        ssize_t n = write_next(fd, buf, len);
        event("write completed", fd);
        written = 1; pthread_cond_broadcast(&changed);
        pthread_mutex_unlock(&mu);
        return n;
    }
    int fsync(int fd) {
        int (*sync_next)(int) = dlsym(RTLD_NEXT, "fsync");
        if (marked(fd, NULL)) { errno = EIO; return -1; }
        if (!marked(fd, "order")) return sync_next(fd);
        pthread_mutex_lock(&mu);
        // A reader's recovery barrier before the upload begins is unrelated to its write.
        if (written < 0) { pthread_mutex_unlock(&mu); return sync_next(fd); }
        int n = sync_next(fd);
        event("fsync completed", fd);
        synced = 1; pthread_cond_broadcast(&changed);
        wait_for(&written);
        pthread_mutex_unlock(&mu);
        return n;
    }
    """)
        shim = Path(bin_dir) / "sync.so"
        subprocess.run(["cc", "-shared", "-fPIC", "-o", str(shim), str(source), "-ldl", "-pthread"], check=True)
    with flow([reply(None, "Ready.") for _ in range(4)], model="sol",
              env={"PATH": bin_dir + ":" + os.environ["PATH"], **({"LD_PRELOAD": str(shim),
                   "MEETING_SYNC_FAILURE": bin_dir} if linux else {})}) as f:
        server.start(f)
        added = f.run("devices", "add", "phone")
        assert added.returncode == 0, added.stderr
        phone = added.stdout.splitlines()[1]
        session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        other = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        meeting = str(uuid.uuid4())
        route = f"/sessions/{session}/meetings/{meeting}"
        start = {"started_at": "2026-11-04T20:00:00Z", "rate": 48000, "attendees": ["Alice"]}

        def request(method, path, body=None, status=200, headers=None):
            response = server.send(f, method, path, body, headers, key=phone)
            data = response.read()
            assert response.status == status, (method, path, response.status, data)
            return json.loads(data) if data and response.getheader("Content-Type", "").startswith("application/json") else data

        def commands():
            path = f.home / "Recordings/meetings/commands.jsonl"
            return [json.loads(row) for row in path.read_text().splitlines()]

        root = f.home / "Recordings/meetings"
        (root / "lose-start-reply").touch()
        request("PUT", route, start, status=500)
        request("PUT", route, start)
        request("PUT", route, start)
        assert sum(p.is_dir() for p in f.cwd.iterdir()) == 1, "start retry lost or duplicated its meeting folder"
        assert sum(c["args"][0] == "start" for c in commands()) == 2
        request("PUT", f"/sessions/{session}/meetings/{uuid.uuid4()}", start, status=409)
        request("GET", f"/sessions/{other}/meetings/{meeting}", status=404)
        facts = server.until(f, session, lambda es: any(e.get("event") == "started" and e.get("source") == "meeting" for e in es))
        folder = Path(next(e["folder"] for e in facts if e.get("source") == "meeting"))
        assert folder.parent == f.cwd, "meeting was saved outside its session's folder"
        wav = folder / "room.live.wav"

        def live(offset, data, status=200):
            return request("PUT", route + f"/live?offset={offset}", data, status)

        assert live(4, b"abcd")["live"] == 0 and wav.stat().st_size == 44
        if linux:
            fault = Path(bin_dir) / "room.live.wav"
            fault.touch()
            live(0, b"abcd", status=500)
            request("GET", route, status=500)
            fault.unlink()
            assert request("GET", route)["live"] == 4
        assert live(0, b"abcd")["live"] == 4 and wav.read_bytes()[44:] == b"abcd"
        if linux:
            (Path(bin_dir) / "order").touch()
        assert live(2, b"cdef")["live"] == 6 and wav.read_bytes()[44:] == b"abcdef"
        if linux:
            (Path(bin_dir) / "order").unlink()
            events = (Path(bin_dir) / "order.log").read_text()
            assert events.index("write completed") < events.index("fsync completed"), events
            assert "fsync completed size=50" in events, events
        live(6, b"x", status=400)
        assert wav.read_bytes()[44:] == b"abcdef"
        assert request("PUT", route + "/speakers/2", {"name": "Alice"})["speakers"]["names"]["2"] == "Alice"
        assert request("PUT", route + "/pauses/0", {"at_s": 2.0, "missing_s": 5.0})["pauses"][0]["k"] == 0
        request("PUT", route + "/pauses/0", {"at_s": 2.0, "missing_s": 5.0})
        assert len(json.loads((folder / "pauses.json").read_text())) == 1

        def add_lines(first, last, size=0):
            with (folder / "live.jsonl").open("a") as out:
                for n in range(first, last):
                    out.write(json.dumps({"kind": "line", "seq": n + 1, "speaker": "2", "who": "Alice",
                                          "text": f"line{n:03d}." + "x" * size, "start": n + .25, "end": n + .75,
                                          "arrived_at": "2026-11-04T20:00:00Z"}) + "\n")

        def lines(es):
            return [e for e in es if e.get("source") == "meeting" and e.get("event") == "line"]

        add_lines(0, 1)
        entries = server.until(f, session, lambda es: len(lines(es)) == 1)
        assert (lines(entries)[0]["who"], lines(entries)[0]["name"], lines(entries)[0]["label"], lines(entries)[0]["start_ms"]) == ("room", "Alice", "2", 250)
        assert not any(e["kind"] == "turn_end" for e in entries), "live transcript started a turn"
        server.wait(lambda: (folder / "flow-cursor").exists() and (folder / "flow-cursor").read_text() == "1")
        follow = server.wait(lambda: next((c for c in reversed(commands()) if c["args"][0] == "live" and "--follow" in c["args"]), None))
        os.kill(follow["pid"], signal.SIGKILL)
        add_lines(1, 2)
        entries = server.until(f, session, lambda es: len(lines(es)) == 2, timeout=20)
        assert [e["text"] for e in lines(entries)] == ["line000.", "line001."]

        server.stop(f)
        before = len(commands())
        server.start(f)
        server.wait(lambda: any(c["args"][0] == "ingest" for c in commands()[before:]), timeout=20)
        server.wait(lambda: any(c["args"][0] == "live" and "--follow" in c["args"] for c in commands()[before:]), timeout=20)
        for first in range(2, 300, 50):
            last = min(first + 50, 300)
            add_lines(first, last)
            entries = server.until(f, session, lambda es: len(lines(es)) == last, timeout=30)
        assert not any(e["kind"] == "turn_end" for e in entries)
        server.post(f, f"/sessions/{session}/input", {"text": "What happened?"})
        server.until(f, session, lambda es: any(e["kind"] == "turn_end" for e in es))
        body = json.loads((f.requests / "001.body").read_text())
        view = json.dumps(body)
        assert re.findall(r"line(\d{3})\.", view) == [f"{n:03d}" for n in range(44, 300)]
        assert "44 earlier" in view and "(+0:44), Alice said: line044." in view
        add_lines(300, 302, size=40000)
        server.until(f, session, lambda es: len(lines(es)) == 302)
        server.post(f, f"/sessions/{session}/input", {"text": "And now?"})
        server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 2)
        view = json.dumps(json.loads((f.requests / "002.body").read_text()))
        assert "line301." in view and "line300." not in view and "1 earlier" in view

        piece = b"a full wav piece"
        digest = hashlib.sha256(piece).digest()
        headers = {"Content-Digest": "sha-256=:" + base64.b64encode(digest).decode() + ":"}
        manifest = {"live_bytes": 8, "pieces": [{"bytes": len(piece), "sha256": digest.hex()}]}
        status = request("PUT", route + "/end", manifest)
        assert status["missing"] == [0] and not (folder / "pieces/complete.json").exists()
        assert (folder / "pieces/expected.json").exists()
        server.stop(f)
        server.start(f)
        assert request("GET", route)["missing"] == [0]
        request("PUT", route + "/pieces/0", b"wrong", status=400, headers=headers)
        if linux:
            fault = Path(bin_dir) / "pieces"
            fault.touch()
            request("PUT", route + "/pieces/0", piece, status=500, headers=headers)
            request("GET", route, status=500)
            fault.unlink()
            assert request("GET", route)["pieces"] == [0]
        request("PUT", route + "/pieces/0", piece, headers=headers)
        assert not (folder / "pieces/complete.json").exists()
        with wav.open("ab") as file:
            file.write(b"x")  # Simulate a crash after half a PCM frame reached the file.
        assert live(6, b"gh")["live"] == 8
        assert wav.read_bytes()[44:] == b"abcdefgh"
        complete = server.wait(lambda: (folder / "pieces/complete.json").exists())
        assert complete and (folder / "pieces/room.0000.wav").read_bytes() == piece
        request("PUT", route + "/pieces/0", b"different", status=409,
                headers={"Content-Digest": "sha-256=:" + base64.b64encode(hashlib.sha256(b"different").digest()).decode() + ":"})
        (folder / "room.wav").write_bytes(b"0123456789")
        (folder / "transcript.md").write_text("Alice: Hello.\n")
        (folder / "transcript.json").write_text('{"words": []}')
        (folder / "final.json").write_text(json.dumps({"v": 1, "ok": True, "transcript": "transcript.json", "text": "transcript.md", "duration_s": 2, "speakers": ["Alice"]}))
        entries = server.until(f, session, lambda es: any(e.get("meeting") == meeting for e in es), timeout=20)
        notes = [e for e in entries if e.get("meeting") == meeting]
        assert len(notes) == 1 and notes[0].get("from") == "razor" and notes[0].get("device") == "phone" and notes[0].get("request") == meeting, notes
        assert request("GET", route + "/transcript")
        response = server.send(f, "GET", route + "/audio", headers={"Range": "bytes=2-4"}, key=phone)
        assert response.status == 206 and response.read() == b"234" and response.getheader("Accept-Ranges") == "bytes"
        response = server.send(f, "GET", route + "/audio", headers={"Range": "bytes=99-"}, key=phone)
        assert response.status == 416
        server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 3)
        server.stop(f)
        server.start(f)
        request("GET", route)
        assert len([e for e in f.record(session) if e.get("meeting") == meeting]) == 1
        request("DELETE", route)
        assert not any(e.get("call") == meeting and e.get("reason") == "canceled" for e in f.record(session)), "finished meeting was canceled"
        assert request("GET", route)["phase"] == "ready"
        retry = str(uuid.uuid4())
        retry_route = f"/sessions/{session}/meetings/{retry}"
        request("PUT", retry_route, start)
        entries = server.until(f, session, lambda es: any(e.get("call") == retry and e.get("event") == "started" for e in es))
        retry_folder = Path(next(e["folder"] for e in entries if e.get("call") == retry and e.get("event") == "started"))
        request("PUT", retry_route + "/end", {"live_bytes": 0, "pieces": []})
        (retry_folder / "final.json").write_text(json.dumps({"v": 1, "ok": False, "step": "words", "error": "unavailable"}))
        server.until(f, session, lambda es: any(e.get("meeting") == retry for e in es), timeout=20)
        server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 4)
        assert request("POST", retry_route + "/retry")["phase"] == "transcribing"
        assert any(c["args"][0] == "ingest" and "--retry" in c["args"] for c in commands())
        cancel_route = f"/sessions/{session}/meetings/{uuid.uuid4()}"
        request("PUT", cancel_route, start)
        request("DELETE", cancel_route)
        request("PUT", cancel_route + "/end", {"live_bytes": 0, "pieces": []})
        canceled_id = cancel_route.rsplit("/", 1)[1]
        assert [e["reason"] for e in f.record(session) if e.get("call") == canceled_id and e.get("event") == "ended"] == ["canceled"]
        assert any(c["args"][0] == "cancel" for c in commands())
        bad_id = str(uuid.uuid4())
        bad_route = f"/sessions/{session}/meetings/{bad_id}"
        request("PUT", bad_route, start)
        entries = server.until(f, session, lambda es: any(e.get("call") == bad_id and e.get("event") == "started" for e in es))
        bad_folder = Path(next(e["folder"] for e in entries if e.get("call") == bad_id and e.get("event") == "started"))
        request("PUT", bad_route + "/pieces/0", piece, headers=headers)
        request("PUT", bad_route + "/end", {"live_bytes": 0, "pieces": [{"bytes": len(piece), "sha256": "0" * 64}]}, status=400)
        assert not (bad_folder / "pieces/complete.json").exists()
        assert not (bad_folder / "pieces/expected.json").exists(), "rejected end persisted a manifest"
        assert not any(e.get("call") == bad_id and e.get("event") == "ended" for e in f.record(session))
        assert request("GET", bad_route)["phase"] == "recording"
        request("PUT", bad_route + "/end", {"live_bytes": 0, "pieces": [{"bytes": len(piece), "sha256": digest.hex()}]})
        assert (bad_folder / "pieces/complete.json").exists()
        request("DELETE", bad_route)

    # An answered start stays available when the model's context is rebuilt.
    notes = [model_response([call("handover", json.dumps({"note": "Continue following the meeting."}), f"note{n}")]) for n in range(2)]
    with flow([reply(None, "Ready."), notes[0], reply(None, "Rebuilt."), notes[1], reply(None, "Rebuilt again.")],
              model="sol", config=PLAIN, env={"PATH": bin_dir + ":" + os.environ["PATH"]}) as f:
        server.start(f)
        session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        route = f"/sessions/{session}/meetings/{uuid.uuid4()}"
        response_ = server.send(f, "PUT", route, {"started_at": "2026-11-04T20:00:00Z", "rate": 48000})
        assert response_.status == 200, response_.read()
        response_.read()
        entries = server.until(f, session, lambda es: any(e.get("event") == "started" for e in es))
        folder = Path(next(e["folder"] for e in entries if e.get("event") == "started"))
        server.post(f, f"/sessions/{session}/input", {"text": "Follow the meeting."})
        server.until(f, session, lambda es: any(e["kind"] == "turn_end" for e in es))
        for n in range(2):
            if n:
                response_ = server.send(f, "PUT", route + "/end", {"live_bytes": 2, "pieces": []})
                assert response_.status == 200, response_.read()
                response_.read()
            server.post(f, f"/sessions/{session}/handover", {})
            entries = server.until(f, session, lambda es: len([e for e in es if e["kind"] == "rebuild"]) == n + 1)
            rebuilt = [e for e in entries if e["kind"] == "rebuild"][-1]
            assert str(folder) in rebuilt["context"], "rebuild lost the open meeting's folder path"
            server.until(f, session, lambda es: any(e["kind"] == "turn_end" and e["id"] > rebuilt["id"] for e in es))
        response_ = server.send(f, "DELETE", route)
        assert response_.status == 200, response_.read()
        response_.read()

    # A helper's meeting outlives its disposable worktree.
    with flow([reply("Make a helper.", "agent", action="start", name="meeting-helper", brief="Wait.", placement="worktree"),
               reply("Make a helper.", "Ready."), reply("You are meeting-helper,", "agent", action="wait", timeout_ms=600000)],
              model="sol", config=PLAIN, env={"PATH": bin_dir + ":" + os.environ["PATH"]}) as f:
        f.base()
        server.start(f)
        parent = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
        server.post(f, f"/sessions/{parent}/input", {"text": "Make a helper."})
        entries = server.until(f, parent, lambda es: any(e["kind"] == "child" for e in es))
        child = next(e["session"] for e in entries if e["kind"] == "child")
        server.until(f, child, lambda es: any(e["kind"] == "call" for e in es))
        route = f"/sessions/{child}/meetings/{uuid.uuid4()}"
        response_ = server.send(f, "PUT", route, {"started_at": "2026-11-04T20:00:00Z", "rate": 48000})
        assert response_.status == 200, response_.read()
        response_.read()
        entries = server.until(f, child, lambda es: any(e.get("event") == "started" for e in es))
        folder = Path(next(e["folder"] for e in entries if e.get("event") == "started"))
        assert folder.parent == f.cwd, "meeting was saved inside the helper's worktree"
        server.until(f, parent, lambda es: any(e["kind"] == "turn_end" for e in es))
        response_ = server.send(f, "DELETE", route)
        assert response_.status == 200, response_.read()
        response_.read()
