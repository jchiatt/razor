"""Responses replies as the Codex backend streams them, with its empty completed output."""
from dataclasses import replace
from itertools import accumulate, count
import json

from standin import Reply

PLAIN = "[profiles.codex]\ncode_mode = false\n"
_ids = count()


def response(items, *, missing=(), completed=True, tier=None, gap=0.03):
    """Each item added, then done unless its index is in `missing`; then, if `completed`,
    `response.completed`. An added reasoning item carries other encrypted bytes than its done
    form, so a replay of the added bytes shows. A summary or message part whose text is a list
    of pieces streams a delta per piece after its item is added, pausing `gap` seconds after
    each, and is done joined."""
    events, streamed = [], []
    for index, item in enumerate(items):
        key, kind = ("summary", "reasoning_summary_text") if item["type"] == "reasoning" else ("content", "output_text")
        pieces = [(at, p["text"]) for at, p in enumerate(item.get(key) or []) if isinstance(p.get("text"), list)]
        if pieces:
            item = {**item, key: [{**p, "text": "".join(p["text"])} for p in item[key]]}
        added = {**item, "encrypted_content": "ADDED_MUST_NOT_REPLAY"} if item["type"] == "reasoning" else item
        events.append({"type": "response.output_item.added", "output_index": index, "item": added})
        for at, texts in pieces:
            for piece in texts:
                events.append({"type": f"response.{kind}.delta", "item_id": item["id"], "output_index": index,
                               f"{key}_index": at, "delta": piece})
                streamed.append(len(events))
        if index not in missing:
            events.append({"type": "response.output_item.done", "output_index": index, "item": item})
    events.append({"type": "response.metadata", "metadata": {}})
    if completed:
        events.append({"type": "response.completed", "response": {"status": "completed", "output": [], "service_tier": tier, "usage": {
            "input_tokens": 100, "output_tokens": 10, "input_tokens_details": {"cached_tokens": 60}}}})
    framed = [f"data: {json.dumps(e, ensure_ascii=False)}\n\n".encode() for e in events]
    ends = list(accumulate(map(len, framed)))
    return Reply(b"".join(framed), cuts=tuple(ends[n - 1] for n in streamed), pause=gap)


def thought(*parts, id="rs_flow"):
    """A reasoning item with a summary part for each list of pieces, each piece streamed."""
    return {"type": "reasoning", "id": id, "summary": [{"type": "summary_text", "text": list(p)} for p in parts]}


def message(*pieces, id="msg_flow"):
    """A final answer whose text streams a delta per piece."""
    return {"type": "message", "id": id, "role": "assistant", "phase": "final_answer",
            "content": [{"type": "output_text", "text": list(pieces)}]}


def call(name, arguments, id="call_flow"):
    """A function call item; `arguments` is the raw string, as a model may send it malformed."""
    return {"type": "function_call", "id": f"fc_{id}", "call_id": id, "name": name, "arguments": arguments}


def retried(abandoned, items, gap=0.03):
    """Two replies: `abandoned` streamed and cut before it completes, which Razor tries again,
    then `items` streamed and completed; the void line comes between their thoughts."""
    return [response(abandoned, completed=False, gap=gap), response(items, gap=gap)]


def done():
    return response([{"type": "message", "id": "msg_done", "role": "assistant", "phase": "final_answer",
                      "content": [{"type": "output_text", "text": "flow done"}]}])


def reply(marker, name, /, **arguments):
    """A plain-GPT call with `arguments`, or with none a reply of `name`'s words, selected by
    a phrase in the request."""
    n = next(_ids)
    item = call(name, json.dumps(arguments), f"call_{n}") if arguments else {
        "type": "message", "id": f"msg_{n}", "role": "assistant", "phase": "final_answer",
        "content": [{"type": "output_text", "text": name}]}
    return replace(response([item]), match=marker)
