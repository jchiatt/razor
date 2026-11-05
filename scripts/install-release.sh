#!/bin/sh
# Installs Razor, its bridge and needed Node in ${XDG_DATA_HOME:-$HOME/.local/share}/razor; links ~/.local/bin/razor.
set -eu
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) platform=linux-x64; node_sha256=a2e703725d8683be86bb5da967bf8272f4518bdaf10f21389e2b2c9eaeae8c8a ;;
  Linux-aarch64) platform=linux-arm64; node_sha256=d415eeea90a2fdb60c66dd386b258acbfc4d1fa4720a8df5dea7369fbdbcddee ;;
  Darwin-arm64) platform=darwin-arm64; node_sha256=2c12913cba67af77ded8a399df3fd91c2e7f8628c7079da40bb9ff33bf00dfc0 ;;
  *) echo 'Unsupported platform; use Linux x86_64 or aarch64, or macOS arm64.' >&2; exit 1 ;;
esac
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d ' ' -f 1; }
# Executables of the installing user's processes, resolved independently of argv or current.
executables() {
  case "$platform" in
    linux-*)
      [ -e /proc/self/exe ] || return 1
      for process in /proc/[0-9]*; do
        [ -O "$process" ] && exe=$(readlink "$process/exe" 2>/dev/null) || continue
        printf 'n%s\n' "$exe"
      done ;;
    darwin-*)
      /usr/sbin/lsof -a -u "$(id -u)" -d txt -Fn ;;
  esac
}
root=${XDG_DATA_HOME:-$HOME/.local/share}/razor
target=$root/$RAZOR_VERSION
if [ ! -d "$target" ]; then
  mkdir -p "$root"
  stage=$(mktemp -d "$root/.install.XXXXXX")
  trap 'rm -rf "$stage"' EXIT HUP INT TERM
  archive=razor-$RAZOR_VERSION-$platform.tar.gz
  curl -fsSL "$RAZOR_BASE_URL/$archive" -o "$stage/archive"
  curl -fsSL "$RAZOR_BASE_URL/$archive.sha256" -o "$stage/checksum"
  [ "$(sha256 "$stage/archive")" = "$(cut -d ' ' -f 1 "$stage/checksum")" ] || { echo 'Release checksum mismatch; download again from the release URL.' >&2; exit 1; }
  mkdir "$stage/bundle"
  tar -xzf "$stage/archive" -C "$stage/bundle"
  if ! command -v node >/dev/null || ! node -e 'const [a,b]=process.versions.node.split(".").map(Number);process.exit(a>22||a===22&&b>=18?0:1)' || ! command -v npm >/dev/null; then
    node_archive=node-v22.18.0-$platform.tar.gz
    curl -fsSL "https://nodejs.org/dist/v22.18.0/$node_archive" -o "$stage/node.tar.gz"
    [ "$(sha256 "$stage/node.tar.gz")" = "$node_sha256" ] || { echo 'Node checksum mismatch; download again from nodejs.org.' >&2; exit 1; }
    mkdir "$stage/bundle/node"
    tar -xzf "$stage/node.tar.gz" -C "$stage/bundle/node" --strip-components=1
    PATH="$stage/bundle/node/bin:$PATH"; export PATH
  fi
  (cd "$stage/bundle/bridge" && npm ci --omit=dev --no-audit --no-fund --no-update-notifier --loglevel=error) >&2
  mv "$stage/bundle" "$target"
fi
if [ "${RAZOR_SELECT:-1}" = 1 ]; then
  previous=$(readlink "$root/current" 2>/dev/null || echo "$RAZOR_VERSION")
  ln -sfn "$RAZOR_VERSION" "$root/current"
  mkdir -p "$HOME/.local/bin"
  first=0
  if [ ! -e "$HOME/.local/bin/razor" ] && [ ! -L "$HOME/.local/bin/razor" ]; then
    ln -s "$root/current/razor" "$HOME/.local/bin/razor"
    first=1
  fi
  if live=$(executables 2>/dev/null); then
    find "$root" -mindepth 1 -maxdepth 1 -type d -mtime +0 ! -path "$target" ! -path "$root/$previous" ! -name Razor.app | while IFS= read -r bundle; do
      printf '%s\n' "$live" | grep -Fq "n$(cd "$bundle" && pwd -P)/" || rm -rf "$bundle"
    done
  else
    echo 'Skipping old Razor bundles: live executables could not be inspected.' >&2
  fi
  if [ "$first" = 1 ]; then
    # Print commands for the calling shell without expanding them here.
    # shellcheck disable=SC2016
    case ":$PATH:" in
      *":$HOME/.local/bin:"*) ;;
      *) case "${SHELL:-}" in
           */fish) echo 'To put razor on PATH, run: fish_add_path "$HOME/.local/bin"' >&2 ;;
           *) echo 'To put razor on PATH in sh/bash/zsh, run: export PATH="$HOME/.local/bin:$PATH"' >&2 ;;
         esac ;;
    esac
    if [ -t 1 ] && [ -t 2 ] && ( : </dev/tty ) 2>/dev/null; then
      "$HOME/.local/bin/razor" setup </dev/tty
    else
      quoted=$(printf '%s' "$HOME/.local/bin/razor" | sed "s/'/'\\\\''/g")
      printf "Run '%s' setup in a terminal.\n" "$quoted" >&2
    fi
  else
    echo 'Run razor restart to use the new version.' >&2
  fi
fi
printf '%s\n' "$target/razor"
