#!/usr/bin/env bash
#
# Build whisper.cpp's `whisper-cli` and install it where Magpie will find it.
#
#   packaging/build-whisper.sh                  build, install to ~/.local/bin
#   PREFIX=/usr/local sudo packaging/build-whisper.sh
#   packaging/build-whisper.sh --staged DIR     install into DIR/lib/magpie (for build-deb.sh)
#   WHISPER_VERSION=v1.9.1 packaging/build-whisper.sh
#
# Why this exists at all: transcripts need whisper.cpp, and there is no Ubuntu
# package for it. Without this script the feature is theoretical — it asks the
# user to build a C++ project before they can use a switch in a dialog, which
# nobody does. Unlike yt-dlp this is worth pinning and building: it is local
# inference with no network, so a fixed version does not rot the way a web
# scraper does.
#
# CPU only, deliberately. The GPU backends (Vulkan, CUDA) are considerably faster
# and considerably more ways to fail on a machine whose drivers are not what the
# build assumed. A binary that works everywhere beats one that is quick on the
# machine it was built on; `-DGGML_NATIVE=OFF` is part of the same promise, since
# a build tuned to this CPU's instruction set would crash with SIGILL on an older
# one.
#
set -euo pipefail

WHISPER_VERSION="${WHISPER_VERSION:-v1.9.1}"
WHISPER_REPO="https://github.com/ggml-org/whisper.cpp"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$here"

say()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

STAGED=""
if [[ "${1:-}" == "--staged" ]]; then
  [[ -n "${2:-}" ]] || die "--staged needs a directory"
  STAGED="$2"
fi

for tool in cmake git; do
  command -v "$tool" >/dev/null || die "$tool is not installed (sudo apt install cmake git build-essential)"
done

BUILD_DIR="$here/.whisper-build"
SRC="$BUILD_DIR/whisper.cpp"

if [[ -d "$SRC/.git" ]]; then
  say "Updating whisper.cpp to $WHISPER_VERSION"
  git -C "$SRC" fetch --depth 1 origin tag "$WHISPER_VERSION" --quiet
  git -C "$SRC" checkout --quiet "$WHISPER_VERSION"
else
  say "Fetching whisper.cpp $WHISPER_VERSION"
  mkdir -p "$BUILD_DIR"
  # A shallow clone of one tag: the full history is ~200 MB and none of it is
  # needed to build a release.
  git clone --depth 1 --branch "$WHISPER_VERSION" --quiet "$WHISPER_REPO" "$SRC"
fi

say "Building whisper-cli (CPU, portable)"
cmake -S "$SRC" -B "$SRC/build" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF \
  -DGGML_NATIVE=OFF \
  -DWHISPER_BUILD_TESTS=OFF \
  -DWHISPER_BUILD_EXAMPLES=ON \
  -DWHISPER_BUILD_SERVER=OFF \
  >/dev/null
cmake --build "$SRC/build" --config Release --target whisper-cli -j"$(nproc)" >/dev/null

BINARY="$(find "$SRC/build" -name whisper-cli -type f -perm -u+x | head -1)"
[[ -n "$BINARY" ]] || die "the build finished but produced no whisper-cli"

# Prove it runs before installing it. A binary that cannot print its own help is
# not one to put on someone's PATH.
"$BINARY" -h >/dev/null 2>&1 || die "the built whisper-cli does not run"

if [[ -n "$STAGED" ]]; then
  # A private directory rather than /usr/bin: this is Magpie's own copy, and it
  # must never shadow a whisper.cpp the user or the distribution installs later.
  # `model::tools::candidates` searches it last, after PATH.
  install -Dm755 "$BINARY" "$STAGED/usr/lib/magpie/whisper-cli"
  say "Staged at $STAGED/usr/lib/magpie/whisper-cli"
else
  PREFIX="${PREFIX:-$HOME/.local}"
  install -Dm755 "$BINARY" "$PREFIX/bin/whisper-cli"
  say "Installed $PREFIX/bin/whisper-cli"
  echo
  say "Magpie will find it on the next launch, or press Check Again on"
  say "Preferences → Tools. Then pick a model on the Transcripts page —"
  say "Small is the default and the one worth starting with."
fi
