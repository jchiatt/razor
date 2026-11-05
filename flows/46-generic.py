"""The generic path: the chat wire, the four tools, and each host's way of carrying reasoning."""
import json
from pathlib import Path

from driver import flow
from server import send
from standin import Reply

DEEPSEEK, GLM, GEMINI = "deepseek-v4.1-flash-1017", "glm-5.3-flash", "gemini-3.8-flash"
HOSTS = {DEEPSEEK: "deepseek-v4-1-flash-1017", GLM: "glm-5-3-flash", GEMINI: GEMINI}
# DeepSeek names the empty variant, which is plain and replaces its shipped leader whole; GLM
# and Gemini name none, so each gets its leader (docs/design/generic.md).
CONFIG = f'[models."{DEEPSEEK}"]\nbase_url = "{{standin}}"\nvariant = "plain.json"\n'
CONFIG += f'[models."{GLM}"]\nbase_url = "{{standin}}"\nreplay = "reasoning"\n'
CONFIG += f'[models."{GEMINI}"]\nbase_url = "{{standin}}"\n'
BUSY = b'{"error": {"message": "reached its limit of 60 concurrent requests"}}'
USAGE = {"prompt_tokens": 100, "completion_tokens": 10, "total_tokens": 130,
         "prompt_tokens_details": {"cached_tokens": 60}}


def reply(model, *deltas, finish="tool_calls", done=True, cut=None, match=None):
    """A chat stream for `model`'s requests: each delta, then the finish, the usage, `[DONE]`.
    `cut` pauses the send inside its first occurrence, as a chunk boundary in a character."""
    chunks = [{"choices": [{"index": 0, "delta": {"role": "assistant", **d}}]} for d in deltas]
    if finish:
        chunks.append({"choices": [{"index": 0, "delta": {"role": None}, "finish_reason": finish}]})
    chunks.append({"choices": [], "usage": USAGE})
    body = "".join(f"data: {json.dumps(c, ensure_ascii=False)}\n\n" for c in chunks)
    body = (body + ("data: [DONE]\n\n" if done else "")).encode()
    cuts = (body.index(cut.encode()) + 1,) if cut else ()
    return Reply(body, cuts=cuts, match=f'"model":"{HOSTS[model]}"' if match is None else match)


def call(index, id, name, arguments, **extra):
    return {"tool_calls": [{"index": index, "id": id, "type": "function",
                            "function": {"name": name, "arguments": arguments}, **extra}]}


