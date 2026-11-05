# Uninstall

Removing Razor takes three steps: stop and remove the background service (the server that `razor install` registered with systemd or launchd), remove the program, and, if you want, remove your data. Razor has no uninstall command; these are the files it made.

## 1. Stop the server and remove the service

If you ran `razor install`, remove the service, which also stops the server. The service is named after the home's folder without its leading dot: `razor` for `~/.razor`, `razor-work` for a `RAZOR_HOME` of `~/.razor-work`; use that name below for each home you installed. On Linux its file is under `${XDG_CONFIG_HOME:-~/.config}`.

```sh
# Linux
systemctl --user disable --now razor.service
rm ~/.config/systemd/user/razor.service
systemctl --user daemon-reload
```

```sh
# macOS
launchctl bootout "gui/$(id -u)/razor"
rm ~/Library/LaunchAgents/razor.plist
```

If the terminal app started the server in the background instead, stop it:

```sh
razor stop
```

Stopping the server leaves running the commands its agents started, so a restarted server can take them back. Before removing your data, check for any still running with `ps -u "$USER" -o pid,command`.

## 2. Remove the program

Installed with the one-line installer or `scripts/install.sh`:

```sh
rm ~/.local/bin/razor
rm -rf "${XDG_DATA_HOME:-$HOME/.local/share}/razor"
```

That folder holds every installed version, its agent-browser, ripgrep and jq, the private Node the installer downloaded if yours was missing, and on macOS Razor.app. On macOS, also take back any permissions you gave Razor with `tccutil reset All com.treygoff.razor.server`.

## 3. Remove your data

Everything Razor keeps about you is in its home, `~/.razor` (or wherever `RAZOR_HOME` points): settings, keys, the ChatGPT sign-in, every session's record, memory, tasks, and the worktrees of helpers that were never settled. Deleting it is permanent.

```sh
rm -rf ~/.razor
```

Helpers' worktrees lived inside that folder, and git still lists them in each repository they came from, along with any `child/…` branches that were discarded rather than merged. In each such repository:

```sh
git worktree prune
git branch --list 'child/*'
git branch -D child/20261101-203800-b659    # for each branch you no longer want
```

## What other programs keep

These belong to other programs that Razor ran for you; remove them only if nothing else uses them.

| Program | What it keeps | Where |
| --- | --- | --- |
| Claude Code | Your Claude sign-in, shared with Claude Code itself if you use it | `~/.claude` and `~/.claude.json`, and the login keychain on macOS |
| agent-browser | Its settings and saved browser state, and the Chrome that `razor setup` downloaded | `~/.agent-browser`; `agent-browser doctor` shows where Chrome is |
