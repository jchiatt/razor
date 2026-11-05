"""A scripted stand-in for GPT-Live's primary WebSocket (docs/design/call.md). Run as
`python3 flows/live.py --port N [--script events.json] [--capture DIR] [--refuse STATUS]`, it
prints its URL and serves until stopped; flows use `serve`."""

from contextlib import contextmanager, suppress
from pathlib import Path
import argparse
import asyncio
import base64
import json
import threading
import uuid

from websockets.asyncio.server import serve as listen
from websockets.asyncio.client import connect
from websockets.exceptions import ConnectionClosed

# With no script: words once the phone's audio comes, a hand-off, the voice saying the agent's
# reply once it is given, and two seconds of use.
CALL = [
    {"after": "session.input_audio.append", "type": "session.input_transcript.delta",
     "delta": "Please check the work.", "start_ms": 0, "end_ms": 1000},
    {"type": "session.delegation.created", "offset_ms": 1000,
     "delegation": {"id": "delegation-1", "type": "delegation", "target": "client"}},
    {"after": "session.commentary.append", "type": "session.output_transcript.delta",
     "delta": "The agent replied.", "start_ms": 1000, "end_ms": 2000},
    {"type": "session.usage.updated", "usage": {"seconds": 2}},
]


def invalid(command):
    """The parameter GPT-Live refuses in a command, if any: an initial message that is not one typed
    text part, an acknowledgement delay outside 0 to 800 ms, or an update whose delegation is not
    null or an id, or whose content is not a string."""
    if command["type"] == "session.start":
        for n, item in enumerate(command["session"].get("input", [])):
            content, part = item.get("content"), "output_text" if item.get("role") == "assistant" else "input_text"
            if not (isinstance(content, list) and [p.get("type") for p in content] == [part]
                    and isinstance(content[0].get("text"), str)):
                return f"session.input[{n}].content"
        delay = (command["session"].get("delegation") or {}).get("acknowledgement_delay_ms", 0)
        return None if type(delay) is int and 0 <= delay <= 800 else "session.delegation.acknowledgement_delay_ms"
    if command["type"] in ("session.thinking.append", "session.commentary.append"):
        delegation = command.get("delegation_id", 0)
        if not (delegation is None or isinstance(delegation, str) and delegation):
            return "delegation_id"
        return None if isinstance(command.get("content"), str) else "content"


