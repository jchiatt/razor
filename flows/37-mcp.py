"""Project files and live MCP management (the CLI lists and previews in plain text), PKCE/rotation,
native search, rich results and questions.
Only external MCP/OAuth/model servers are stand-ins; the installed Razor and Claude SDK are real.
New checks need a mutation red-proof before they are claimed proven.
"""
from dataclasses import replace
from pathlib import Path
from urllib.parse import urlencode, urlsplit
import base64
import json
import os
import shlex
import signal
import subprocess
import sys
import time

import claude
import mcp
import responses
from driver import BINARY, flow
from server import Local, gone, post, send, start, until, wait


def api(f, path, *, status=200, authenticated=True, sent=None):
    connection = Local(f.home / ".razor/server.sock")
    headers = {"Authorization": "Bearer " + (f.home / ".razor/device.key").read_text().strip()} if authenticated else {}
    if sent is not None:
        headers["Content-Type"] = "application/json"
    connection.request("GET" if sent is None else "POST", "/v1" + path, sent and json.dumps(sent), headers=headers)
    response = connection.getresponse()
    body = response.readline() if path == "/status" else response.read()
    connection.close()
    assert response.status == status, (path, response.status, body)
    return json.loads(body)


def cli(f, *args):
    result = f.run("mcp", *args)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def operation(f, op, session=None, **fields):
    return post(f, "/mcp", {"cwd": str(f.cwd), "session": session, "op": op, **fields})


def create(f):
    """An attached real API client keeps cold sessions and their per-file last-good values live."""
    session = post(f, "/sessions", {"cwd": str(f.cwd)})["session"]
    stream = send(f, "GET", f"/sessions/{session}/events")
    assert stream.status == 200, stream.read()
    f.__dict__.setdefault("streams", []).append(stream)
    return session


def connected(f, session, name):
    return wait(lambda: next((r for r in operation(f, "list", session) if r["name"] == name and r["state"] == "connected"), None),
                15, f"{name} never connected")


