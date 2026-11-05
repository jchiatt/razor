"""Claude replies: one streamed Messages API message, framed as the recorded shell-short.sse is."""
from itertools import accumulate, count
import json

from driver import REPLIES, Reply

_messages = count(1)
_events = [json.loads(line[6:]) for line in (REPLIES / "shell-short.sse").read_text().splitlines()
           if line.startswith("data: ")]


def reply(*calls, text=None, thinking=None, match=None, redacted=False, gap=0.03):
    """A `redacted_thinking` block if `redacted`, a `thinking` block as the API returns a
    summarized one, its words in one delta or a list of pieces each its own delta with a pause of
    `gap` seconds after it, one or several `text` blocks, then each (id, name, input) tool_use
    block, in that order; a call whose id is None gets one of its own."""
    first, start, delta, stop, end, last = _events
    n = next(_messages)
    texts = [] if text is None else [text] if isinstance(text, str) else text
    pieces = [] if not thinking else [thinking] if isinstance(thinking, str) else thinking
    blocks = [({"type": "redacted_thinking", "data": "UkVEQUNURUQ="}, [])] if redacted else []
    blocks += [({"type": "thinking", "thinking": "", "signature": ""},
                [{"type": "thinking_delta", "thinking": piece} for piece in pieces])] if pieces else []
    blocks += [({"type": "text", "text": ""}, [{"type": "text_delta", "text": value}]) for value in texts]
    blocks += [(dict(start["content_block"], id=id or f"toolu_flow_{n}_{i}", name=name),
                [dict(delta["delta"], partial_json=json.dumps(input))]) for i, (id, name, input) in enumerate(calls)]
    events = [dict(first, message=dict(first["message"], id=f"msg_flow_{n}"))]
    for index, (block, changes) in enumerate(blocks):
        events += [dict(start, index=index, content_block=block), *(dict(delta, index=index, delta=c) for c in changes),
                   dict(stop, index=index)]
    events += [dict(end, delta=dict(end["delta"], stop_reason="tool_use" if calls else "end_turn")), last]
    framed = [f"event: {e['type']}\ndata: {json.dumps(e)}\n\n".encode() for e in events]
    cuts = tuple(at for at, e in zip(accumulate(map(len, framed)), events)
                 if e.get("delta", {}).get("type") == "thinking_delta")
    return Reply(b"".join(framed), cuts=cuts if len(pieces) > 1 else (), pause=gap, match=match)
