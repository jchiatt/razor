"""GPT code mode preserves completed native items, renders the pinned declarations, and runs a
cell's web.run as Codex's search request on the sign-in, view_image as `read` showing the image,
and image_gen.imagegen as Codex's image request (docs/design/tools.md, responses.md)."""
import base64
import json
import os
import signal
import server

from driver import Reply, flow
from mcp import PNG
from responses import done, response
from terminal import Terminal

source = 'text("flow exec 🪶"); text(await tools.web__run({search_query: [{q: "flow kingfisher"}]}))'
items = [
    {"type": "reasoning", "id": "rs_flow", "summary": [{"type": "summary_text", "text": "SUMMARY_A"}], "encrypted_content": "DONE_REPLAY_EXACTLY"},
    {"type": "custom_tool_call", "id": "ct_flow", "call_id": "call_exec", "name": "exec", "input": source},
]
search = Reply(json.dumps({"output": "flow search found", "encrypted_output": "SEARCH_NOT_SENT"}).encode())
look = ('image(await tools.view_image({path: "shot.png"}));'
        'text(await tools.view_image({path: "big.png"}).catch(String));'
        'text(await tools.image_gen__imagegen({prompt: "a fox", referenced_image_paths: "shot.png"}).catch(String));'
        'generatedImage(await tools.image_gen__imagegen({prompt: "a fox"}));')
edit = 'generatedImage(await tools.image_gen__imagegen({prompt: "add a hat", num_last_images_to_include: 1}))'
cell = lambda id, source: response([{"type": "custom_tool_call", "id": "ct_" + id, "call_id": id, "name": "exec", "input": source}])
SHOT = base64.b64decode(PNG)
MADE = SHOT + b"made"
made = lambda data: Reply(json.dumps({"created": 1, "data": [{"b64_json": base64.b64encode(data).decode()}]}).encode(),
                          match='"gpt-image-2"')
url = lambda data: "data:image/png;base64," + base64.b64encode(data).decode()
summary_events = [{"type": "response.reasoning_summary_text." + kind, "item_id": "rs_flow", "output_index": 0,
                   "summary_index": 0, field: "SUMMARY_A"} for kind, field in (("delta", "delta"), ("done", "text"))]