def files_case():
    model_replies = []
    with flow(model_replies, env={key: "WEB_KEY_MUST_NOT_INHERIT" for key in ("EXA_API_KEY", "FIRECRAWL_API_KEY", "PARALLEL_API_KEY")}) as f:
        f.base()
        pack = f.home / "pack"
        pack.mkdir()
        (pack / "config.toml").write_text('[mcp.packonly]\nenabled=false\n[mcp.shared]\nenabled=false\nurl="http://localhost/pack"\n')
        person = f.home / ".razor/config.toml"
        person.write_text('pack=' + json.dumps(str(pack)) + '\n' + person.read_text() +
                          '\n[mcp.persononly]\nenabled=false\n[mcp.shared]\nenabled=false\nurl="http://localhost/person"\n')
        directory = f.cwd / ".razor"
        directory.mkdir()
        project, private = directory / "config.toml", directory / "config.local.toml"
        shared = '[mcp.projectonly]\nenabled=false\n[mcp.shared]\nenabled=false\nurl="http://localhost/project"\n'
        project.write_text(shared)
        private.write_text('# private comment survives edits\neffort="high"\n[mcp.shared]\nenabled=false\n')
        start(f)
        session = create(f)
        settings = lambda: api(f, f"/sessions/{session}/settings")
        first = settings()
        assert set(first["config"]["mcp"]) == {"packonly", "persononly", "projectonly", "shared"}, first
        assert first["config"]["mcp"]["shared"] == {"enabled": False}, first
        assert first["sources"]["mcp.shared"] == str(private), first
        assert first["sources"]["mcp.packonly"] == str(pack / "config.toml"), first
        assert first["sources"]["mcp.persononly"] == str(person), first
        # A session id inherited from another home's agent names nothing here, so it is ignored.
        foreign = subprocess.run([BINARY, "mcp", "list"], env={**f.env, "RAZOR_SESSION": "20261104-000000-abcd"},
                                 capture_output=True, text=True, timeout=30)
        assert foreign.returncode == 0, foreign
        assert foreign.stdout.splitlines()[:2] == ["packonly: disabled (pack)", "persononly: disabled (global)"], foreign
        project.write_text(shared + '[mcp.zero]\nurl="http://localhost/zero"\ntimeout=0\n')
        zero = settings()
        assert any("nonzero" in error for error in zero["errors"]), zero
        assert "zero" not in zero["config"]["mcp"], zero
        project.write_text(shared)
        cli(f, "enable", "shared", "--scope", "private")
        assert settings()["config"]["mcp"]["shared"]["url"].endswith("/project")
        assert "# private comment survives edits" in private.read_text()
        project.write_text('not valid TOML [')
        person.write_text(person.read_text() + '\n[mcp.later]\nenabled=false\n')
        bad = settings()
        assert "later" in bad["config"]["mcp"] and bad["config"]["mcp"]["shared"]["url"].endswith("/project"), bad
        assert any(str(project) in error for error in bad["errors"]), bad
        failed = f.run("mcp", "disable", "shared", "--scope", "project")
        assert failed.returncode != 0 and project.read_text() == 'not valid TOML [', failed
        project.write_text('pack="forbidden"\nask=["bad"]\nnotify=["bad"]\nstartup=["bad.md"]\n' + shared +
                           '[server]\naddress="127.0.0.1:1"\n[guards]\ndelete=false\n[brand]\nname="forbidden"\n'
                           '[responses]\nbase_url="http://localhost/forbidden"\n[[plugins]]\nfile="bad.js"\n'
                           '[residents.bad]\ncwd="."\nstartup=["bad.md"]\n[sessions]\nidle=1\n')
        refused = settings()
        assert all(key in " ".join(refused["errors"]) for key in
                   ("server", "guards", "responses.base_url", "plugins", "pack", "ask", "notify", "brand", "residents", "sessions", "startup")), refused
        assert refused["config"]["mcp"] == bad["config"]["mcp"], refused
        person.write_text(person.read_text() + '\ninvalid [')
        packfile = pack / "config.toml"
        packgood = packfile.read_text()
        packfile.write_text('invalid [')
        project.write_text(shared + '[mcp.newproject]\nenabled=false\n')
        fallback = settings()
        assert {"later", "newproject", "packonly"} <= fallback["config"]["mcp"].keys(), fallback
        assert all(any(str(path) in e for e in fallback["errors"]) for path in (person, packfile)), fallback
        person.write_text(person.read_text().removesuffix('\ninvalid ['))
        packfile.write_text(packgood)
        linked = f.cwd.parent / "linked"
        f.git("worktree", "add", "-b", "flow37", str(linked))
        (linked / ".razor").mkdir()
        (linked / ".razor/config.toml").write_text('[mcp.wrong]\nenabled=false\n')
        work = replace(f, cwd=linked)
        other = create(work)
        snapshot = api(f, f"/sessions/{other}/settings")
        assert snapshot["project"] == str(linked) and snapshot["sources"]["mcp.shared"] == str(project), snapshot
        assert "wrong" not in snapshot["config"]["mcp"], snapshot
        definition = {"command": sys.executable, "args": [str(Path(mcp.__file__).resolve())], "env": {"MCP_LITERAL": "literal"}}
        operation(work, "add", name="legacy", scope="project", config=definition)
        other = create(work)
        connected(work, other, "legacy")
        # This is the real legacy process, executing at the linked checkout, not the settings root.
        replies = [claude.reply(("where37", "mcp__razor__mcp__legacy__where", {})), claude.reply(text="done")]
        # flow's model stand-in reads its original list by reference.
        model_replies.extend(replies)
        result = work.run("--resume", other, "--print=json", "Read the local MCP directory.")
        assert result.returncode == 0, result.stderr
        output = next(e for e in work.record(other) if e["kind"] == "result")
        assert str(linked) + ":literal" in output["output"], output
        assert json.loads(output["output"])["structured"]["inherited_web_keys"] == [], output
        import_case(f, project, definition)
    print("PASS 37-mcp files, sources, fallback, worktree, legacy and import", flush=True)


