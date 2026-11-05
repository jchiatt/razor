# Recorded response bodies

Flow 29's `web-*-fetch.json` files are successful provider bodies from 2026-10-28 live probes of https://example.com/, byte for byte except that their request, session and index identifiers are replaced with zeros and Exa's cost field is removed. Its search, answer and failure bodies are synthetic vendor examples labelled with their documentation URLs in `29-web.py`; none is a live negative capture.

These are authored Messages API SSE bodies, pinned to the shape accepted by Claude Agent SDK 0.3.312 and its bundled CLI 2.1.312. They are scripted model replies, not captured live model answers. The stand-in sends the file bytes unchanged; it neither expands objects nor derives replies from incoming requests.

`responses-tool.sse` and `responses-done.sse` are for the continuation proof and flow 28: the reasoning item's added and completed `encrypted_content` deliberately differ, the `exec` call uses raw custom input, and the final assistant message carries `phase`. These match the current Responses event reference; they have not run through Razor's GPT loop yet.

Each flow lists its replies in request order. `high-usage.sse` reports 190,000 input tokens so flow 17 can cross the threshold after its warm-up turn. `recall.sse` asks for entry 2, the first person's input. Flows 6 and 7 use the Claude route's own names (`Bash`, `TaskStop`, `mcp__razor__terminal`; docs/design/claude.md), and flow 7's last two replies answer its turn and the second job's notice.

Flows 21 to 23 (residents) use the `resident-*` replies: one tool call each, on the Claude route's names (`Bash`, `mcp__razor__message`), with `done.sse` ending each turn. Flow 24 (memory) writes its replies inline.