streamed = b"".join(f"data: {json.dumps(e)}\n\n".encode() for e in summary_events)
first_reply = response(items, tier="default")
first_reply = Reply(first_reply.body.replace(b'data: {"type": "response.output_item.done"', streamed + b'data: {"type": "response.output_item.done"', 1))
replies = [first_reply, search, cell("call_look", look), made(MADE), cell("call_edit", edit), made(SHOT), done(), done(), done()]
with flow(replies, model="gpt-6.1-sol", config='startup = ["razor", "project"]\n[profiles.codex]\nimage_gen = true') as f:
    (f.cwd / "shot.png").write_bytes(SHOT)
    (f.cwd / "big.png").write_bytes(SHOT[:8] + bytes(7_500_000))
    result = f.run("--print=json", "--speed", "fast", "Run the cell and continue.")
    assert result.returncode == 0, result.stderr
    answer = json.loads(result.stdout)
    assert answer["message"] == "flow done", answer
    entries = list(f.record(answer["session"]))
    assert [e["text"] for e in entries if e["kind"] == "thinking"] == ["SUMMARY_A"], "summary missing or duplicated"
    assert [e["text"] for e in entries if e["kind"] == "notice"] == ["Fast unavailable; provider used Standard for this request."]
    responses = [e for e in entries if e["kind"] == "response"]
    assert len(responses) == 4, responses
    assert responses[0]["native"]["responses"]["items"] == items, responses[0]
    with Terminal(f, "--resume", answer["session"], rows=80) as t:
        t.shown("SUMMARY_A")
    calls = [e for e in entries if e["kind"] == "call" and e["call"].startswith("call_exec")]
    assert [c["call"] for c in calls] == ["call_exec", "call_exec:1"] and calls[0]["input"] == source, calls
    outputs = [e for e in entries if e["kind"] == "result" and e["call"].startswith("call_exec")]
    assert [o["call"] for o in outputs] == ["call_exec:1", "call_exec"], outputs
    assert outputs[0]["output"] == "flow search found", outputs
    assert "Script completed" in outputs[1]["output"] and "flow exec 🪶" in outputs[1]["output"], outputs
    assert "flow search found" in outputs[1]["output"], outputs
    meta = json.loads((f.requests / "002.json").read_text())
    asked = json.loads((f.requests / "002.body").read_bytes())
    assert meta["path"] == "/alpha/search" and meta["headers"]["chatgpt-account-id"] == "flow-account", meta
    assert asked["commands"] == {"search_query": [{"q": "flow kingfisher"}]} and asked["model"] == "gpt-6.1-sol", asked
    assert asked["settings"] == {"allowed_callers": ["direct"], "external_web_access": False}, asked
    assert asked["id"] == answer["session"] and "Run the cell and continue." in json.dumps(asked["input"]), asked
    assert responses[0]["usage"]["input"] == 40 and responses[0]["usage"]["cache_read"] == 60, responses[0]
    first = json.loads((f.requests / "001.body").read_bytes())
    second = json.loads((f.requests / "003.body").read_bytes())
    assert first["service_tier"] == "priority", first
    assert first["store"] is False and first["parallel_tool_calls"] is False, first
    assert "reasoning.encrypted_content" in first["include"], first
    assert first["reasoning"]["context"] == "all_turns", first
    assert first["reasoning"].get("summary") == "auto", first["reasoning"]
    prefix = first["input"][:-1]
    # Four characters per token errs high against the last real-backend measurement.
    prefix_text = json.dumps(prefix, ensure_ascii=False) + first.get("instructions", "")
    estimate = (len(prefix_text) + 3) // 4
    # 12,037 with the notify wording (2026-11-03): it took right reach-the-person behaviour from
    # 30/42 (opus low) and 20/42 (terra medium) to 41/42 for each, so the extra tokens stay.
    assert estimate < 12_060, estimate
    offered = next(i for i in prefix if i["type"] == "additional_tools")["tools"]
    tools = next(t for t in offered if t["name"] == "functions")["tools"]
    assert {t["name"] for t in tools} == {"exec", "wait"}, tools
    description = next(t for t in tools if t["name"] == "exec")["description"]
    assert "### `handover`" in description, tools
    assert "## web\nTools in the web namespace.\n\n### `web__run`\nTool for accessing the internet." in description, description
    assert second["input"][:len(first["input"])] == first["input"], second
    assert second["input"][len(first["input"]):len(first["input"]) + 2] == items, second
    assert "ADDED_MUST_NOT_REPLAY" not in json.dumps(second), second
    assert any(i["type"] == "custom_tool_call_output" and i["call_id"] == "call_exec" for i in second["input"]), second

    # The image file reaches the model after the cell's output, each image after its path; one
    # over 7.5 MB is refused, and so is an argument of the wrong type, before any request.
    results = {e["call"]: e for e in entries if e["kind"] == "result"}
    assert results["call_look:2"]["outcome"] == "error" and "over 7.5 MB" in results["call_look:2"]["output"], results
    assert results["call_look:3"]["outcome"] == "error" and "does not fit" in results["call_look:3"]["output"], results
    after = json.loads((f.requests / "005.body").read_bytes())["input"]
    output = next(i for i in after if i["type"] == "custom_tool_call_output" and i["call_id"] == "call_look")
    shown = after[-1]["content"]
    assert after.index(output) < len(after) - 1 and [c["type"] for c in shown] == ["input_text", "input_image"] * 2, shown
    assert [c["image_url"] for c in shown[1::2]] == [url(SHOT), url(MADE)], "the model did not see both images"
    # The generation is Codex's request on the sign-in; the PNG is saved under the call's entry,
    # and the record keeps its path, which generatedImage() writes for the model.
    entry = next(e["id"] for e in entries if e["kind"] == "call" and e["call"] == "call_look:4")
    meta, body = json.loads((f.requests / "004.json").read_text()), json.loads((f.requests / "004.body").read_bytes())
    assert meta["path"] == "/images/generations", meta
    assert body == {"prompt": "a fox", "background": "opaque", "model": "gpt-image-2", "quality": "auto", "size": "auto"}, body
    headers = meta["headers"]
    assert headers.get("chatgpt-account-id") == "flow-account" and headers.get("originator") == "codex_cli_rs", headers
    assert headers.get("x-codex-image-turn-id") == f"{answer['session']}:{entry}", headers
    saved = f.home / ".razor/sessions" / answer["session"] / f"images/gen-{entry}.png"
    assert json.loads(results["call_look:4"]["output"])["path"] == str(saved) and saved.read_bytes() == MADE, results
    assert str(saved) in json.dumps(output), output
    kept = json.dumps([e for e in entries if e["kind"] in ("call", "result")])
    assert base64.b64encode(MADE).decode() not in kept, "the record holds the generated image's bytes"
    # An edit of the latest image sends the image the model last saw.
    meta, body = json.loads((f.requests / "006.json").read_text()), json.loads((f.requests / "006.body").read_bytes())
    assert meta["path"] == "/images/edits" and body["images"] == [{"image_url": url(MADE)}], (meta, body)

    # Turn-end video cleanup must preserve native items, in memory and after a clean restart.
    expected = json.loads((f.requests / "007.body").read_bytes())["input"] + responses[-1]["native"]["responses"]["items"]
    for index in (8, 9):
        if index == 9:
            pid = server.start(f)
            os.kill(pid, signal.SIGTERM)
            server.gone(pid)
        resumed = f.run("--print=json", "--resume", answer["session"], "Continue the conversation.")
        assert resumed.returncode == 0, resumed.stderr
        replayed = json.loads((f.requests / f"{index:03}.body").read_bytes())["input"]
        assert replayed[:len(expected)] == expected, "native items changed across turns"
    assert not any(e["kind"] == "rebuild" for e in f.record(answer["session"])), "native resume rebuilt instead"