def import_case(f, project, definition):
    imported = f.cwd / "import.json"
    imported.write_text(json.dumps({"mcpServers": {"legacy": {**definition, "env": {"SECRET": "IMPORT_SECRET"}},
        "remote": {"url": "http://localhost/mcp", "headers": {"Authorization": "IMPORT_SECRET"}},
        "token": {"url": "http://localhost/token", "oauth": {"access_token": "TOKEN_MUST_NOT_COPY"}}}}))
    before = project.read_bytes()
    shown = f.run("mcp", "import", str(imported), "--scope", "project")
    assert shown.returncode == 0 and project.read_bytes() == before, shown
    lines = shown.stdout.splitlines()
    assert "Replace: legacy" in lines and "Needs a value: legacy: env.SECRET" in lines, lines
    assert "Needs a value: remote: headers.Authorization" in lines and "Add: remote" in lines, lines
    assert "IMPORT_SECRET" not in shown.stdout and "TOKEN_MUST_NOT_COPY" not in shown.stdout, lines
    assert "/mcp import" in lines[-1], "the preview does not say how to apply it"
    preview = operation(f, "import", name=str(imported), source=str(imported), scope="project")
    assert project.read_bytes() == before and "IMPORT_SECRET" not in json.dumps(preview), preview
    assert "TOKEN_MUST_NOT_COPY" not in json.dumps(preview) and "legacy" in preview["replacements"], preview
    assert {(r["name"], r["field"]) for r in preview["unresolved"]} == {("legacy", "env.SECRET"), ("remote", "headers.Authorization")}, preview
    assert all(op["op"] == "add" and op["scope"] == "project" for op in preview["operations"]), preview
    for filename, value in (("codex.toml", '[mcp_servers.codex]\nurl="http://localhost/codex"\ntool_timeout_sec=2\n'),
                            ("opencode.json", json.dumps({"mcp": {"opencode": {"command": ["echo", "local"]}}}))):
        source = f.cwd / filename
        source.write_text(value)
        preview = operation(f, "import", name=str(source), source=str(source), scope="private")
        assert len(preview["operations"]) == 1 and not preview["errors"] and not preview["unresolved"], preview
        if filename == "codex.toml":
            assert preview["operations"][0]["config"]["timeout"] == 2000, preview
        assert project.read_bytes() == before, "import wrote instead of previewing"


def exclude_case():
    """`mcp add` creating the private file keeps it out of git, the exclusion listed once."""
    with flow([]) as f:
        f.base()
        private, exclude = f.cwd / ".razor/config.local.toml", f.cwd / ".git/info/exclude"
        for name in ("first", "second"):
            private.unlink(missing_ok=True)
            cli(f, "add", name, "--config", json.dumps({"url": f"http://localhost/{name}"}))
            assert private.exists() and f.git("status", "--porcelain") == "", f.git("status", "--porcelain")
        assert exclude.read_text().count(".razor/config.local.toml") == 1, exclude.read_text()