@contextmanager
def serve(script=None, capture=None, *, port=0, refuse=None):
    """Yields the stand-in's URL. `session.start` must come first, and a command GPT-Live refuses
    (`invalid`) gets its error and ends the call, so that a flow sending one fails; then each event
    of `script` is sent in order. An event with `after` first waits for a command of that type it
    has not yet waited for, `delay` waits that many seconds, `audio` names raw 24 kHz PCM sent in
    20 ms frames `pace` seconds apart (0.02, real time, unless given; 0 sends them at once, as a
    stalled network would), `drop` cuts the connection with no close, and anything else goes as
    written. Appends, mute and unmute are acknowledged; `session.close` gets `session.closed` with
    the latest usage. `capture` names a folder collecting the commands in commands.jsonl, each audio
    as its byte count; `refuse` answers the handshake with that HTTP status instead."""
    capture = Path(capture) if capture else None
    if capture:
        capture.mkdir(parents=True, exist_ok=True)

    async def call(ws):
        seen, waited, usage, arrived = {}, {}, [{"seconds": 0}], asyncio.Condition()

        async def play():
            for event in CALL if script is None else script:
                event = dict(event)
                if after := event.pop("after", None):
                    async with arrived:
                        await arrived.wait_for(lambda: seen.get(after, 0) > waited.get(after, 0))
                        waited[after] = waited.get(after, 0) + 1
                await asyncio.sleep(event.pop("delay", 0))
                if event.pop("drop", False):
                    return ws.transport.abort()
                pcm = Path(event.pop("audio")).read_bytes() if "audio" in event else b""
                pace = event.pop("pace", 0.02)
                for at in range(0, len(pcm), 960):
                    delta = base64.b64encode(pcm[at:at + 960]).decode()
                    await ws.send(json.dumps({"type": "session.output_audio.delta", "delta": delta}))
                    await asyncio.sleep(pace)
                usage[0] = event.get("usage", usage[0])
                if event:
                    await ws.send(json.dumps(event))

        player = None
        async for raw in ws:
            command = json.loads(raw)
            kind = command["type"]
            if capture:
                if "audio" in command:
                    command["audio"] = len(base64.b64decode(command["audio"]))
                with (capture / "commands.jsonl").open("a") as commands:
                    commands.write(json.dumps(command) + "\n")
            async with arrived:
                seen[kind] = seen.get(kind, 0) + 1
                arrived.notify_all()
            if param := invalid(command):
                await ws.send(json.dumps({"type": "error", "error": {"type": "invalid_request_error",
                    "code": "invalid_value", "param": param, "message": f"Invalid value for '{param}'."}}))
                return
            if kind == "session.start":
                await ws.send(json.dumps({"type": "session.started", "client_event_id": command.get("event_id"),
                                          "session": {"id": f"live_{uuid.uuid4().hex}"}}))
                player = asyncio.create_task(play())
            elif kind.endswith(("append", "mute")) and kind != "session.input_audio.append":
                await ws.send(json.dumps({"type": kind + ("ed" if kind.endswith("append") else "d"),
                                          "client_event_id": command.get("event_id")}))
            elif kind == "session.close":
                await ws.send(json.dumps({"type": "session.closed", "reason": "close_requested", "usage": usage[0]}))
                break
        if player:
            player.cancel()

    def refusing(connection, request):
        return connection.respond(refuse, "Refused.\n")

    async def start():
        return await listen(call, "127.0.0.1", port, max_size=1 << 20, process_request=refusing if refuse else None)

    async def stop():
        server.close()
        await server.wait_closed()

    loop = asyncio.new_event_loop()
    server = loop.run_until_complete(start())
    thread = threading.Thread(target=loop.run_forever, daemon=True)
    thread.start()
    try:
        yield f"ws://127.0.0.1:{server.sockets[0].getsockname()[1]}"
    finally:
        asyncio.run_coroutine_threadsafe(stop(), loop).result(10)
        loop.call_soon_threadsafe(loop.stop)
        thread.join(10)


async def phone(f, session, act=None, *, streaming=False):
    """The phone's half of a call, through the real API; retain frames through socket close.
    `act` can react to each frame; `streaming` sends PCM until Razor closes the socket."""
    home = f.home / ".razor"
    address = json.loads((home / "server.json").read_text())["address"]
    key = (home / "device.key").read_text().strip()
    seen = []
    async with connect(f"ws://{address}/v1/sessions/{session}/call", proxy=None,
                       additional_headers={"Authorization": "Bearer " + key}) as ws:
        async def stream():
            while True:
                await ws.send(bytes(960))
                await asyncio.sleep(.02)
        sending = asyncio.create_task(stream()) if streaming else None
        try:
            async with asyncio.timeout(60):
                async for raw in ws:
                    event = {"audio": raw} if isinstance(raw, bytes) else json.loads(raw)
                    seen.append(event)
                    if act:
                        await act(ws, event)
        finally:
            if sending:
                sending.cancel()
                with suppress(asyncio.CancelledError, ConnectionClosed):
                    await sending
    return seen


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--script", type=Path)
    parser.add_argument("--capture", type=Path)
    parser.add_argument("--refuse", type=int, help="answer the handshake with this HTTP status")
    flags = parser.parse_args()
    with serve(flags.script and json.loads(flags.script.read_text()), flags.capture, port=flags.port,
               refuse=flags.refuse) as url:
        print(url, flush=True)
        threading.Event().wait()
