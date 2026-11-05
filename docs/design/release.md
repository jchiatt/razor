# Release and install

```text
/// Build this checkout's archive and install it; --link also selects it.
scripts/install.sh [--link]
/// Build the current platform's archive, checksum, and versioned installer in DIR.
scripts/release.sh VERSION DIR BASE
/// Install the release named in the script; environment overrides select a different one.
/// On creating ~/.local/bin/razor in a terminal, run setup; otherwise print its next command.
/// Preserve stdout's installed binary path for callers that capture it.
curl -fsSL BASE/install.sh | RAZOR_BASE_URL=BASE RAZOR_VERSION=VERSION sh
```

1. Release builds use Debian 12 for Linux x86_64 and aarch64, and the Mac for arm64.
2. Each archive holds the binary, pinned native agent-browser, pinned ripgrep and jq in `tools/` (which `serve` puts last on its agents' `PATH`, so the person's own copies win), and at their source paths the files `scripts/release-files.txt` lists: bridge sources and lockfile, scripts/speech.py, scripts/notify.sh, packs/starter, README.md, SPEC.md, RULES.md and docs/, so agents can read the installed version's docs, and follow their links, offline; missing starter files or docs fail the build. A published archive is built in a checkout of the public release commit, so it carries exactly that commit's docs.
3. The release script records checksums for the archives and generates the installer with the chosen base and version.
4. The installer detects the platform, verifies the archive, and extracts it into a new version folder under the user's data directory.
5. It uses Node 22.18+ from PATH when available; otherwise it verifies and installs a pinned official Node archive inside that version folder.
6. It installs the bridge's npm dependencies at install time and selects the version only after those steps succeed.
7. It links `razor` from `~/.local/bin`, retains the selected and previous versions, folders modified within 24 hours, and any bundle a process owned by the installing user executes from; reruns skip the download and npm for an installed version. Executable paths come from that user's `/proc` entries on Linux and `/usr/sbin/lsof -a -u <uid> -d txt -Fn` on macOS, so a server or bundled Node started through `current` keeps its real bundle. A process-listing failure skips pruning with a sentence on stderr; unreadable executable links are ignored. Other users executing private home bundles, and servers run as system units with `User=`, are outside this promise (4 November 2026).
8. `scripts/install.sh` builds the local archive and invokes the same installer from a file URL; an option preserves the evaluation's unselected candidate.
9. Razor resolves its binary's folder once before starting the runtime, using it for the bridge and private Node; only `serve` adds it to PATH, and `install` writes the stable current/razor path for a release.
10. Only the install that creates `~/.local/bin/razor` enters setup. Print any needed PATH advice on stderr and run setup when stdout/stderr are terminals and `/dev/tty` opens; redirect only stdin there, preserving terminal output. Under `curl … | sh`, stdin is the script and answers come from the controlling terminal. Without it, print the exact shell-quoted `setup` command. Updates print `Run razor restart to use the new version.` An unselected install does neither. Stdout remains one binary path; setup's nonzero exit propagates, leaving the completed install available to rerun. Source `scripts/install.sh --link` follows the same first-link rule.

**Decision: print a PATH command without changing shell files.** If `~/.local/bin` is absent from PATH, print `export PATH="$HOME/.local/bin:$PATH"` for sh/bash/zsh, or `fish_add_path "$HOME/.local/bin"` for fish. For an unrecognized shell, label the sh command and retain the exact executable path. Appending to a profile requires choosing among login and interactive files and cannot update the calling shell. Setup needs no PATH change: serve already prepends its binary's folder. Requiring terminal output keeps captured installer calls noninteractive.

Check the HTTP installer in a clean Linux container and a scratch Mac home, including doctor and agent-browser in an agent shell. Exercise a piped script with a controlling terminal, no controlling terminal, captured stdout, interruption, updates, source installs and unselected installs.

Node on PATH avoids downloading a runtime for most developers; automatically installing pinned Node otherwise removes a first-run barrier at the cost of a roughly 40 MB download and maintaining the pinned checksum. Private Node is used only for the bridge, so it cannot shadow the user's Node in agent shells. Browser installation itself belongs to `razor setup`.

A source build without `--link` requests an unselected candidate from the common installer; installation must not remove existing versions in that mode.