def oauth_case():
    with mcp.serve() as external, flow([]) as f:
        start(f)
        for name, configured in (("dynamic", False), ("configured", True)):
            config = {"url": external.url + "/secure"}
            if configured:
                config["oauth"] = {"client_id": "configured-37", "issuer": external.url}
            cli(f, "add", name, "--scope", "global", "--config", json.dumps(config))
            authorization = cli(f, "sign-in", name)["url"]
            redirect = external.authorize(authorization)
            api(f, "/mcp/callback?" + urlencode({"redirect": redirect.replace("http://localhost/", "http://wrong.invalid/")}), status=400)
            assert "code_challenge=" in authorization and "code_challenge_method=S256" in authorization, authorization
            if configured:
                assert cli(f, "complete", redirect)["signed_in"] is True
            else:
                assert api(f, "/mcp/callback?" + urlsplit(redirect).query, authenticated=False)["signed_in"] is True
            api(f, "/mcp/callback?" + urlencode({"redirect": redirect}), status=400)
        assert len(external.registrations) == 1 and len(external.exchanges) == 2, external.registrations
        session = create(f)
        for name in ("dynamic", "configured"):
            assert connected(f, session, name)["signed_in"], name
        assert len(external.rotations) == 2, external.rotations
        credential = f.home / ".razor/mcp-auth.json"
        assert credential.stat().st_mode & 0o777 == 0o600
        saved = credential.read_text()
        for old in external.rotations:
            assert old not in saved, "rotated refresh token was not persisted"
        assert all(token in saved for token in external.refresh), saved
        public = json.dumps(operation(f, "list", session)) + json.dumps(api(f, f"/sessions/{session}/settings"))
        assert "access-secret" not in public and "refresh-secret" not in public, public
        cli(f, "reconnect", "dynamic")
        connected(f, session, "dynamic")
        assert len(external.rotations) == 2, "reconnect lost the refreshed token"
        cli(f, "sign-out", "dynamic")
        assert not next(r for r in operation(f, "list") if r["name"] == "dynamic")["signed_in"]
        assert next(r for r in operation(f, "list") if r["name"] == "configured")["signed_in"]
        authorization = operation(f, "sign_in", session, name="dynamic")["url"]
        question = next(e for e in until(f, session, lambda es: any(e["kind"] == "question" for e in es)) if e["kind"] == "question")
        assert authorization in question["text"] and len(external.registrations) == 1, question
        redirect = external.authorize(authorization)
        # Pasted into the composer and sent, the landed address answers the open sign-in ask.
        post(f, f"/sessions/{session}/input", {"text": f" {redirect}\n"})
        api(f, f"/sessions/{session}/input", status=400, sent={"text": redirect})
        entries = list(f.record(session))
        assert any(e["kind"] == "input" and e["from"] == {"answer": {"ask": question["id"]}} for e in entries), entries[-3:]
        assert redirect not in json.dumps(entries), "the pasted authorization code entered the record"
        assert not list(f.requests.glob("[0-9]*.body")), "sign-in admitted an agent turn"
        assert next(r for r in operation(f, "list") if r["name"] == "dynamic")["signed_in"]
        # Finished at the callback instead, a session's sign-in still answers its question, which
        # is recorded before the address is returned.
        cli(f, "sign-out", "dynamic")
        authorization = operation(f, "sign_in", session, name="dynamic")["url"]
        question = next(e for e in f.record(session) if e["kind"] == "question" and authorization in e["text"])
        redirect = external.authorize(authorization)
        assert api(f, "/mcp/callback?" + urlsplit(redirect).query, authenticated=False)["signed_in"] is True
        entries = list(f.record(session))
        assert any(e["kind"] == "input" and e["from"] == {"answer": {"ask": question["id"]}} for e in entries), entries[-3:]
        assert redirect not in json.dumps(entries), "the callback's authorization code entered the record"
        assert not list(f.requests.glob("[0-9]*.body")), "sign-in admitted an agent turn"
        # Begun in a shell, so no session asked, a pasted address still completes the sign-in, and
        # the draft the terminal saved it in is cleared.
        cli(f, "sign-out", "dynamic")
        redirect = external.authorize(cli(f, "sign-in", "dynamic")["url"])
        draft = f.home / ".razor/sessions" / session / "draft"
        draft.write_text(redirect)
        assert api(f, f"/sessions/{session}/input", sent={"text": redirect}) == {"signed_in": True}
        assert next(r for r in operation(f, "list") if r["name"] == "dynamic")["signed_in"]
        assert draft.read_text() == "" and redirect not in json.dumps(list(f.record(session)))
        # Under an HTTPS address spelt another way, with sign-ins open in two sessions, each
        # landed address answers the ask of the session that began it, though both are pasted
        # into the first.
        with (f.home / ".razor/config.toml").open("a") as config:
            config.write('[server]\nhttps_url = "https://Razor.Example:443"\n')
        time.sleep(1.5)  # the configuration is read once a second
        sessions = [session, create(f)]
        urls = [operation(f, "sign_in", s, name=name)["url"] for s, name in zip(sessions, ("dynamic", "configured"))]
        asks = [next(e["id"] for e in until(f, s, lambda es: any(e["kind"] == "question" and u in e["text"] for e in es))
                     if e["kind"] == "question" and u in e["text"]) for s, u in zip(sessions, urls)]
        for asker, url, ask in reversed(list(zip(sessions, urls, asks))):
            redirect = external.authorize(url)
            assert redirect.startswith("https://razor.example/v1/mcp/callback?"), redirect
            post(f, f"/sessions/{session}/input", {"text": redirect})
            entries = list(f.record(asker))
            assert any(e["kind"] == "input" and e["from"] == {"answer": {"ask": ask}} for e in entries), entries[-3:]
            assert redirect not in json.dumps(entries + list(f.record(session))), "the pasted authorization code entered the record"
    print("PASS 37-mcp OAuth configured/dynamic PKCE, callbacks, rotation and sign-out", flush=True)


