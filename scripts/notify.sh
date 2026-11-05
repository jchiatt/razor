#!/bin/sh
# The starter pack's `notify` and `ask` program: its one argument becomes a desktop notification,
# through osascript on macOS or notify-send on Linux; where neither exists, as on a headless
# server, it exits quietly. It prints nothing, so an ask it carries waits for a client's answer.
if command -v osascript >/dev/null; then
  exec osascript -e 'on run argv' -e 'display notification (item 1 of argv) with title "Razor"' \
    -e 'end run' -- "$1" >/dev/null
elif command -v notify-send >/dev/null; then
  exec notify-send --app-name=Razor -- Razor "$1" >/dev/null
fi
