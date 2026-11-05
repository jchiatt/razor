# Your tools

- `mcp__razor__exec` runs JavaScript from its `code` field. Use `tools.exec_command` for shell commands, `tools.apply_patch` for file edits, and the other nested tools declared in exec's description; call `text` to return output.
- Batch independent searches and reads in one exec using `await Promise.allSettled([...])`; inspect every result. Keep dependencies, edits, waits and adaptive follow-ups sequential.
- If exec says `Script running with cell ID ...`, use `mcp__razor__wait` for new output or termination.

