#!/usr/bin/env bash
#
# Install Magpie into the user's home directory. No root, no system paths —
# everything lands under ~/.local.
#
#   ./install.sh
#   ./install.sh --with-whisper    also build whisper.cpp, for transcripts
#   PREFIX=/usr/local sudo ./install.sh
#
set -euo pipefail

APP_ID="us.hagreli.Magpie"

WITH_WHISPER=""
for arg in "$@"; do
  [[ "$arg" == "--with-whisper" ]] && WITH_WHISPER=1
done

PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
DATA_DIR="$PREFIX/share"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here"

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m warning:\033[0m %s\n' "$*" >&2; }

say "Building (release)"
cargo build --release --locked

say "Installing to $PREFIX"
install -Dm755 target/release/magpie "$BIN_DIR/magpie"
install -Dm644 "data/$APP_ID.desktop" "$DATA_DIR/applications/$APP_ID.desktop"
install -Dm644 "data/$APP_ID.metainfo.xml" "$DATA_DIR/metainfo/$APP_ID.metainfo.xml"
install -Dm644 "data/icons/hicolor/scalable/apps/$APP_ID.svg" \
  "$DATA_DIR/icons/hicolor/scalable/apps/$APP_ID.svg"
install -Dm644 "data/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg" \
  "$DATA_DIR/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg"

# The desktop file declares DBusActivatable, so GNOME needs a matching D-Bus
# service file to launch the app on demand — from the dock's context menu, for
# instance.
install -Dm644 /dev/stdin "$DATA_DIR/dbus-1/services/$APP_ID.service" <<EOF
[D-BUS Service]
Name=$APP_ID
Exec=$BIN_DIR/magpie --gapplication-service
EOF

if command -v gtk4-update-icon-cache >/dev/null; then
  gtk4-update-icon-cache -qtf "$DATA_DIR/icons/hicolor" 2>/dev/null || true
elif command -v gtk-update-icon-cache >/dev/null; then
  gtk-update-icon-cache -qtf "$DATA_DIR/icons/hicolor" 2>/dev/null || true
fi
if command -v update-desktop-database >/dev/null; then
  update-desktop-database -q "$DATA_DIR/applications" 2>/dev/null || true
fi

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) warn "$BIN_DIR is not on your PATH; add it to run 'magpie' from a terminal" ;;
esac

# Magpie runs these rather than shipping them, so a missing one is worth saying
# now instead of at the moment a download fails.
#
# The yt-dlp advice names whichever installer is already here, matching what the
# Tools page in Preferences will say.
#
# Two things the obvious command gets wrong. `uvx` runs a tool without leaving
# anything on PATH, so it must be `uv tool install`. And the PyPI package on its
# own omits `yt-dlp-ejs`, the challenge-solver scripts YouTube needs, so it must
# be the `[default]` group — without it yt-dlp warns about missing formats however
# many JavaScript engines are installed.
if command -v uv >/dev/null; then
  ytdlp_hint='uv tool install "yt-dlp[default]"'
elif command -v pipx >/dev/null; then
  ytdlp_hint='pipx install "yt-dlp[default]"'
else
  ytdlp_hint="sudo apt install yt-dlp — though the packaged version is often too old for YouTube; installing uv gets a current one"
fi

# YouTube extraction needs a JavaScript engine. Deno is the one yt-dlp recommends
# and the only one it enables without being told to; its own installer puts it in
# ~/.deno/bin, which Magpie searches.
if command -v snap >/dev/null; then
  js_hint="sudo snap install deno"
else
  js_hint="sudo apt install nodejs — or Deno from https://docs.deno.com/runtime/getting_started/installation/"
fi

echo
missing=()
command -v yt-dlp >/dev/null || missing+=("yt-dlp — required. $ytdlp_hint")
command -v ffmpeg >/dev/null || missing+=("ffmpeg — needed to merge high quality video and convert audio. sudo apt install ffmpeg")
# Checked in yt-dlp's own order of preference, and including the directories the
# engines' installers use, since those are reached by editing the shell profile
# and Magpie is not always launched from a shell.
if ! command -v deno >/dev/null && ! command -v node >/dev/null \
  && ! command -v quickjs >/dev/null && ! command -v bun >/dev/null \
  && [[ ! -x "$HOME/.deno/bin/deno" ]] && [[ ! -x "$HOME/.bun/bin/bun" ]]; then
  missing+=("a JavaScript engine — YouTube needs one to reveal every format. $js_hint")
fi
if [[ -n "$WITH_WHISPER" ]]; then
  echo
  say "Building whisper.cpp for transcripts"
  PREFIX="$PREFIX" packaging/build-whisper.sh
elif ! command -v whisper-cli >/dev/null && ! command -v whisper-cpp >/dev/null; then
  missing+=("whisper.cpp — optional, only for transcripts. Run ./install.sh --with-whisper")
fi

if (( ${#missing[@]} )); then
  say "Magpie also uses these, and did not find them:"
  for line in "${missing[@]}"; do printf '    %s\n' "$line"; done
  echo
  say "Preferences → Tools says the same, with a button to install the ones it can"
  say "and a Copy button for the ones needing a terminal."
else
  say "Found everything Magpie uses. Nothing else to set up."
fi

say "Installed. Downloads go to your Downloads folder; the list lives in"
say "~/.local/share/magpie/library.json."