replies = [
    reply(DEEPSEEK, {"content": ""}, {"reasoning": "Plan: write notes, then "}, {"reasoning": "✓"},
          call(0, "call_w", "write", ""), {"tool_calls": [{"index": 0, "function": {"arguments":
          '{"path": "notes.txt", '}}]}, {"tool_calls": [{"index": 0, "function": {"arguments":
          '"content": "alpha\\n"}'}}]}, cut="✓"),
    reply(DEEPSEEK, {"reasoning": "Edit, check, and one broken call."},
          call(0, "call_e", "edit", '{"path": "notes.txt", "old_string": "alpha", "new_string": "beta"}'),
          call(1, "call_b", "bash", '{"command": "cat notes.txt"}'),
          call(2, "call_m", "bash", '{"command": "touch must-not-exist"')),
    reply(DEEPSEEK, {"reasoning": "All checked."}, {"content": "first done"}, finish="stop"),
    reply(DEEPSEEK, {"reasoning": "Read it back."}, call(0, "call_r", "read", '{"path": "notes.txt"}')),
    reply(DEEPSEEK, {"content": "second done"}, finish="stop"),
    # Early ends: no finish_reason, then no [DONE]; neither call may run.
    reply(DEEPSEEK, call(0, "call_x", "bash", '{"command": "touch early-1"}'), finish=None),
    reply(DEEPSEEK, call(0, "call_y", "bash", '{"command": "touch early-2"}'), done=False),
    reply(DEEPSEEK, {"content": "early done"}, finish="stop"),
    reply(GLM, {"reasoning_content": "Look first."}, call(0, "call_l", "bash", '{"command": "ls"}')),
    reply(GLM, {"content": "glm done"}, finish="stop"),
    reply(GEMINI, {"tool_calls": [
        {"id": "fc-a", "type": "function", "function": {"name": "read", "arguments": '{"path": "a.txt"}'},
         "extra_content": {"google": {"thought_signature": "sig-A"}}},
        {"id": "fc-b", "type": "function", "function": {"name": "read", "arguments": '{"path": "b.txt"}'}}]}),
    reply(GEMINI, {"content": "gemini one"}, {"content": "", "extra_content": {"google":
          {"thought_signature": "sig-T"}}}, finish="stop"),
    reply(GEMINI, {"content": "gemini two"}, finish="stop"),
    # A host echoing the bearer, in a reply and in an error: its text stays, the key goes.
    reply(DEEPSEEK, {"content": "echo flow-runinfra-key"}, finish="stop"),
    Reply(b'{"error": {"message": "Bad key Bearer flow-runinfra-key"}}', status=400,
          match=f'"model":"{HOSTS[DEEPSEEK]}"'),
    # A host at capacity, then one asking for longer than 60 s; ChatGPT's 429 is a plan limit.
    Reply(BUSY, status=429, headers=(("Retry-After", "1"),), match=f'"model":"{HOSTS[DEEPSEEK]}"'),
    reply(DEEPSEEK, {"content": "busy done"}, finish="stop"),
    Reply(BUSY, status=429, headers=(("Retry-After", "120"),), match=f'"model":"{HOSTS[DEEPSEEK]}"'),
    Reply(BUSY, status=429, headers=(("Retry-After", "1"),), match='"model":"gpt-6.1-sol"'),
]
keys = {"RUNINFRA_API_KEY": "flow-runinfra-key", "GEMINI_API_KEY": "flow-gemini-key"}
with flow(replies, model=DEEPSEEK, config=f'effort = "max"\n{CONFIG}', env=keys) as f:
    (f.home / ".razor/plain.json").write_text("{}")
    (f.cwd / "a.txt").write_text("alpha file\n")
    (f.cwd / "b.txt").write_text("beta file\n")

    def requests(model):
        """Each request to `model` so far: its body and its spooled metadata, in order."""
        found = sorted(f.requests.glob("[0-9][0-9][0-9].body"))
        pairs = [(json.loads(p.read_bytes()), json.loads(p.with_suffix(".json").read_text())) for p in found]
        return [(b, m) for b, m in pairs if b["model"] == HOSTS[model]]

    def turn(*argv):
        result = f.run(*argv, "--print=json", timeout=90)
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)

    first = turn("Write notes.txt, edit it, check it.")
    assert first["message"] == "first done", first
    assert turn("--resume", first["session"], "Read it back.")["message"] == "second done"
    assert (f.cwd / "notes.txt").read_text() == "beta\n" and not (f.cwd / "must-not-exist").exists()
    sent = requests(DEEPSEEK)
    assert len(sent) == 5 and all(m["path"] == "/chat/completions" for _, m in sent), sent
    assert all(b["reasoning_effort"] == "max" and b["stream_options"] == {"include_usage": True} for b, _ in sent)
    assert all(m["headers"]["authorization"] == "Bearer flow-runinfra-key" for _, m in sent)
    assert all(m["headers"]["x-session-id"] == first["session"] for _, m in sent)
    assert sent[0][0]["messages"][0]["role"] == "system", sent[0][0]["messages"][0]
    tools = [t["function"]["name"] for t in sent[0][0]["tools"]]
    assert {"read", "write", "edit", "bash"} <= set(tools) and "web_search" not in tools, tools
    assert "-generic-plain-" in next(f.record(first["session"]))["profile_version"]
    back = sent[1][0]["messages"]
    assert back[-2] == {"role": "assistant", "content": "", "reasoning": "Plan: write notes, then ✓",
                        "tool_calls": [{"id": "call_w", "type": "function", "function": {"name": "write",
                        "arguments": '{"path": "notes.txt", "content": "alpha\\n"}'}}]}, back[-2]
    assert back[-1]["role"] == "tool" and back[-1]["tool_call_id"] == "call_w" and back[-1]["name"] == "write"
    back = sent[2][0]["messages"]
    assert [m["tool_call_id"] for m in back[-3:]] == ["call_e", "call_b", "call_m"], back[-3:]
    assert back[-4]["tool_calls"][2]["function"]["arguments"] == "{}", back[-4]
    assert "beta" in back[-2]["content"], back[-2]
    texts = [m for m in sent[3][0]["messages"] if m.get("content") == "first done"]
    assert texts and texts[0]["reasoning"] == "All checked.", sent[3][0]["messages"]
    entries = list(f.record(first["session"]))
    results = {e["call"]: e for e in entries if e["kind"] == "result"}
    assert results["call_m"]["outcome"] == "error" and results["call_b"]["outcome"] == "success", results
    assert [e["text"] for e in entries if e["kind"] == "thinking"][0] == "Plan: write notes, then ✓"

    early = turn("Try the early ends.")
    assert early["message"] == "early done", early
    assert not (f.cwd / "early-1").exists() and not (f.cwd / "early-2").exists()
    assert not [e for e in f.record(early["session"]) if e["kind"] == "call"]

    glm = turn("--model", GLM, "--effort", "high", "Look around.")
    assert glm["message"] == "glm done", glm
    sent = requests(GLM)
    assert all(b["reasoning_effort"] == "high" for b, _ in sent)
    tools = {t["function"]["name"]: t["function"]["description"] for t in sent[0][0]["tools"]}
    assert "bash" in tools and not {"read", "write", "edit"} & set(tools), tools
    assert "Read, search, create and edit files with it too." in sent[0][0]["messages"][0]["content"]
    back = sent[1][0]["messages"][-2]
    assert back["reasoning"] == "Look first." and "reasoning_content" not in back, back
    start = next(f.record(glm["session"]))
    assert "-generic-bash-coord-1-" in start["profile_version"], start

    gemini = turn("--model", GEMINI, "--speed", "fast", "Read both files.")
    assert gemini["message"] == "gemini one", gemini
    assert turn("--resume", gemini["session"], "Again.")["message"] == "gemini two"
    sent = requests(GEMINI)
    assert all(b["service_tier"] == "priority" for b, _ in sent), sent
    assert all(b["reasoning_effort"] == "high" for b, _ in sent)
    assert "# Working economically" in sent[0][0]["messages"][0]["content"]
    assert "-generic-lean-1-" in next(f.record(gemini["session"]))["profile_version"]
    assert all(m["headers"]["authorization"] == "Bearer flow-gemini-key" for _, m in sent)
    calls = sent[1][0]["messages"][-3]["tool_calls"]
    assert calls[0]["extra_content"] == {"google": {"thought_signature": "sig-A"}}, calls
    assert "extra_content" not in calls[1] and not any("index" in c for c in calls), calls
    tool = sent[1][0]["messages"][-2:]
    assert [(m["tool_call_id"], m["name"]) for m in tool] == [("fc-a", "read"), ("fc-b", "read")], tool
    assert "alpha file" in tool[0]["content"] and "beta file" in tool[1]["content"], tool
    texts = [m for m in sent[2][0]["messages"] if m.get("content") == "gemini one"]
    assert texts and texts[0]["extra_content"] == {"google": {"thought_signature": "sig-T"}}, texts

    echo = turn("Echo the key.")
    assert echo["message"] == "echo [key removed]", echo
    refused = f.run("--resume", echo["session"], "Again.", "--print=json", timeout=90)
    assert "HTTP 400: Bad key Bearer [key removed]" in refused.stdout, refused
    for path in Path(f.home).rglob("*"):
        if path.is_file():
            data = path.read_bytes()
            assert b"flow-runinfra-key" not in data and b"flow-gemini-key" not in data, path

    # The chat route sends the same request again after the host's Retry-After, and the
    # response's usage says what was waited; a wait past 60 s is not taken, so the turn ends at
    # once. ChatGPT's 429 ends its turn at once (docs/design/generic.md).
    busy = turn("Wait for a slot.")
    assert busy["message"] == "busy done", busy
    entries = list(f.record(busy["session"]))
    waits = [e["usage"]["waited"] for e in entries if e["kind"] in ("response", "turn_end")]
    assert waits == [1000, 1000], waits
    for argv in (["--resume", busy["session"]], ["--model", "gpt-6.1-sol"]):
        refused = f.run(*argv, "Again.", "--print=json", timeout=20)
        assert "HTTP 429: reached its limit of 60 concurrent requests" in refused.stdout, refused