def model_reply(code, name=None, arguments=None, id="call37", text=None):
    if not code:
        return claude.reply(*([] if name is None else [(id, name if name == "Bash" else "mcp__razor__" + name, arguments)]), text=text)
    if name is None:
        return responses.done()
    source = f'text(await tools.{name}({json.dumps(arguments)}))'
    return responses.response([{"type": "custom_tool_call", "id": "ct_" + id,
                               "call_id": id, "name": "exec", "input": source}])


def blocks(value):
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from blocks(child)
    elif isinstance(value, list):
        for child in value:
            yield from blocks(child)


def cached(value):
    """Ignore moving cache markers and the SDK's equivalent text/string serialization only."""
    if isinstance(value, list):
        return [cached(child) for child in value]
    if isinstance(value, dict):
        return {key: cached([{"type": "text", "text": child}] if key == "content" and isinstance(child, str) else child)
                for key, child in value.items() if key != "cache_control"}
    return value


def unchanged(first, later, code):
    if code:
        assert later["input"][:len(first["input"])] == first["input"], "Responses rewrote cached input prefix"
    else:
        assert cached(later["system"]) == cached(first["system"]) and cached(later["tools"]) == cached(first["tools"]), "Claude rewrote cached declarations"
        assert cached(later["messages"][:len(first["messages"])]) == cached(first["messages"]), "Claude rewrote cached messages"


def check_question(f, session, question, word, url):
    row = next(row for row in api(f, "/status")["agents"] if row["session"] == session)
    assert row["state"] == "needs_you", row
    assert "remote" in question["text"] and word in question["text"], question
    if word == "form":
        assert all(field in question["text"] for field in ("name", "count")), question
    else:
        assert url + "/consent" in question["text"] and "127.0.0.1" in question["text"], question


