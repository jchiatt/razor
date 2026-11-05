"""A voice note: uploaded, transcribed by the configured program before admission, kept with its
transcript as the input's text, led for the model only, admitted once across resends, refused
with a sentence when the program fails, and played back with ranges (docs/design/voice-notes.md)."""
import json
import tempfile
import threading
import time
from pathlib import Path

from driver import REPLIES, Reply, flow
import server

# The stand-in transcriber counts its runs, and acts on what the audio says.
scratch = Path(tempfile.mkdtemp())
runs, program = scratch / "runs", scratch / "transcribe"
program.write_text(f"""audio=$(cat); echo run >> {runs}
case $audio in
  *SLOW*) sleep 2; echo "the slow note" ;;
  *FAIL*) echo "working" >&2; echo "stand-in: the engine refused it." >&2; exit 3 ;;
  *EMPTY*) echo "   " ;;
  *) echo "  hello from a voice note  " ;;
esac
""")
lead = "(Voice note, transcribed by machine; some words may be misheard.)"


def call(f, method, path, body=b"", headers=None):
    answer = server.send(f, method, path, body, headers)
    return answer.status, dict(answer.getheaders()), answer.read()


def transcribing(f):
    return json.loads(server.send(f, "GET", "/status").readline())["server"].get("transcribe", False)


def note(f, session, audio, request, levels=(0, 128, 255)):
    path = json.loads(call(f, "POST", f"/sessions/{session}/files/voice.m4a", audio)[2])["path"]
    voice = {"file": path, "duration_ms": 1500, "levels": list(levels)}
    return voice, {"text": "", "request": request, "voice": voice}


def turns(entries):
    return sum(e["kind"] == "turn_end" for e in entries)


with flow([Reply((REPLIES / "hello.sse").read_bytes()) for _ in range(3)],
          config=f"transcribe = {json.dumps(['/bin/sh', str(program)])}") as f:
    server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    assert transcribing(f) is True

    # 1. The note is admitted with the clean transcript as its text and its voice beside it; the
    # model reads the lead before the words, and the record does not.
    voice, sent = note(f, session, b"audio one", "voice-1")
    entry = server.post(f, f"/sessions/{session}/input", sent)["entry"]
    entries = server.until(f, session, lambda es: turns(es) == 1)
    inputs = [e for e in entries if e["kind"] == "input"]
    assert [(i["id"], i["text"], i["voice"]) for i in inputs] == [
        (entry, "hello from a voice note", voice)], inputs
    asked = json.dumps(json.loads((f.requests / "001.body").read_bytes())["messages"])
    assert json.dumps(f"{lead}\nhello from a voice note")[1:-1] in asked, asked

    # 2. A resend after admission answers the same entry without running the program again.
    assert server.post(f, f"/sessions/{session}/input", sent)["entry"] == entry
    assert runs.read_text().count("run") == 1, runs.read_text()

    # 3. A resend while the note is transcribed is 409 and runs nothing, as is another spelling
    # of its path under another request id; once admitted, it answers the first entry.
    voice, sent = note(f, session, b"SLOW audio", "voice-2")
    first = {}
    thread = threading.Thread(target=lambda: first.update(
        server.post(f, f"/sessions/{session}/input", sent)))
    thread.start()
    while runs.read_text().count("run") < 2:
        time.sleep(0.02)
    assert call(f, "PUT", f"/sessions/{session}/draft", b"edited during transcription")[0] == 200
    status, _, body = call(f, "POST", f"/sessions/{session}/input", sent)
    assert (status, json.loads(body)) == (409, {"error": "This voice note is still being transcribed."}), body
    alias = {**sent, "request": "voice-2b", "voice": {**voice, "file": voice["file"].replace("/", "//", 1)}}
    status, _, body = call(f, "POST", f"/sessions/{session}/input", alias)
    assert status == 409, (status, body)
    thread.join()
    assert call(f, "GET", f"/sessions/{session}/draft")[2] == b"edited during transcription"
    assert server.post(f, f"/sessions/{session}/input", sent)["entry"] == first["entry"]
    assert runs.read_text().count("run") == 2, runs.read_text()
    server.until(f, session, lambda es: turns(es) == 2)

    # 4. A failing program's last line of standard error is the refusal; no words is refused
    # too; neither admits anything. Too many levels is refused before the program runs.
    for audio, sentence in ((b"FAIL", "Couldn't transcribe it: stand-in: the engine refused it."),
                            (b"EMPTY", "Couldn't transcribe it: no words were heard.")):
        status, _, body = call(f, "POST", f"/sessions/{session}/input", note(f, session, audio, None)[1])
        assert (status, json.loads(body)) == (500, {"error": sentence}), (status, body)
    status, _, body = call(f, "POST", f"/sessions/{session}/input",
                           note(f, session, b"audio", "voice-3", range(65))[1])
    assert status == 400 and runs.read_text().count("run") == 4, (status, body)
    assert sum(e["kind"] == "input" for e in f.record(session)) == 2

    # 5. After those four claimed runs, a fresh note still runs, so every claim was let go; typed
    # words before it stay on their own line.
    sent = {**note(f, session, b"audio five", "voice-5")[1], "text": "Please do this:"}
    entry = server.post(f, f"/sessions/{session}/input", sent)["entry"]
    entries = server.until(f, session, lambda es: turns(es) == 3)
    assert next(e for e in entries if e["id"] == entry)["text"] == "Please do this:\nhello from a voice note"

    # 6. The audio plays back through the files route, typed and ranged.
    status, headers, body = call(f, "GET", f"/sessions/{session}/files/{voice['file']}",
                                 headers={"Range": "bytes=0-3"})
    assert (status, headers["content-type"], body) == (206, "audio/mp4", b"SLOW"), (status, headers)

    # 7. With no transcriber the note is refused with a sentence, and the status says so.
    config = f.home / ".razor/config.toml"
    config.write_text("\n".join(l for l in config.read_text().splitlines() if "transcribe" not in l))
    status, _, body = call(f, "POST", f"/sessions/{session}/input", note(f, session, b"audio", None)[1])
    assert (status, json.loads(body)["error"]) == (
        503, "This server has no transcriber; set `transcribe`."), body
    assert transcribing(f) is False
