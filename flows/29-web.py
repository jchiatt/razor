"""Web failures advance per verb; declarations follow available actions at admission.

Fetch successes are captures from live probes (2026-10-28), their identifiers zeroed. Other
provider bodies are synthetic vendor API examples, not captures of failures:
https://docs.exa.ai/reference/get-contents
https://exa.ai/docs/reference/answer
https://docs.firecrawl.dev/api-reference/endpoint/search
https://docs.firecrawl.dev/api-reference/endpoint/scrape
https://docs.parallel.ai/api-reference/search/search
https://docs.parallel.ai/api-reference/extract
https://docs.parallel.ai/api-reference/responses-api/create-response
"""
import json
from pathlib import Path
from tempfile import TemporaryDirectory

from driver import REPLIES, Reply, flow
from model import reply
from standin import serve
from responses import PLAIN, done

URL = "https://example.com/"
KEYS = {"EXA_API_KEY": "flow-exa-secret", "FIRECRAWL_API_KEY": "flow-firecrawl-secret",
        "PARALLEL_API_KEY": "flow-parallel-secret"}
PROVIDERS = ("exa", "firecrawl", "parallel")
PATHS = {"exa": {"search": "/search", "fetch": "/contents", "answer": "/answer"},
         "firecrawl": {"search": "/v2/search", "fetch": "/v2/scrape"},
         "parallel": {"search": "/v1/search", "fetch": "/v1/extract", "answer": "/v1/responses"}}


def success(provider, verb):
    if verb == "fetch":
        return json.loads((REPLIES / f"web-{provider}-fetch.json").read_bytes())
    item = {"title": "Example Domain", "url": URL}
    text = "web evidence " + " ".join(KEYS.values())
    if verb == "search":
        text += " excerpt" * 200
        field = {"description": text} if provider == "firecrawl" else ({"text": text} if provider == "exa" else {"excerpts": [text]})
        items = [{**item, "url": URL + str(i), **field} for i in range(10)]
        return {"success": True, "data": {"web": items}} if provider == "firecrawl" else {"results": items}
    if provider == "exa":
        return {"answer": text, "citations": [item]}
    return {"status": "completed", "output": [{"type": "message", "content": [
        {"type": "output_text", "text": text, "annotations": [{**item, "type": "url_citation"}]}]}]}


def failure(provider, verb):
    # Keep nonempty success text: fallback must bind to failure fields, not emptiness.
    data = success(provider, verb)
    if provider == "exa":
        if verb == "fetch":
            data["statuses"] = [{"status": "error", "error": {"tag": "CRAWL_NOT_FOUND", "httpStatusCode": 403}}]
        else:
            data["error"] = "flow-exa-secret"
    elif provider == "firecrawl":
        if verb == "fetch":
            data["data"]["metadata"]["statusCode"] = 403
        else:
            data["success"] = False
    elif verb == "answer":
        data["status"] = "failed"
    else:
        data["errors"] = [{"error_type": "fetch_error", "http_status_code": 403}]
    return data


def config(url, verb, providers):
    text = f"[web]\n{verb} = {json.dumps(providers)}\n"
    return text + "".join(f"[web.{p}]\nbase_url = {json.dumps(url)}\n" for p in PROVIDERS)


def invoke(f, verb, session=None):
    argv = ["--print=json"] + (["--resume", session] if session else [])
    result = f.run(*argv, "Use the web verb.")
    assert result.returncode == 0, result.stderr
    answer = json.loads(result.stdout)
    assert answer["message"] == "flow done", answer
    entries = list(f.record(answer["session"]))
    results = [e for e in entries if e["kind"] == "result"]
    assert results, entries
    assert not any(key in json.dumps(entries) for key in KEYS.values()), "key reached record"
    assert [e for e in entries if e["kind"] == "call"][-1]["name"].endswith(verb), entries
    entry = results[-1]
    error = entry["outcome"] == "error"
    observed = {"error": error, "output": entry["output"] if error else json.loads(entry["output"])}
    return answer["session"], observed, entries