def native_case(code):
    replies = []
    with mcp.serve() as external, flow(replies, model="gpt-6.1-sol" if code else "opus",
            config='[tools.mcp_search]\ncommand="printf shadowed"\ndescription="Wrong search"\ninput={type="object"}',
            env={"MCP_FLOW_ENV": "ENV_HEADER_SECRET", "EXA_API_KEY": "EXA_HEADER_SECRET"}) as f:
        start(f)
        (f.cwd / ".razor").mkdir()
        (f.cwd / ".razor/header").write_text("FILE_HEADER_SECRET\n")
        external.large = not code  # Claude proves automatic size choice; GPT proves the override.
        config = {"url": external.url + "/mcp", "tools": ["rich", "form", "url", "view"],
                  "headers": {"X-Flow-Env": {"env": "MCP_FLOW_ENV"}, "X-Flow-File": {"file": "header"}, "X-Exa-Key": {"env": "EXA_API_KEY"}}}
        if code: config["search"] = True
        operation(f, "add", name="remote", scope="project", config=config)
        session = create(f)
        row = connected(f, session, "remote")
        assert len(row["tools"]) == 3 and row["file"] == str(f.cwd / ".razor/config.toml"), row
        shell = "exec_command" if code else "Bash"
        command = lambda verb: {"command" if not code else "cmd": f'{shlex.quote(BINARY)} mcp {verb} remote --scope project'}
        steps = [("search37", "mcp_search", {"query": "rich Flow"}), ("rich37", "mcp__remote__rich", {"value": "first"}),
                 ("form37", "mcp__remote__form", {}), ("url37", "mcp__remote__url", {}),
                 ("disable37", shell, command("disable")), ("stale37", "mcp__remote__rich", {})]
        replies.extend(model_reply(code, name, args, id=id) for id, name, args in steps)
        replies.append(model_reply(code, text="done"))
        post(f, f"/sessions/{session}/input", {"text": "Search rich, call immediately, ask both questions, then disable and try the old tool."})
        observed = until(f, session, lambda es: any(e["kind"] == "result" and e["call"] == "rich37" for e in es))
        first_call = next(e for e in observed if e["kind"] == "result" and e["call"] == "rich37")
        assert first_call["outcome"] == "success" and "MCP_RICH_37" in first_call["output"], ("native call immediately after search failed", first_call)
        for number, (word, answer) in enumerate((("form", '{"name":"Trey","count":2}'), ("url", "accept")), 1):
            entries = until(f, session, lambda es: len([e for e in es if e["kind"] == "question"]) == number or any(e["kind"] == "turn_end" for e in es))
            questions = [e for e in entries if e["kind"] == "question"]
            assert len(questions) == number, [e for e in entries if e["kind"] == "result"]
            question = questions[-1]
            check_question(f, session, question, word, external.url)
            post(f, f"/sessions/{session}/asks/{question['id']}", {"text": answer})
        entries = until(f, session, lambda es: any(e["kind"] == "turn_end" for e in es), timeout=60)
        outputs = {e["call"]: e for e in entries if e["kind"] == "result"}
        search = outputs["search37"]["output"]
        assert "mcp__remote__rich" in search and "schema" not in search.lower() and "properties" not in search, search
        assert "MCP_STRUCTURED_37" in outputs["rich37"]["output"] and "MCP_RICH_37" in outputs["rich37"]["output"], outputs
        assert outputs["stale37"]["outcome"] == "error" and ("tool" in outputs["stale37"]["output"].lower() or "not a function" in outputs["stale37"]["output"]), outputs["stale37"]
        assert outputs["disable37"]["id"] < outputs["stale37"]["id"] < next(e["id"] for e in entries if e["kind"] == "turn_end"), outputs
        assert next(r for r in operation(f, "list") if r["name"] == "remote")["config"]["enabled"] is False
        calls = [r for r in external.requests if r["body"]["method"] == "tools/call"]
        assert [r["body"]["params"]["name"] for r in calls] == ["rich", "form", "form", "url", "url"], calls
        headers = [{k.lower(): v for k, v in r["headers"].items()} for r in calls]
        assert all(h.get("mcp-protocol-version") == "2026-07-28" for h in headers), calls
        meta = [r["body"]["params"].get("_meta", {}) for r in calls]
        assert all(m.get("razor/home") == str(f.home / ".razor") and m.get("razor/session") == session for m in meta), meta
        assert all(h.get("x-flow-env") == "ENV_HEADER_SECRET" and h.get("x-flow-file") == "FILE_HEADER_SECRET" and h.get("x-exa-key") == "EXA_HEADER_SECRET" for h in headers), calls
        assert "HEADER_SECRET" not in json.dumps(row), row
        images = list((f.home / ".razor/sessions" / session / "images").glob("mcp-*.png"))
        assert images and images[0].read_bytes() == base64.b64decode(mcp.PNG), images
        requests = [json.loads(p.read_text()) for p in sorted(f.requests.glob("[0-9]*.body"))]
        assert len(requests) == 7, len(requests)
        first, found, rich = requests[:3]
        assert "mcp__remote__rich" not in json.dumps(first), "searched tool was declared upfront"
        offered = [b for b in blocks(found) if b.get("type") == ("additional_tools" if code else "tool_addition")]
        assert offered and "mcp__remote__rich" in json.dumps(offered) and "Flow rich tool" in json.dumps(offered), "found declaration missed the VERY NEXT request"
        assert "mcp__remote__hidden" not in json.dumps(found), "allow-list leaked a hidden tool"
        assert "mcp__remote__view" not in search + json.dumps(requests), "an app-only tool reached the model"
        for request in requests[1:]:
            unchanged(first, request, code)
        if not code:
            assert any(b.get("type") == "image" and b.get("source", {}).get("data") == mcp.PNG
                       for b in blocks(rich["messages"])), rich
        operation(f, "enable", name="remote", scope="project")
        ready = create(f)
        connected(f, ready, "remote")
        operation(f, "reconnect", name="remote")
        connected(f, ready, "remote")
        replies.extend([model_reply(code, shell, command("remove"), id="remove37"),
                        model_reply(code, "mcp__remote__rich", {}, id="removed37"), model_reply(code, text="done")])
        post(f, f"/sessions/{session}/input", {"text": "Remove the server and try the stale native tool in this same turn."})
        ended = until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 2, timeout=60)
        refused = next(e for e in ended if e["kind"] == "result" and e["call"] == "removed37")
        assert refused["outcome"] == "error" and ("tool" in refused["output"].lower() or "not a function" in refused["output"]), refused
        assert len([r for r in external.requests if r["body"]["method"] == "tools/call"]) == len(calls)
        assert f.run("mcp", "list").stdout.startswith("No MCP servers configured."), "removed server remains configured"
        final = json.loads(sorted(f.requests.glob("[0-9]*.body"))[-1].read_text())
        unchanged(first, final, code)
    print(f"PASS 37-mcp {'GPT code' if code else 'Claude native'} search-next-request, prefix, rich, elicitation and live refusal", flush=True)


