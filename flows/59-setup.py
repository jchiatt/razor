"""Setup and keys in a real terminal: hidden keys saved at 0600 and kept on Enter, a rerun changing nothing,
serve loading them, and --console reaching Claude Code's own sign-in. Provider auth files, the Claude Code
CLI and the browser installer stand in for other people's software (docs/design/setup.md)."""
import json
import os
import select
import shutil
import subprocess
import time
import tomllib
from driver import BINARY, Reply, flow
from model import reply
from standin import serve


def terminal(f, argv, answers, binary=BINARY):
    master, slave = os.openpty()
    process = subprocess.Popen([binary, *argv], stdin=slave, stdout=slave, stderr=slave,
                               env=f.env, cwd=f.cwd, start_new_session=True)
    os.close(slave)
    output, cursor = b"", 0
    try:
        for prompt, answer in answers:
            deadline = time.monotonic() + 30
            while prompt.encode() not in output[cursor:]:
                assert time.monotonic() < deadline and process.poll() is None, f"{argv} did not reach {prompt}: {output!r}"
                if select.select([master], [], [], .1)[0]:
                    try:
                        output += os.read(master, 65536)
                    except OSError:  # The program has ended; the check above says what it printed.
                        time.sleep(.05)
            cursor = len(output)
            os.write(master, (answer + "\r").encode())
        deadline = time.monotonic() + 30
        while process.poll() is None:
            assert time.monotonic() < deadline, f"{argv} did not finish"
            if select.select([master], [], [], .1)[0]:
                try:
                    output += os.read(master, 65536)
                except OSError:
                    break
        assert process.wait(timeout=5) == 0, output
        return output
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)


with flow([reply(None, "mcp__razor__search", {"query": "setup saved key"}), reply(None)]) as f:
    browser = f.home / "agent-browser"
    browser.write_text("#!/bin/sh\nexit 0\n")
    browser.chmod(0o755)
    f.env["PATH"] = f"{f.home}:{f.env['PATH']}"
    secret = "flow-setup-exa-hidden"
    keys = [("EXA_API_KEY:", secret), ("FIRECRAWL_API_KEY:", ""),
            ("PARALLEL_API_KEY:", ""), ("ELEVENLABS_API_KEY:", "")]
    ending = [("background when you log in", ""), ("Open Razor for your first message", "")]
    saved, person, you = (f.home / ".razor" / name for name in ("keys.toml", "config.toml", "about.md"))
    before = person.read_bytes()
    terminal(f, ["setup"], [("Use Claude, ChatGPT or both", "chatgpt"), *ending])
    assert not saved.exists() and not you.exists() and person.read_bytes() == before, "setup asked for or wrote more than providers and the service"
    # Keys are typed hidden, saved together at 0600, and Enter keeps a saved one.
    screen = terminal(f, ["keys"], keys)
    assert secret.encode() not in screen, "keys echoed the typed key"
    assert tomllib.loads(saved.read_text()).get("EXA_API_KEY") == secret, "keys did not save the key"
    assert saved.stat().st_mode & 0o777 == 0o600, "keys.toml is not owner-only"
    kept = saved.read_bytes()
    terminal(f, ["keys"], [(p, "") for p, _ in keys])
    assert saved.read_bytes() == kept and saved.stat().st_mode & 0o777 == 0o600, "Enter did not keep the saved key"
    terminal(f, ["setup"], [("Use Claude, ChatGPT or both", ""), *ending])
    assert [p.read_bytes() for p in (saved, person)] == [kept, before], "rerunning setup overwrote saved files"
    # A newly spawned serve must read keys.toml itself. No key is in the client's environment.
    with serve([Reply(b'{"results":[{"title":"Found","url":"https://example.invalid","text":"saved key works"}]}')],
               f.home / "web-requests") as url:
        with person.open("a") as config:
            config.write(f'\n[web.exa]\nbase_url = "{url}"\n')
        result = f.run("--model", "opus", "--print=json", "Search with the saved key.")
        assert result.returncode == 0, result.stderr
        sent = json.loads((f.home / "web-requests/001.json").read_text())
        assert sent["headers"].get("x-api-key") == secret, "serve did not load the saved key"
    # Claude Code's CLI is signed in. Plain setup leaves it; --console runs its console sign-in.
    bundle = f.home / "bundle"
    cli = bundle / "bridge/node_modules/@anthropic-ai/claude-agent-sdk-flow/claude"
    cli.parent.mkdir(parents=True)
    (bundle / "bridge/main.ts").touch()
    cli.write_text('#!/bin/sh\necho "$*" >> "$HOME/claude-calls"\n'
                   '[ "$1 $2" = "auth status" ] && echo \'{"loggedIn":true,"authMethod":"flow"}\'\nexit 0\n')
    cli.chmod(0o755)
    shutil.copy2(BINARY, bundle / "razor")
    calls = f.home / "claude-calls"
    for option in ([], ["--console"]):
        terminal(f, ["setup", *option], [("Use Claude, ChatGPT or both", ""), *ending], bundle / "razor")
        logins = [l for l in (calls.read_text() if calls.exists() else "").splitlines() if l.startswith("auth login")]
        assert logins == (["auth login --console"] if option else []), (option, logins)
    saved.write_text('EXA_API_KEY = "flow-nul-prefix\\u0000flow-nul-suffix"\n')
    refused = f.run("doctor")
    assert refused.returncode != 0 and "flow-nul-" not in refused.stdout + refused.stderr, "NUL key was accepted or printed"
