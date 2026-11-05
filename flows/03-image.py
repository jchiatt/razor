"""A session's first message carries an image: it is stored in the session, the record names it,
and the model receives its exact bytes before the text (docs/design/claude.md)."""
import base64
import json
import struct
import zlib

from driver import REPLIES, Reply, flow
import server


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


# A real 2-by-2 RGB PNG; the server tells its type from these bytes, not from a label.
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 2, 8, 2, 0, 0, 0)) + chunk(
    b"IDAT", zlib.compress(b"\0\xff\0\0\0\xff\0" * 2)) + chunk(b"IEND", b"")
data = base64.b64encode(png).decode()
prompt = "What colours are in this picture?"
with flow([Reply((REPLIES / "hello.sse").read_bytes())]) as f:
    server.start(f)
    session = server.post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    ask = server.post(f, f"/sessions/{session}/input", {"text": prompt, "images": [data]})["entry"]
    entries = server.until(f, session, lambda es: es[-1]["kind"] == "turn_end")
    assert entries[-1]["inputs"] == [ask] and entries[-1]["end"] == "done", entries[-1]
    inputs = [e for e in entries if e["kind"] == "input"]
    assert len(inputs) == 1 and inputs[0]["id"] == ask and inputs[0]["text"] == prompt, inputs
    images = inputs[0]["images"]
    assert len(images) == 1 and images[0]["media_type"] == "image/png", images
    stored = f.home / ".razor/sessions" / session / images[0]["path"]
    assert not images[0]["path"].startswith("/") and stored.read_bytes() == png, images
    assert [e["text"] for e in entries if e["kind"] == "text"] == ["flow reply"], entries
    request = json.loads((f.requests / "001.body").read_bytes())
    asked = next(m for m in request["messages"] if m["role"] == "user")["content"]
    kinds = [block["type"] for block in asked]
    assert kinds.count("image") == 1 and kinds.index("image") < kinds.index("text"), asked
    picture = asked[kinds.index("image")]["source"]
    assert picture == {"type": "base64", "media_type": "image/png", "data": data}, picture
    assert any(prompt in block.get("text", "") for block in asked), asked