# An API-key route needs no code-mode setting. The loopback stand-in is the explicit HTTP
# proxy, so the real public hostname selects key auth while every request stays local.
with flow([done()], model="gpt-6.1-sol") as f:
    config = f.home / ".razor/config.toml"
    original = config.read_text()
    proxy = json.loads(original.split("base_url = ", 1)[1].strip())
    f.env.update({"HTTP_PROXY": proxy, "NO_PROXY": ""})
    config.write_text(original.replace(proxy, "http://api.openai.com/v1"))
    (f.home / ".razor/auth.json").unlink()
    (f.home / ".razor/key").write_text("flow-public-key")
    result = f.run("--print=json", "Reply on the API key.")
    assert (f.requests / "001.json").exists(), (result.stdout, result.stderr)
    meta = json.loads((f.requests / "001.json").read_text())
    assert "service_tier" not in json.loads((f.requests / "001.body").read_bytes())
    assert meta["path"] == "http://api.openai.com/v1/responses", meta
    assert meta["headers"]["authorization"] == "Bearer flow-public-key", meta
    assert "chatgpt-account-id" not in meta["headers"], meta
    assert result.returncode == 0 and json.loads(result.stdout)["message"] == "flow done", result
    body = json.loads((f.requests / "001.body").read_bytes())
    assert body["reasoning"].get("summary") == "auto", body["reasoning"]
    names = {t["name"] for t in body["tools"] if t["type"] == "function"}
    assert {"read", "write", "edit", "bash"} <= names and "exec" not in names, names
    config.write_text(config.read_text() + "\n[profiles.codex]\ncode_mode = true\n")
    refused = f.run("--print", "Explicit code mode.")
    assert refused.returncode != 0 and "API-key and chat routes support plain tools only" in refused.stderr, refused
    assert len(list(f.requests.glob("[0-9]*.body"))) == 1, "refused code mode contacted the host"

# Common model limits do not make native routes accept chat connection fields. Empty request
# objects count as an override too, and alias collisions and invalid fractions refuse locally.
with flow([]) as f:
    config = f.home / ".razor/config.toml"
    original = config.read_text()
    for name in ("opus", "sol"):
        for field in ('base_url = "http://127.0.0.1:1"', 'key = "DUMMY_KEY"', 'id = "other"',
                      'request = {}', 'variant = {}', 'replay = "streamed"'):
            config.write_text(original + f'\n[models.{name}]\n{field}\n')
            refused = f.run("serve")
            assert refused.returncode != 0 and "remove base_url, key, id, request, variant, replay and speed_ids" in refused.stderr, refused
    for field in ("window = 0", "threshold = 0", "threshold = 1.1", "threshold = nan", "threshold = inf"):
        config.write_text(original + f'\n[models.opus]\n{field}\n')
        refused = f.run("serve")
        assert refused.returncode != 0 and "positive window" in refused.stderr, refused
    config.write_text(original + '\n[models.sol]\nthreshold = 0.5\n[models."gpt-6.1-sol"]\nthreshold = 0.6\n')
    refused = f.run("serve")
    assert refused.returncode != 0 and "same model" in refused.stderr, refused
