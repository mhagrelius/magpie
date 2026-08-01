#!/usr/bin/env bash
#
# Remove what install.sh installed. Never touches a download: the files are in a
# folder you chose, and they are not Magpie's to delete.
#
set -euo pipefail

APP_ID="us.hagreli.Magpie"
PREFIX="${PREFIX:-$HOME/.local}"
DATA_DIR="$PREFIX/share"

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

say "Removing Magpie from $PREFIX"
rm -f "$PREFIX/bin/magpie"
rm -f "$DATA_DIR/applications/$APP_ID.desktop"
rm -f "$DATA_DIR/metainfo/$APP_ID.metainfo.xml"
rm -f "$DATA_DIR/icons/hicolor/scalable/apps/$APP_ID.svg"
rm -f "$DATA_DIR/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg"
rm -f "$DATA_DIR/dbus-1/services/$APP_ID.service"

if command -v update-desktop-database >/dev/null; then
  update-desktop-database -q "$DATA_DIR/applications" 2>/dev/null || true
fi

echo
say "Done. Your downloads were left alone, and so were:"
say "  ~/.config/magpie          preferences"
say "  ~/.local/share/magpie     the download list, and any whisper models"
say "  ~/.cache/magpie           thumbnails and scratch files"
echo
say "A whisper model is up to 1.5 GB, so it is worth checking before removing:"
say "  du -sh ~/.local/share/magpie && rm -r ~/.config/magpie ~/.local/share/magpie ~/.cache/magpie"
