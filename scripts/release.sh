#!/usr/bin/env bash
# Build this platform's release archive, its checksum and the versioned installer.
set -euo pipefail
cd "$(dirname "$0")/.."
version=${1:?version required} out=${2:?output directory required} base=${3:?base URL required}
case "$version" in *[!a-zA-Z0-9._-]*|'') echo 'Invalid version' >&2; exit 1;; esac
case "$base" in *[!a-zA-Z0-9._:/~-]*|'') echo 'Invalid base URL' >&2; exit 1;; esac
case "${RAZOR_PLATFORM:-$(uname -s)-$(uname -m)}" in
  Linux-x86_64|linux-x64) platform=linux-x64; target=x86_64-unknown-linux-gnu; browser=2a89295b62ddd497986a9a80b52e1dbe4c1f6bd68ffde67eaae82eec2206af61; rg=x86_64-unknown-linux-musl; rg_sha=33e15bcf1624b25cdd2a55813a47a2f95dbe126268203e76aa6a585d1e7b149c; jq=linux-amd64; jq_sha=b1c22172dd303f3be49e935aa56aa48a8b7a46e0bc838b4997d3bb451495870f ;;
  Linux-aarch64|linux-arm64) platform=linux-arm64; target=aarch64-unknown-linux-gnu; browser=f0b3bdaa40abf2a5111ec08e6d242110aaf56045ef5108cbc633bacc8e53ca17; rg=aarch64-unknown-linux-musl; rg_sha=800b1e7206afe799dfb5a6901f23147cfaabe0e52210538100f61e86e1740915; jq=linux-arm64; jq_sha=8b85c817833814ddca00a144c33705546355afccf0cf39b188f3cdb48b852309 ;;
  Darwin-arm64|darwin-arm64) platform=darwin-arm64; target=aarch64-apple-darwin; browser=63b0a97b85a54b4c9b2871318af74104c29b80abd507b53a30316884e062854b; rg=aarch64-apple-darwin; rg_sha=3750b2e93f37e0c692657da574d7019a101c0084da05a790c83fd335bad973e4; jq=macos-arm64; jq_sha=2d75340ba57a4b4b4c8708a21c2dc8e958a48aaa8bba13b27f77f6e4c0eca07e ;;
  *) echo 'Unsupported platform; use Linux x86_64 or aarch64, or macOS arm64.' >&2; exit 1 ;;
esac
stage=$(mktemp -d)
trap 'rm -rf "$stage" "$stage.zip" "$stage.rg"' EXIT
mkdir -p "$out" "$stage/tools"
if [ -n "${RAZOR_BINARY:-}" ]; then binary=$RAZOR_BINARY
elif [[ $platform = linux-* ]]; then
  docker run --rm -v "$PWD:/source:ro" -v "$stage:/output" -w /source \
    -e CARGO_TARGET_DIR=/tmp/target rust:1.98-bookworm bash -ec '
      if [ "$1" = linux-arm64 ]; then
        apt-get update && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross cmake clang
        rustup target add aarch64-unknown-linux-gnu
        export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
        cargo build --release --locked --target aarch64-unknown-linux-gnu
        cp /tmp/target/aarch64-unknown-linux-gnu/release/razor /output/razor
      else
        cargo build --release --locked
        cp /tmp/target/release/razor /output/razor
      fi
    ' _ "$platform"
  binary=$stage/razor
else
  cargo build --release --locked --quiet
  target_dir=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
  binary=$target_dir/release/razor
fi
[ "$binary" = "$stage/razor" ] || cp "$binary" "$stage/razor"
python3 scripts/notices.py "$target" > "$stage/THIRD-PARTY-NOTICES.md"
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi; }
# Downloads $1 to $2, which must match the pinned SHA-256 $3.
fetch() { curl -fsSL "$1" -o "$2" && [ "$(sha256 "$2" | cut -d ' ' -f 1)" = "$3" ] || { echo "Download or checksum failed: $1" >&2; exit 1; }; }
fetch "https://github.com/vercel-labs/agent-browser/releases/download/v0.41.1/agent-browser-$platform" "$stage/agent-browser" "$browser"
# ripgrep and jq for agents' shells, in tools/, which serve puts last on PATH so the person's own copies win.
fetch "https://github.com/BurntSushi/ripgrep/releases/download/15.2.0/ripgrep-15.2.0-$rg.tar.gz" "$stage.rg" "$rg_sha"
tar -xzf "$stage.rg" -C "$stage/tools" --strip-components=1 "ripgrep-15.2.0-$rg/rg"
fetch "https://github.com/jqlang/jq/releases/download/jq-1.8.2/jq-$jq" "$stage/tools/jq" "$jq_sha"
chmod +x "$stage/agent-browser" "$stage/tools/jq"
# Razor.app, the macOS service's first program. A Developer ID (RAZOR_SIGN_IDENTITY names one)
# keeps a permission granted to Razor across releases; an ad hoc signature, only across the same
# bytes. RAZOR_NOTARY_PROFILE names a notarytool keychain profile (xcrun notarytool
# store-credentials) to have Apple notarize the signed programs.
if [[ $platform = darwin-* ]]; then
  app=$stage/Razor.app/Contents
  mkdir -p "$app/MacOS" && cp scripts/Info.plist "$app/" && cp "$(dirname "$binary")/razor-launcher" "$app/MacOS/Razor"
  if [ -n "${RAZOR_SIGN_IDENTITY:-}" ]; then
    codesign --force --options runtime --timestamp --sign "$RAZOR_SIGN_IDENTITY" "$stage/razor" "$stage/agent-browser" "$stage/tools/rg" "$stage/tools/jq" "$stage/Razor.app"
  else codesign --force --sign - "$stage/Razor.app"; fi
  if [ -n "${RAZOR_NOTARY_PROFILE:-}" ]; then
    ditto -c -k "$stage" "$stage.zip"
    xcrun notarytool submit "$stage.zip" --keychain-profile "$RAZOR_NOTARY_PROFILE" --wait
    xcrun stapler staple "$stage/Razor.app"
  fi
fi
archive="razor-$version-$platform.tar.gz"
# No extended attributes, and on macOS no ._ metadata files: an archive holds only the release's files.
# shellcheck disable=SC2046 # release-files.txt holds plain paths, one a line.
COPYFILE_DISABLE=1 tar --no-xattrs -czf "$out/$archive" -C "$stage" . -C "$PWD" $(grep -v '^#' scripts/release-files.txt)
(cd "$out" && sha256 "$archive" > "$archive.sha256")
{ head -n 2 scripts/install-release.sh; printf 'RAZOR_BASE_URL=%s\nRAZOR_VERSION=%s\n' "\${RAZOR_BASE_URL:-$base}" "\${RAZOR_VERSION:-$version}"; tail -n +3 scripts/install-release.sh; } > "$out/install.sh"
chmod +x "$out/install.sh"
