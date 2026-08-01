#!/usr/bin/env bash
#
# Install sherpa-onnx's speaker diarizer where Magpie will find it.
#
#   packaging/fetch-diarizer.sh                  install to ~/.local/lib/magpie
#   PREFIX=/usr/local sudo packaging/fetch-diarizer.sh
#   packaging/fetch-diarizer.sh --staged DIR     install into DIR/usr/lib/magpie
#   SHERPA_VERSION=v1.13.4 packaging/fetch-diarizer.sh
#
# Fetch rather than build, which is the difference between this and
# build-whisper.sh. Upstream publishes a prebuilt Linux binary for every release;
# building sherpa-onnx from source means CMake, a C++ toolchain and ONNX Runtime,
# for a result identical to the one they already compiled. whisper.cpp gets built
# because nobody publishes a usable Linux `whisper-cli`.
#
# The version is pinned for the same reason whisper's is: this is local
# inference against pinned model files, so there is no upstream service to fall
# out of step with, and an unpinned download is a build that breaks on a day
# nobody changed anything.
#
set -euo pipefail

SHERPA_VERSION="${SHERPA_VERSION:-v1.13.4}"
ARCHIVE_BASE="https://github.com/k2-fsa/sherpa-onnx/releases/download"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$here"

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

STAGED=""
if [[ "${1:-}" == "--staged" ]]; then
  [[ -n "${2:-}" ]] || die "--staged needs a directory"
  STAGED="$2"
fi

for tool in curl tar; do
  command -v "$tool" >/dev/null || die "$tool is not installed"
done

# The two architectures are not named to a pattern: x86-64's CPU build is
# `-shared`, arm64's is `-shared-cpu`, because arm64 also publishes GPU variants
# that x86-64 spells differently again. Derived rather than assumed — a guessed
# name here is a 404 on somebody else's machine.
case "$(uname -m)" in
  x86_64)  ARCHIVE_ARCH="x64-shared" ;;
  aarch64) ARCHIVE_ARCH="aarch64-shared-cpu" ;;
  *) die "no prebuilt sherpa-onnx for $(uname -m) — see https://github.com/k2-fsa/sherpa-onnx" ;;
esac

# The shared build, not the static one. Static would be a single file with
# nothing to place correctly, but the download is 385 MB against 28 MB for the
# whole of one binary we want.
ARCHIVE="sherpa-onnx-$SHERPA_VERSION-linux-$ARCHIVE_ARCH.tar.bz2"
URL="$ARCHIVE_BASE/$SHERPA_VERSION/$ARCHIVE"

BUILD_DIR="$here/.diarizer-build"
mkdir -p "$BUILD_DIR"

if [[ ! -f "$BUILD_DIR/$ARCHIVE" ]]; then
  say "Fetching sherpa-onnx $SHERPA_VERSION ($ARCH)"
  curl -fL --progress-bar -o "$BUILD_DIR/$ARCHIVE.part" "$URL" ||
    die "could not download $URL"
  mv "$BUILD_DIR/$ARCHIVE.part" "$BUILD_DIR/$ARCHIVE"
fi

say "Unpacking"
rm -rf "$BUILD_DIR/tree"
mkdir -p "$BUILD_DIR/tree"
tar -xjf "$BUILD_DIR/$ARCHIVE" -C "$BUILD_DIR/tree" --strip-components=1

BINARY="$BUILD_DIR/tree/bin/sherpa-onnx-offline-speaker-diarization"
[[ -f "$BINARY" ]] || die "the archive did not contain the diarizer"

if [[ -n "$STAGED" ]]; then
  DEST="$STAGED/usr/lib/magpie"
else
  PREFIX="${PREFIX:-$HOME/.local}"
  DEST="$PREFIX/lib/magpie"
fi

# `bin/` beside `lib/`, and not flattened, because the binary is linked with
# RPATH=$ORIGIN/../lib. Put it anywhere else and it is found and then refuses to
# start for want of libonnxruntime.so. `model::tools::PRIVATE_DIRECTORIES` knows
# this shape.
install -Dm755 "$BINARY" "$DEST/bin/sherpa-onnx-offline-speaker-diarization"
for lib in "$BUILD_DIR/tree/lib/"*.so*; do
  [[ -f "$lib" ]] || continue
  install -Dm644 "$lib" "$DEST/lib/$(basename "$lib")"
done

# Prove it runs before declaring it installed — for a downloaded tarball the
# thing most likely to be wrong is a shared library that will not load, and that
# failure is invisible until the first transcript.
"$DEST/bin/sherpa-onnx-offline-speaker-diarization" --help >/dev/null 2>&1 ||
  die "the installed diarizer does not run"

say "Installed $DEST/bin/sherpa-onnx-offline-speaker-diarization"

if [[ -z "$STAGED" ]]; then
  echo
  say "Magpie will find it on the next launch, or press Check Again on"
  say "Preferences → Tools. Then download the two speaker models on the"
  say "Transcripts page — about 34 MB, and nothing works without them."
fi