def restart_case():
    """A conversation resumed after a restart announces no MCP change: its servers did not change."""
    replies = []
    with mcp.serve() as external, flow(replies) as f:
        pid = start(f)
        operation(f, "add", name="remote", scope="global", config={"url": external.url + "/mcp", "tools": ["rich"]})
        session = create(f)
        connected(f, session, "remote")
        replies.append(model_reply(False, text="first"))
        post(f, f"/sessions/{session}/input", {"text": "First."})
        until(f, session, lambda es: any(e["kind"] == "turn_end" for e in es))
        os.kill(pid, signal.SIGKILL)
        gone(pid)
        start(f)
        replies += [model_reply(False, "Bash", {"command": "true"}, id="bash37"), model_reply(False, text="second")]
        post(f, f"/sessions/{session}/input", {"text": "Second."})
        entries = until(f, session, lambda es: sum(e["kind"] == "turn_end" for e in es) == 2, timeout=60)
        assert not [e for e in entries if e["kind"] == "rebuild"], "the restart rebuilt instead of resuming"
        news = [e for e in entries if e["kind"] == "input" and "MCP availability" in e.get("text", "")]
        assert not news, news
    print("PASS 37-mcp restart resumes without MCP news", flush=True)


def timeout_case():
    """A server that never answers is cancelled without replay, through either route."""
    for code in (False, True):
        for legacy in (False, True):
            replies = []
            with mcp.serve() as external, flow(replies, model="gpt-6.1-sol" if code else "opus") as f:
                start(f)
                cancelled = f.cwd / "cancelled.json"
                connection = ({"command": sys.executable, "args": [str(Path(mcp.__file__).resolve())],
                               "env": {"MCP_CANCEL_FILE": str(cancelled)}} if legacy else {"url": external.url + "/mcp"})
                operation(f, "add", name="remote", scope="global", config={
                    **connection, "tools": ["hang"], "timeout": 2000})
                session = create(f)
                connected(f, session, "remote")
                replies += [model_reply(code, "mcp__remote__hang", {}, id="hang37"), model_reply(code, text="done")]
                post(f, f"/sessions/{session}/input", {"text": "Call the tool once."})
                entries = until(f, session, lambda es: any(e["kind"] == "turn_end" for e in es), timeout=30)
                call = next(e for e in entries if e["kind"] == "call" and e["call"] == "hang37")
                result = next(e for e in entries if e["kind"] == "result" and e["call"] == "hang37")
                assert result["outcome"] == "error" and "within 2000 ms" in result["output"], result
                assert "outcome is unknown" in result["output"] and "not retried" in result["output"], result
                assert result["at"] - call["at"] < 5000, (call, result)
                if legacy:
                    until(f, session, lambda _: cancelled.exists(), timeout=2)
                    assert json.loads(cancelled.read_text())["method"] == "notifications/cancelled"
                else:
                    # rmcp 2026-07-28 HTTP cancels its response stream without a wire notification.
                    assert len([r for r in external.requests if r["body"]["method"] == "tools/call"]) == 1, external.requests
            print(f"PASS 37-mcp timeout {'GPT' if code else 'Claude'} {'stdio' if legacy else 'HTTP'} unknown, no replay", flush=True)


if __name__ == "__main__":
    timeout_case()
    files_case()
    exclude_case()
    oauth_case()
    native_case(False)
    native_case(True)
    restart_case()
