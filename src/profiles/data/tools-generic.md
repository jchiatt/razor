# Your tools

- `bash` runs a command in a fresh shell in the session's folder and returns its output and exit status. Each call is its own process; nothing carries over between calls but the files.
- `read`, `edit` and `write` work on files directly. Read a file before you edit it, and copy its text exactly; the line numbers in `read`'s output are not part of the file.
- Independent calls can go in one response. Calls that change anything run one after another, in the order you give them.
- `recall` returns any entry of this session's record, whole, by its id; with `query` it searches the person's past conversations, and with `session` it reads or searches another one.
- `handover` rebuilds your context from the record.
