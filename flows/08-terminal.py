"""A prompt-driven program under a real terminal, through Claude's terminal and plain GPT's
bash/wait/send: text and Enter reach it, screens reach the model, and secret_file records only
a path. There is no peek operation or secret flag; secrets never enter a tool argument."""
import json
import secrets

from claude import reply
from driver import flow
from responses import PLAIN, call, done, response


program = r'''import pathlib
import sys
import termios

sys.stdout.write("\x1b[2J\x1b[HOVERWRITTEN\r\x1b[2KName: ")
sys.stdout.flush()
assert sys.stdin.readline() == "Razor\n"
settings = termios.tcgetattr(sys.stdin)
hidden = settings.copy()
hidden[3] &= ~termios.ECHO
termios.tcsetattr(sys.stdin, termios.TCSANOW, hidden)
print("Secret:", end="", flush=True)
assert sys.stdin.readline().encode() == pathlib.Path("secret.txt").read_bytes()
termios.tcsetattr(sys.stdin, termios.TCSANOW, settings)
print("\nTERMINAL_DONE", flush=True)
'''

for model in ("opus", "gpt-6.1-sol"):
    replies = []
    with flow(replies, model=model, config=PLAIN if model != "opus" else "") as f:
        (f.cwd / "interactive.py").write_text(program)
        secret = secrets.token_hex(24).encode()
        secret_file = f.cwd / "secret.txt"
        secret_file.write_bytes(secret + b"\n")
        if model == "opus":
            calls = [
                ("start", "mcp__razor__terminal", {"command": "python3 interactive.py", "pattern": "Name:"}),
                ("text", "mcp__razor__terminal", {"job": "job-1", "text": "Razor", "timeout_ms": 0}),
                ("key", "mcp__razor__terminal", {"job": "job-1", "key": "Enter", "pattern": "Secret:"}),
                ("secret", "mcp__razor__terminal", {"job": "job-1", "secret_file": str(secret_file)}),
            ]
            replies.extend(reply((id, name, args)) for id, name, args in calls)
            replies.append(reply(text="flow done"))
            prompts = {"start": "Name:", "key": "Secret:", "secret": "TERMINAL_DONE"}
        else:
            calls = [
                ("start", "bash", {"command": "python3 interactive.py", "pty": True, "wait_ms": 0}),
                ("prompt", "wait", {"job": 1, "pattern": "Name:", "timeout_ms": 10000}),
                ("text", "send", {"job": 1, "text": "Razor"}),
                ("key", "send", {"job": 1, "key": "Enter"}),
                ("hidden", "wait", {"job": 1, "pattern": "Secret:", "timeout_ms": 10000}),
                ("secret", "send", {"job": 1, "secret_file": str(secret_file), "timeout_ms": 10000}),
            ]
            replies.extend(response([call(name, json.dumps(args), id)]) for id, name, args in calls)
            replies.append(done())
            prompts = {"prompt": "Name:", "hidden": "Secret:", "secret": "TERMINAL_DONE"}
        result = f.run("--print=json", "Drive the interactive program to its done marker.")
        assert result.returncode == 0, result.stderr
        answer = json.loads(result.stdout)
        assert answer["message"] == "flow done", answer
        entries = list(f.record(answer["session"]))
        outputs = [e for e in entries if e["kind"] == "result"]
        assert [e["call"] for e in outputs] == [id for id, _, _ in calls], outputs
        assert all(e["outcome"] == "success" for e in outputs), outputs
        for index, output in enumerate(outputs, 2):
            if output["call"] not in prompts:
                continue
            screen = output["output"] if model == "opus" else json.loads(output["output"])["output"]
            # The stale text was erased by terminal control codes, not merely appended as bytes.
            assert prompts[output["call"]] in screen and "OVERWRITTEN" not in screen and "\x1b" not in screen, screen
            if model == "opus":
                assert "cursor" in output["output"], output
            else:
                assert len(json.loads(output["output"])["cursor"]) == 2, output
            request = json.loads((f.requests / f"{index:03}.body").read_bytes())
            if model == "opus":
                block = next(b for m in request["messages"] for b in m["content"]
                             if isinstance(b, dict) and b.get("type") == "tool_result"
                             and b["tool_use_id"] == output["call"])
                shown = block["content"]
                if isinstance(shown, list):
                    shown = "\n".join(b["text"] for b in shown)
            else:
                block = next(i for i in request["input"] if i.get("type") == "function_call_output"
                             and i["call_id"] == output["call"])
                shown = json.loads(block["output"])["output"]
            assert shown == screen, (shown, screen)
        jobs = [e for e in entries if e["kind"] == "job"]
        assert [(e["event"], e.get("end")) for e in jobs] == [("started", None), ("ended", {"exit": 0})], jobs
        directory = f.home / ".razor/sessions" / answer["session"]
        files = [p for p in directory.rglob("*") if p.is_file()]
        assert files and (directory / "record.jsonl") in files, files
        paths = []
        for file in files:
            body = file.read_bytes()
            assert secret not in body, f"secret leaked into {file.relative_to(directory)}"
            if str(secret_file).encode() in body:
                paths.append(file)
        assert directory / "record.jsonl" in paths, "the secret_file path never reached the record"
