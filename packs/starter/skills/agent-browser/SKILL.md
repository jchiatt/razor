---
name: agent-browser
description: A real browser from the shell. Open pages, read them as snapshots with element refs, click and fill by ref, wait for content, take screenshots, run page JavaScript; for forms, logins and web apps.
---

# agent-browser

`agent-browser` is a command-line browser over Chromium, installed with Razor. The browser keeps running between commands, so each command carries on from the last. Refs such as `@e3` come from snapshots. To read a page's text, the web tools are cheaper; use the browser when a task needs clicks, forms, a login or a screenshot.

## Your own browser

Without a name, every agent on this machine shares one browser and can move another's page. Give yours a name on every command; each shell call is its own process, so an `export` does not carry over:

```bash
agent-browser --session "$RAZOR_SESSION" open https://example.com
```

The commands below leave the flag out for short lines; keep it in yours. `agent-browser --session "$RAZOR_SESSION" close` ends your browser when you are done.

## Open and look

```bash
agent-browser open <url>
agent-browser snapshot -i              # interactive elements only, with refs (preferred)
agent-browser snapshot -i -c           # compact: no empty structural nodes
agent-browser snapshot -s "#main"      # scope to a CSS selector
agent-browser snapshot -i --delta      # full state once, then only what changed
```

Take a fresh snapshot after navigation or any page change; "Ref not found" means the page changed since yours.

## Read text the snapshot leaves out

```bash
agent-browser read                     # readable text of the current page
agent-browser get text @e5             # visible text of one element
agent-browser snapshot                 # full tree, prose included (verbose)
```

## Act

```bash
agent-browser click @e3
agent-browser fill @e2 "hello"         # clear, then type
agent-browser select @e4 "Option"      # by value or visible label
agent-browser check @e6
agent-browser press Enter
agent-browser find label "Email" fill "user@test.com"   # no snapshot needed
```

If a click does nothing, something covers the element (a cookie banner, a modal): snapshot, dismiss it, then click again.

## Wait for content

```bash
agent-browser wait --text "Success"    # until the text appears on the page
agent-browser wait @e1                 # until an element appears
agent-browser wait --fn "document.querySelectorAll('tr').length > 1"
```

Agents fail more often from bad waits than from bad selectors: wait for what you expect, not a fixed time.

## Screenshots

```bash
agent-browser screenshot page.png      # the viewport
agent-browser screenshot --full page.png
```

A screenshot is an image file; open it with a tool that shows you images.

## Navigate and tabs

```bash
agent-browser back
agent-browser tab                      # list tabs
agent-browser tab t2                   # switch tab
```

## JavaScript

```bash
agent-browser eval "document.title"
agent-browser eval --stdin <<'JS'
Array.from(document.querySelectorAll("table tr")).map(r => r.innerText)
JS
agent-browser eval "(async () => (await fetch('/next')).status)()"   # promises are awaited
agent-browser batch "open https://example.com/a" "get text main" "back"   # several commands in one call
```

## More

`agent-browser <command> --help` shows one command's options, and `agent-browser skills get core --full` prints agent-browser's own guide for the installed version. When the browser will not start or a command fails oddly, `agent-browser doctor --offline` says what is wrong, with a test launch of the browser. If `agent-browser` is not found, it is not installed on this server's PATH.

The commands follow agent-browser's own documentation (github.com/vercel-labs/agent-browser, Apache-2.0, copyright 2025 Vercel Inc.); this guide is written for Razor.