# A new provider uses plain tools, its own id/key/request settings, and the configured window
# asks for a handover before the host runs out of context. A host without auth needs no key.
LOCAL, OPEN = "local-provider-46", "unauthenticated-46"
HOSTS.update({LOCAL: "accounts/fireworks/routers/kimi-k3-fast", OPEN: OPEN})
config = (f'[rebuild]\nthreshold = 0.5\n[models."{LOCAL}"]\nbase_url = "{{standin}}"\n'
          f'id = "maker/custom-model"\nspeed_ids.fast = "{HOSTS[LOCAL]}"\nkey = "LOCAL_KEY"\nwindow = 200\n'
          f'[models."{LOCAL}".request]\nmax_completion_tokens = 64\n'
          f'[models."{OPEN}"]\nbase_url = "{{standin}}"\n'
          f'[models."{GEMINI}"]\nwindow = 4096\n')
old = "OVERSIZED_OLD_RESPONSE_46 " * 1024
with flow([reply(LOCAL, call(0, "local_write", "write", '{"path":"local.txt","content":"kept"}'), match=""),
           reply(LOCAL, call(0, "local_note", "handover", '{"note":"Keep the local request."}'), match=""),
           reply(LOCAL, {"content": "local done"}, finish="stop", match=""),
           reply(OPEN, call(0, "open_note", "handover", '{"note":"Keep the unauthenticated request."}')),
           reply(OPEN, {"content": old}, finish="stop"),
           Reply(b'{"error":{"code":"context_length_exceeded","message":"Maximum context length reached"}}', status=400,
                 match=f'"model":"{OPEN}"'),
           reply(OPEN, {"content": "recovered done"}, finish="stop"),
           reply(OPEN, {"content": "fresh done"}, finish="stop")],
          model=LOCAL, config=config, env={"LOCAL_KEY": "flow-local-key"}) as f:
    result = f.run("--print=json", "--speed", "fast", "Keep this exact local request.")
    assert result.returncode == 0, (result.stdout, result.stderr)
    answer = json.loads(result.stdout)
    assert answer["message"] == "local done" and (f.cwd / "local.txt").read_text() == "kept", answer
    entries = list(f.record(answer["session"]))
    rebuilt = [e for e in entries if e["kind"] == "rebuild"]
    assert len(rebuilt) == 1 and rebuilt[0]["trigger"] == "threshold", rebuilt
    assert "Keep this exact local request." in rebuilt[0]["context"], rebuilt
    first = json.loads((f.requests / "001.body").read_bytes())
    assert first["model"] == HOSTS[LOCAL] and first["max_completion_tokens"] == 64, first
    assert {"read", "write", "edit", "bash"} <= {t["function"]["name"] for t in first["tools"]}, first
    assert "-generic-plain-" in entries[0]["profile_version"], entries[0]
    meta = json.loads((f.requests / "001.json").read_text())
    assert meta["path"] == "/chat/completions" and meta["headers"]["authorization"] == "Bearer flow-local-key", meta
    models = json.loads(send(f, "GET", "/models").read())
    assert {LOCAL, OPEN} <= set(models["models"]) and models["windows"][LOCAL] == 200 and models["windows"][GEMINI] == 4096, models
    result = f.run("--model", OPEN, "--print=json", "Reply without a key.")
    opened = json.loads(result.stdout)
    assert result.returncode == 0 and opened["message"] == old, result
    meta = json.loads((f.requests / "004.json").read_text())
    assert "authorization" not in meta["headers"], meta
    result = f.run("--resume", opened["session"], "--print=json", "This old session is full.")
    recovered = json.loads(result.stdout)
    assert result.returncode == 0 and recovered["message"] == "recovered done" and recovered["session"] == opened["session"], result
    entries = list(f.record(opened["session"]))
    rebuilds = [e for e in entries if e["kind"] == "rebuild"]
    assert [e["trigger"] for e in rebuilds] == ["handover", "overflow"], rebuilds
    assert rebuilds[-1]["note"] == "Keep the unauthenticated request." and "This old session is full." in rebuilds[-1]["context"], rebuilds
    failed = [e for e in entries if e["kind"] == "turn_end" and isinstance(e["end"], dict)]
    assert len(failed) == 1 and not failed[0]["inputs"] and "Maximum context length reached" in failed[0]["end"]["error"], failed
    assert old in (f.requests / "006.body").read_text() and old not in (f.requests / "007.body").read_text()
    result = f.run("--resume", opened["session"], "--print=json", "Continue in the recovered session.")
    fresh = json.loads(result.stdout)
    assert result.returncode == 0 and fresh["message"] == "fresh done" and fresh["session"] == opened["session"], result
    sent = (f.requests / "008.body").read_text()
    assert "Continue in the recovered session." in sent and "This old session is full." in sent and old not in sent, sent
    result = f.run("--model", "sonet", "--print", "Typo.")
    assert result.returncode != 0 and "check the name" in result.stderr and '[models."sonet"]' in result.stderr, result

# A second rejection pauses, even when the local host has no advertised window.
refusal = Reply(b'{"error":{"code":"context_length_exceeded","message":"Maximum context length reached"}}', status=400)
with flow([refusal, refusal], model=OPEN, config=f'[models."{OPEN}"]\nbase_url = "{{standin}}"\n') as f:
    result = f.run("--print=json", "This context cannot fit even after rebuilding.")
    answer = json.loads(result.stdout)
    assert result.returncode != 0, answer
    entries = list(f.record(answer["session"]))
    rebuilds = [e for e in entries if e["kind"] == "rebuild"]
    assert len(rebuilds) == 1 and rebuilds[0]["trigger"] == "overflow", rebuilds
    assert str(len(rebuilds[0]["context"].encode())) + " bytes" in answer["error"], answer
    assert len(list(f.requests.glob("[0-9]*.body"))) == 2, "overflow retried in a loop"
