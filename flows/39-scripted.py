"""Configured scripts reach both model paths; code-mode apply_patch edits the named file.
Claude's plain Edit is native to the SDK, not Razor's patch action (docs/design/tools.md).
"""
import json

from claude import reply
from driver import flow
from responses import PLAIN, call, done, response
import server

patch = "*** Begin Patch\n*** Update File: note\n@@\n-before\n+after\n*** End Patch"
source = ('text(await tools.flow_script({text:"seed"}));'
          f'text(await tools.apply_patch({json.dumps(patch)}));')
script = "python3 -c 'import json,sys; print(json.load(sys.stdin)[\"text\"].upper())'"
config = (f"[tools.flow_script]\ncommand = {json.dumps(script)}\n"
          'description = "Uppercase the input text."\n'
          'input = {type = "object", properties = {text = {type = "string"}}, required = ["text"]}\n')
for model, extra, replies in [
    ("gpt-6.1-sol", "", [response([{"type": "custom_tool_call", "id": "ct_script",
        "call_id": "call_script", "name": "exec", "input": source}]), done()]),
    ("opus", "[profiles.claude]\ncode_mode = true\n",
        [reply((None, "mcp__razor__exec", {"code": source})), reply(text="flow done")]),
]:
    with flow(replies, model=model, config=extra + config) as f:
        (f.cwd / "note").write_text("before\n")
        result = f.run("--print=json", "Call the script, then apply the patch.")
        assert result.returncode == 0, result.stderr
        entries = list(f.record(json.loads(result.stdout)["session"]))
        outputs = [e for e in entries if e["kind"] == "result"]
        assert outputs and all(e["outcome"] == "success" for e in outputs), outputs
        if model == "opus":
            # The CLI cuts an MCP tool's description at 4,096 characters unless told otherwise.
            tools = json.loads((f.requests / "001.body").read_bytes())["tools"]
            exec_ = next(t for t in tools if t["name"] == "mcp__razor__exec")
            assert "flow_script(" in exec_["description"], "exec's description was cut short"
        second = json.loads((f.requests / "002.body").read_bytes())
        assert "SEED" in json.dumps(second), "the model did not receive the script's output"
        assert (f.cwd / "note").read_text() == "after\n", "patch did not change the file"

# A command that ran reports its exit to JavaScript even when the record marks it failed.
source = ('const first = await tools.exec_command({cmd:"exit 1"});'
          'text(first.end.exit);'
          'text(await tools.exec_command({cmd:"printf continued > continued"}));'
          'const job = await tools.exec_command({cmd:"read line; exit 7", tty:true, yield_time_ms:1});'
          'text((await tools.write_stdin({session_id:job.job, chars:"go\\n"})).end.exit);')
with flow([response([{"type":"custom_tool_call", "id":"ct_exit", "call_id":"call_exit",
                     "name":"exec", "input":source}]), done()], model="gpt-6.1-sol") as f:
    result = f.run("--print=json", "Read the exits and continue.")
    assert result.returncode == 0, result.stderr
    entries = list(f.record(json.loads(result.stdout)["session"]))
    outputs = {e["call"]:e for e in entries if e["kind"] == "result"}
    assert (f.cwd / "continued").exists(), outputs
    assert outputs["call_exit"]["outcome"] == "success", outputs
    assert outputs["call_exit:1"]["outcome"] == outputs["call_exit:4"]["outcome"] == "error", outputs
    assert "\n1\n" in outputs["call_exit"]["output"] and "\n7\n" in outputs["call_exit"]["output"], outputs

# A long cell keeps its opening read and final output inside its character cap.
source = '// @exec: {"max_output_tokens":100}\ntext("FIRST 🪶");text("🪶".repeat(3000));text("LAST 🪶");'
with flow([response([{"type":"custom_tool_call", "id":"ct_cap", "call_id":"call_cap",
                     "name":"exec", "input":source}]), done()], model="gpt-6.1-sol") as f:
    result = f.run("--print=json", "Read the long output.")
    assert result.returncode == 0, result.stderr
    output = next(e["output"] for e in f.record(json.loads(result.stdout)["session"])
                  if e["kind"] == "result" and e["call"] == "call_cap")
    assert "FIRST 🪶" in output and "LAST 🪶" in output, output
    assert "bytes omitted" in output and "Full output:" in output and "�" not in output, output
    assert len(output.split("\nFull output:")[0].split("\n",1)[1]) <= 400, output

# A configured tool whose name ends in "message" asks the person nothing, though its input looks
# like a question: only Razor's own `message` does.
config = (PLAIN + '[tools.archive_message]\ncommand = "cat > /dev/null"\ndescription = "Archive a message."\n'
          'input = {type = "object", properties = {type = {type = "string"}, text = {type = "string"}}}\n')
question = json.dumps({"type": "question", "text": "Archived question?"})
with flow([response([call("archive_message", question)]), done()], model="gpt-6.1-sol", config=config) as f:
    session = json.loads(f.run("--print=json", "Archive it.").stdout)["session"]
    assert [e["outcome"] for e in f.record(session) if e["kind"] == "result"] == ["success"]
    marks = json.loads(server.send(f, "GET", f"/sessions/{session}/marks").read())
    assert server.agents(f)[session]["asks"] == [] and "question" not in json.dumps(marks), marks
