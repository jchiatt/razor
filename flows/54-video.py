"""A video: a Gemini session sees it, uploaded to the Files API, in the turn it starts; any other
session gets a watcher, a child on Gemini that reports and keeps the video for questions, across a
rebuild too; two videos at once get two watchers; with no key there is no watcher
(docs/design/video.md)."""
import json
import threading
from pathlib import Path

from driver import flow
from responses import PLAIN, reply as gpt
import server
from standin import Reply

GEMINI = "gemini-3.8-flash"
HEAD = b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00isomiso2"
LATER = "2099-01-01T00:00:00Z"


def chat(match, text):
    """A Gemini chat stream, picked by a phrase in its request, ending in `text`."""
    chunks = [{"choices": [{"index": 0, "delta": {"role": "assistant", "content": text}}]},
              {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
              {"choices": [], "usage": {"prompt_tokens": 800, "completion_tokens": 9, "total_tokens": 809}}]
    body = "".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n"
    return Reply(body.encode(), match=match)


def files(url, name, state, match, **extra):
    """The Files API's answer for one file: to its raw upload (`match`, a phrase of the bytes),
    or to a read of its state (`match` None)."""
    file = {"name": f"files/{name}", "uri": f"{url}/v1beta/files/{name}", "state": state,
            "expirationTime": LATER, **extra}
    if match is None:
        return Reply(json.dumps(file).encode(), method="GET", match=f"/v1beta/files/{name}")
    return Reply(json.dumps({"file": file}).encode(), match=match)


replies = []
config = (f'{PLAIN}[models."{GEMINI}"]\nbase_url = "{{standin}}/v1beta/openai"\n'
          '[models.gemini-two]\nbase_url = "{standin}/v1beta/openai"\nid = "gemini-3.8-flash"\n'
          # A parent whose every request goes to a closed port, so it sends the stand-in nothing.
          '[models.mute]\nbase_url = "http://127.0.0.1:9/v1"\nkey = "GEMINI_API_KEY"\n')
with flow(replies, model="sol", config=config, env={"GEMINI_API_KEY": "flow-gemini-key"}) as f:
    url = f.env["ANTHROPIC_BASE_URL"]
    server.start(f)

    def call(method, path, body=b""):
        answer = server.send(f, method, path, body)
        return answer.status, dict(answer.getheaders()), answer.read()

    def upload(session, bytes, name="video.mp4"):
        return json.loads(call("POST", f"/sessions/{session}/files/{name}", HEAD + bytes)[2])["path"]

    def spooled(kind):
        """Each request so far of `kind` ("upload", "state", "gemini", "gpt"): body and metadata."""
        found = []
        for meta in sorted(f.requests.glob("*.json"), key=lambda p: json.loads(p.read_text())["arrived"]):
            m, body = json.loads(meta.read_text()), meta.with_suffix(".body").read_bytes()
            # Anything else on the machine may knock on the stand-in's port; only Razor's reads count.
            got = ("upload" if m["path"] == "/upload/v1beta/files" else "gemini" if m["path"].endswith(
                "/chat/completions") else "state" if m["path"].startswith("/v1beta/files/") else
                   "gpt" if m["path"] == "/responses" else "other")
            if got == kind:
                found.append((body if kind in ("upload", "state") else json.loads(body), m))
        return found

    def turns(session, n):
        return server.until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) >= n)

    # 1. A Gemini session: the video is uploaded raw with the key Google's Files API takes, read
    # until ACTIVE, and sent as an image_url of its uri before the words, after the path's line.
    replies += [files(url, "one", "PROCESSING", "flow video one"), files(url, "one", "ACTIVE", None),
                chat("Why does it stutter", "It stutters at 0:04."), chat("And now", "Still fine.")]
    gemini = server.post(f, "/sessions", {"cwd": str(f.cwd), "model": GEMINI})["session"]
    path = upload(gemini, b"flow video one")
    video = {"file": path, "duration_ms": 8000}
    sent = {"text": "Why does it stutter?", "video": {**video, "watcher": "forged"}, "request": "video-1"}
    entry = server.post(f, f"/sessions/{gemini}/input", sent)["entry"]
    entries = turns(gemini, 1)
    assert next(e for e in entries if e["id"] == entry)["video"] == video, entries
    [(body, meta)] = spooled("upload")
    assert body == HEAD + b"flow video one", body
    assert (meta["headers"]["x-goog-api-key"], meta["headers"]["x-goog-upload-protocol"],
            meta["headers"]["content-type"]) == ("flow-gemini-key", "raw", "video/mp4"), meta
    assert [m["path"] for _, m in spooled("state")] == ["/v1beta/files/one"]
    full = str(f.home / ".razor/sessions" / gemini / path)
    asked = spooled("gemini")[0][0]["messages"][-1]["content"]
    assert asked[0] == {"type": "image_url", "image_url": {"url": f"{url}/v1beta/files/one"}}, asked
    assert asked[1]["text"].endswith(f"\n\n(Video: {full}, 0:08 long.)\nWhy does it stutter?"), asked
    # A repeat of the request admits nothing and starts nothing.
    assert server.post(f, f"/sessions/{gemini}/input", sent)["entry"] == entry
    # The next turn: no upload, and the video's place holds a line saying it is no longer shown.
    server.post(f, f"/sessions/{gemini}/input", {"text": "And now?"})
    turns(gemini, 2)
    assert len(spooled("upload")) == 1
    earlier = spooled("gemini")[1][0]["messages"]
    shown = [m["content"][0] for m in earlier if m["role"] == "user" and "Why does it stutter" in json.dumps(m)]
    assert shown == [{"type": "text", "text": f"(The video {full} was in an earlier turn and is no longer "
                      "shown; ffmpeg can cut its frames for you to look at again.)"}], shown

    # 2. A failed upload is shown as a line saying why, Google's words without the key, and the
    # turn goes on.
    replies += [files(url, "bad", "FAILED", "flow video bad", error={"message": "bad codec, key flow-gemini-key"}),
                chat("This one fails", "I could not see it.")]
    failing = server.post(f, "/sessions", {"cwd": str(f.cwd), "model": GEMINI})["session"]
    bad = upload(failing, b"flow video bad")
    server.post(f, f"/sessions/{failing}/input", {"text": "This one fails.", "video": {"file": bad, "duration_ms": 1000}})
    turns(failing, 1)
    asked = spooled("gemini")[2][0]["messages"][-1]["content"]
    assert asked[0]["text"] == (f"The video {f.home / '.razor/sessions' / failing / bad} could not be sent "
                                "to the model: Google could not process it: bad codec, key [key removed]"), asked
    record = (f.home / ".razor/sessions" / failing / "record.jsonl").read_bytes()
    assert b"flow-gemini-key" not in record + json.dumps(spooled("gemini")[2][0]).encode()

    # 3. A GPT session: admitted at once naming its watcher, which starts at that entry on Gemini
    # in the video's folder with the person's words and the parent's last reply, and reports.
    replies += [gpt("Hello parent", "Send me the clip."), gpt("it stutters", "Waiting for watcher-1."),
                files(url, "two", "ACTIVE", "flow video two"),
                chat("Machine-made description of", "Machine-made description: it freezes at 0:04."),
                gpt("reports finished", "Thanks.")]
    parent = server.post(f, "/sessions", {"cwd": str(f.cwd), "model": "sol"})["session"]
    server.post(f, f"/sessions/{parent}/input", {"text": "Hello parent"})
    turns(parent, 1)
    two = upload(parent, b"flow video two")
    # Two sends of one request at once: one admission, one watcher, no failure to start.
    sent, answers = {"text": "Here; it stutters.", "video": {"file": two, "duration_ms": 65000}, "request": "watch-1"}, []
    threads = [threading.Thread(target=lambda: answers.append(server.post(f, f"/sessions/{parent}/input", sent)))
               for _ in range(2)]
    [t.start() for t in threads] and [t.join() for t in threads]
    [entry] = {a["entry"] for a in answers}
    entries = turns(parent, 3)
    assert not [e for e in entries if "could not start" in e.get("text", "")], entries
    assert next(e for e in entries if e["id"] == entry)["video"]["watcher"] == "watcher-1"
    status, _, body = call("GET", f"/sessions/{parent}/children")
    [row] = json.loads(body)
    assert (row["name"], row["launched"], row["model"]) == ("watcher-1", entry, GEMINI), row
    watcher = row["session"]
    full = str(f.home / ".razor/sessions" / parent / two)
    asked = json.dumps([b for b, _ in spooled("gpt") if "it stutters" in json.dumps(b)][0]["input"])
    assert f"(Video: {full}, 1:05 long. Your helper watcher-1 is watching it" in asked, asked
    first = spooled("gemini")[3][0]["messages"]
    brief = json.dumps(first[-1]["content"])
    assert first[-1]["content"][0] == {"type": "image_url", "image_url": {"url": f"{url}/v1beta/files/two"}}
    assert all(w in brief for w in ("Here; it stutters.", "Send me the clip.", f.cwd.name, full)), brief
    start = next(f.record(watcher))
    assert start["cwd"] == str(Path(full).parent), start

    # 4. The person's question to the finished watcher continues it; it resumes with the video,
    # uploaded once, and its report went by its clean turn end, not by a finish call.
    replies += [chat("What happens at 0:05", "At 0:05 it recovers.")]
    server.post(f, f"/sessions/{watcher}/input", {"text": "What happens at 0:05?"})
    entries = turns(watcher, 2)
    assert not [e for e in entries if e["kind"] in ("rebuild", "call")], [e["kind"] for e in entries]
    again = spooled("gemini")[4][0]["messages"]
    assert again[1]["content"][0] == {"type": "image_url", "image_url": {"url": f"{url}/v1beta/files/two"}}, again[1]
    assert len(spooled("upload")) == 3
    # Three questions later its first input is past the four asks a rebuild carries verbatim; a
    # model switch's rebuild still shows its video, with its words.
    for n in (2, 3, 4):
        replies.append(chat(f"Question {n}", "Answered."))
        server.post(f, f"/sessions/{watcher}/input", {"text": f"Question {n}?"})
        turns(watcher, n + 1)
    replies += [files(url, "two", "ACTIVE", "flow video two"), chat("On the other model", "Still see it.")]
    server.post(f, f"/sessions/{watcher}/input", {"text": "On the other model now.", "model": "gemini-two"})
    entries = turns(watcher, 6)
    assert [e["trigger"] for e in entries if e["kind"] == "rebuild"] == ["change"], entries
    rebuilt = [b for b, _ in spooled("gemini") if "On the other model" in json.dumps(b)][0]["messages"][-1]
    assert rebuilt["content"][0] == {"type": "image_url", "image_url": {"url": f"{url}/v1beta/files/two"}}, rebuilt
    assert "Here; it stutters." in rebuilt["content"][1]["text"], rebuilt

    # 5. A video whose input moves a GPT session to Gemini follows the rebuild that switch makes, with its words last.
    replies += [gpt("Before the switch", "Ready."), files(url, "four", "ACTIVE", "flow video four"),
                chat("Now on Gemini", "I see it.")]
    switch = server.post(f, "/sessions", {"cwd": str(f.cwd), "model": "sol"})["session"]
    server.post(f, f"/sessions/{switch}/input", {"text": "Before the switch."})
    turns(switch, 1)
    four = upload(switch, b"flow video four")
    server.post(f, f"/sessions/{switch}/input", {"text": "Now on Gemini.", "model": GEMINI,
                                                  "video": {"file": four, "duration_ms": 3000}})
    entries = turns(switch, 2)
    assert [e["trigger"] for e in entries if e["kind"] == "rebuild"] == ["change"], entries
    asked = [b for b, _ in spooled("gemini") if "Now on Gemini" in json.dumps(b)][0]["messages"][-1]["content"]
    assert asked[-2] == {"type": "image_url", "image_url": {"url": f"{url}/v1beta/files/four"}} and asked[-1]["text"].endswith("Now on Gemini."), asked

    # 6. Two different videos at once take two names, and neither start fails.
    replies += [files(url, "p1", "ACTIVE", "flow video p1"), files(url, "p2", "ACTIVE", "flow video p2"),
                chat("pair one", "Seen one."), chat("pair two", "Seen two.")]
    mute = server.post(f, "/sessions", {"cwd": str(f.cwd), "model": "mute"})["session"]
    sends = [{"text": f"pair {w}", "video": {"file": upload(mute, b"flow video " + v), "duration_ms": 1000}}
             for w, v in (("one", b"p1"), ("two", b"p2"))]
    threads = [threading.Thread(target=server.post, args=(f, f"/sessions/{mute}/input", s)) for s in sends]
    [t.start() for t in threads] and [t.join() for t in threads]
    rows = json.loads(call("GET", f"/sessions/{mute}/children")[2])
    assert sorted(r["name"] for r in rows) == ["watcher-1", "watcher-2"], rows
    for row in rows:
        turns(row["session"], 1)
    assert not [e for e in f.record(mute) if "could not start" in e.get("text", "")]

    # 7. With the watcher's key unset, a GPT session's video gets its path line alone; a file that
    # is not a video is refused before admission; the status says this server takes video.
    config_file = f.home / ".razor/config.toml"
    config_file.write_text(config_file.read_text().replace(
        f'base_url = "{url}/v1beta/openai"', f'base_url = "{url}/v1beta/openai"\nkey = "FLOW_UNSET_KEY"'))
    replies += [gpt("Without a key", "Noted.")]
    lone = server.post(f, "/sessions", {"cwd": str(f.cwd), "model": "sol"})["session"]
    three = upload(lone, b"flow video three")
    entry = server.post(f, f"/sessions/{lone}/input", {"text": "Without a key.", "video": {"file": three, "duration_ms": 2000}})["entry"]
    entries = turns(lone, 1)
    assert "watcher" not in next(e for e in entries if e["id"] == entry)["video"]
    assert json.loads(call("GET", f"/sessions/{lone}/children")[2]) == []
    text = json.loads(call("POST", f"/sessions/{lone}/files/notes.mp4", b"plain words, not a video")[2])["path"]
    status, _, body = call("POST", f"/sessions/{lone}/input", {"text": "x", "video": {"file": text, "duration_ms": 1}})
    assert (status, json.loads(body)["error"]) == (400, "Send the video as an MP4 or QuickTime file uploaded to this session."), body
    assert len(spooled("upload")) == 7
    status, headers, _ = call("GET", f"/sessions/{lone}/files/{three}")
    assert (status, headers["content-type"]) == (200, "video/mp4"), headers
    assert json.loads(server.send(f, "GET", "/status").readline())["server"]["video"] is True