# Every provider/verb has a success, and a reported failure followed by another
# provider's success. Two Responses statuses are distinct documented failures.
cases = [(p, v) for p in PROVIDERS for v in PATHS[p]]
for provider, verb in cases:
    fallback = next(p for p in PROVIDERS if p != provider and verb in PATHS[p])
    arguments = {"url": URL} if verb == "fetch" else {"query": "web evidence"}
    bodies = [success(provider, verb), failure(provider, verb), success(fallback, verb)]
    with TemporaryDirectory(prefix="razor-web-") as scratch:
        requests = Path(scratch)
        with serve([Reply(json.dumps(b).encode()) for b in bodies], requests) as url:
            replies = [reply(None, f"mcp__razor__{verb}", arguments), reply(None),
                       reply(None, f"mcp__razor__{verb}", arguments), reply(None)]
            with flow(replies, config=config(url, verb, [provider, fallback]), env=KEYS) as f:
                session, first, _ = invoke(f, verb)
                _, second, _ = invoke(f, verb, session)
                for entry, expected in [(first, provider), (second, fallback)]:
                    assert not entry["error"], entry
                    output = entry["output"]
                    assert output["output"].startswith(expected + "\n"), output
                    assert URL in output["output"], output
                    if verb != "fetch":
                        assert "web evidence" in output["output"] and "[redacted]" in output["output"], output
                    if verb == "search":
                        assert all(URL + str(i) in output["output"] for i in range(10)), output
                        assert output["path"] is None and len(output["output"]) < 30_000, output
                offered = json.loads((f.requests / "001.body").read_bytes())["tools"]
                names = {t["name"] for t in offered}
                # Claude Code's own web tools are always there, beside a configured verb.
                assert f"mcp__razor__{verb}" in names and {"WebSearch", "WebFetch"} <= names, names
                # The verb says to use the native tools first and for routine work (Trey, 2026-10-29).
                told = next(t for t in offered if t["name"] == f"mcp__razor__{verb}")["description"]
                assert "native web tool" in told and "second, independent" in told, told
            # Ignore unrelated port-probe GETs; all three provider calls must still be POSTs.
            sent = [r for p in sorted(requests.glob("*.json"))
                    if (r := json.loads(p.read_text()))["method"] == "POST"]
            assert [r["path"] for r in sent] == [PATHS[provider][verb]] * 2 + [PATHS[fallback][verb]], sent
            for i, p in enumerate([provider, provider, fallback], 1):
                headers = sent[i - 1]["headers"]
                key = KEYS[f"{p.upper()}_API_KEY"]
                header = "authorization" if p == "firecrawl" else "x-api-key"
                assert headers.get(header) == (f"Bearer {key}" if p == "firecrawl" else key), headers
                body = json.loads((requests / f"{i:03}.body").read_bytes())
                assert URL in json.dumps(body) if verb == "fetch" else "web evidence" in json.dumps(body), body

# Non-success bytes, empty text, incomplete answers, find (literal), whole-file
# spill and exhaustion share the same installed-binary path, not a private API.
extras = [
    ("fetch", ["exa", "firecrawl"], [b"not JSON", success("firecrawl", "fetch")], {}, "firecrawl", False),
    ("fetch", ["exa", "parallel"], [{"results": [{"text": "  "}]}, success("parallel", "fetch")], {}, "parallel", False),
    ("answer", ["parallel", "exa"], [{**success("parallel", "answer"), "status": "incomplete"}, success("exa", "answer")], {}, "exa", False),
    ("fetch", ["firecrawl", "exa"], [success("firecrawl", "fetch")], {"find": "not-present.*"}, "firecrawl", False),
    ("fetch", ["firecrawl"], [{"success": True, "data": {"markdown": "spill evidence\n" * 3000 + "flow-firecrawl-secret"}}], {}, "firecrawl", False),
    ("fetch", list(PROVIDERS), [failure(p, "fetch") for p in PROVIDERS], {}, "parallel", True),
]
for verb, providers, bodies, fields, expected, error in extras:
    with TemporaryDirectory(prefix="razor-web-") as scratch:
        encoded = [Reply(b if isinstance(b, bytes) else json.dumps(b).encode()) for b in bodies]
        with serve(encoded, Path(scratch)) as url:
            arguments = {**({"url": URL} if verb == "fetch" else {"query": "web evidence"}), **fields}
            with flow([reply(None, f"mcp__razor__{verb}", arguments), reply(None)],
                      config=config(url, verb, providers), env=KEYS) as f:
                session, result, entries = invoke(f, verb)
                assert result["error"] == error, result
                output = result["output"] if error else result["output"]["output"]
                assert output.startswith(expected + "\n"), output
                if fields:
                    assert output == "firecrawl\nNo lines match find.", output
                if not error and result["output"]["path"]:
                    whole = Path(result["output"]["path"]).read_text()
                    assert len(whole) > 30_000 and whole.startswith("firecrawl\n"), whole[:100]
                    assert "flow-firecrawl-secret" not in whole and "[redacted]" in whole, whole[-100:]
                if error:
                    assert all(p + ":" in output for p in PROVIDERS), output
                    assert "another URL" in output and "[web]" in output, output

# A configured provider without a key still leaves the vendor's own names.
with flow([reply(None)], config='[web]\nsearch = ["exa"]\nfetch = ["firecrawl"]\n') as f:
    result = f.run("--print=json", "Check web declarations.")
    assert result.returncode == 0, result.stderr
    names = {t["name"] for t in json.loads((f.requests / "001.body").read_bytes())["tools"]}
    assert {"WebSearch", "WebFetch"} <= names, names
    assert not {"mcp__razor__search", "mcp__razor__fetch", "mcp__razor__answer"} & names, names

# GPT's hosted search is always there; the search verb keys on the action.
for web, custom in [('[web]\nsearch = []\n', False), ('[web]\nsearch = ["exa"]\n', True)]:
    with flow([done()], model="gpt-6.1-sol", config=PLAIN + web, env=KEYS) as f:
        result = f.run("--print=json", "Check search declarations.")
        assert result.returncode == 0, result.stderr
        tools = json.loads((f.requests / "001.body").read_bytes())["tools"]
        assert any(t.get("name") == "search" for t in tools) == custom, tools
        assert any(t["type"] == "web_search" for t in tools), tools

# A verb becoming available changes the recorded version and rebuilds at admission.
with flow([reply(None), reply(None)], config='[profiles.claude]\ncode_mode = true\n[web]\nsearch = ["exa"]\nfetch = []\nanswer = []\n', env=KEYS) as f:
    first = f.run("--print=json", "First declaration set.")
    assert first.returncode == 0, first.stderr
    session = json.loads(first.stdout)["session"]
    path = f.home / ".razor/config.toml"
    path.write_text(path.read_text().replace('fetch = []', 'fetch = ["firecrawl"]'))
    second = f.run("--print=json", "--resume", session, "Second declaration set.")
    assert second.returncode == 0, second.stderr
    entries = list(f.record(session))
    versions = [e["profile_version"] for e in entries if e["kind"] in ("start", "rebuild")]
    assert len(versions) == 2 and versions[0].endswith("-web-100") and versions[1].endswith("-web-110"), versions
    names = {t["name"] for t in json.loads((f.requests / "002.body").read_bytes())["tools"]}
    assert "WebFetch" in names and "### `fetch`" in json.dumps(json.loads((f.requests / "002.body").read_bytes())["tools"]), names

# Detached startup must also fail the printing client and leave the reason in the server log.
with flow([], config='[web]\nsearch = ["exa", "exa"]') as f:
    result = f.run("--print", "Bad configuration.")
    assert result.returncode != 0, result
    assert "at most once" in (f.home / ".razor/server.log").read_text()

for text, message in [
    ('answer = ["firecrawl"]', "no answer action"),
    ('search = ["unknown"]', "unknown variant"),
    ('[web.exa]\nbase_url = "file:///tmp/unsafe"', "HTTP or HTTPS"),
]:
    with flow([], config="[web]\n" + text) as f:
        result = f.run("serve")
        assert result.returncode != 0, result
        assert message in result.stderr, result

# All routes except Claude code mode add web verbs in place. Start without fetch, add it,
# then remove it and try calling it: the old declaration must grant no authority.
from claude import reply as claude_reply
from responses import call, response
for model, mode in (("opus", ""), ("sol", PLAIN), ("sol", ""), ("gemini-3.8-flash", "chat")):
    if mode == "chat":
        mode = '[models."gemini-3.8-flash"]\nbase_url="{standin}"\n'
        def chat(delta, finish):
            frames = [{"choices": [{"index": 0, "delta": delta}]},
                      {"choices": [{"index": 0, "delta": {}, "finish_reason": finish}]}]
            return Reply(("".join(f"data: {json.dumps(e)}\n\n" for e in frames) + "data: [DONE]\n\n").encode())
        end = chat({"role": "assistant", "content": "Done."}, "stop")
        removed = chat({"tool_calls": [{"index": 0, "id": "removed", "type": "function",
                        "function": {"name": "fetch", "arguments": '{"url":"https://example.com"}'}}]}, "tool_calls")
    elif model == "opus":
        end, removed = claude_reply(text="Done."), claude_reply(("removed", "mcp__razor__fetch", {"url": URL}))
    else:
        end = done()
        removed = (response([call("fetch", json.dumps({"url": URL}), "removed")]) if mode == PLAIN else
                   response([{"type": "custom_tool_call", "id": "ct_removed", "call_id": "removed", "name": "exec",
                              "input": f"text(await tools.fetch({{url: {json.dumps(URL)}}}));"}]))
    with flow([end, end, removed, end], model=model, config=mode + '\n[web]\nfetch=[]\n', env={**KEYS, "GEMINI_API_KEY": "flow-gemini-key"}) as f:
        first = f.run("--print=json", "Remember this conversation.")
        assert first.returncode == 0, first.stderr
        session = json.loads(first.stdout)["session"]
        path = f.home / ".razor/config.toml"
        path.write_text(path.read_text().replace('fetch=[]', 'fetch=["exa"]'))
        assert (added := f.run("--print=json", "--resume", session, "Fetch is available.")).returncode == 0, added.stderr
        tools = json.dumps(json.loads((f.requests / "002.body").read_bytes()))
        assert '"name": "fetch"' in tools or "mcp__razor__fetch" in tools or "### `fetch`" in tools, tools
        path.write_text(path.read_text().replace('fetch=["exa"]', 'fetch=[]'))
        assert (last := f.run("--print=json", "--resume", session, "Fetch is removed.")).returncode == 0, last.stderr
        entries = list(f.record(session))
        assert not any(e["kind"] == "rebuild" for e in entries), entries
        result = next(e for e in entries if e["kind"] == "result" and e["call"] == "removed")
        assert result["outcome"] == "error", result
