#!/usr/bin/env bash
#
# Build a .deb for the host architecture.
#
#   packaging/build-deb.sh                 build
#   packaging/build-deb.sh --install       build, then install it with apt
#   packaging/build-deb.sh --with-whisper  also build and include whisper-cli
#
# Uses only dpkg-deb, dpkg-shlibdeps and fakeroot — no debhelper, no debuild.
#
# `--with-whisper` compiles whisper.cpp and ships it in /usr/lib/magpie, because
# Ubuntu has no package for it and without one the transcript feature asks the user
# to build a C++ project first. It is off by default: it needs cmake and a
# compiler, adds a couple of minutes, and a package that quietly grows a bundled
# binary should do so because someone asked.
#
set -euo pipefail

APP_ID="us.hagreli.Magpie"
PKG="magpie"

WITH_WHISPER=""
for arg in "$@"; do
  [[ "$arg" == "--with-whisper" ]] && WITH_WHISPER=1
done

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$here"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
ARCH="$(dpkg-architecture -qDEB_HOST_ARCH)"
STAGE="$(mktemp -d)"
DIST="$here/dist"
trap 'rm -rf "$STAGE"' EXIT

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

say "Building $PKG $VERSION for $ARCH"
cargo build --release --locked

# ---- stage the file tree --------------------------------------------------

say "Staging"
install -Dm755 "target/release/$PKG"       "$STAGE/usr/bin/$PKG"
install -Dm644 "data/$APP_ID.desktop"      "$STAGE/usr/share/applications/$APP_ID.desktop"
install -Dm644 "data/$APP_ID.metainfo.xml" "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
install -Dm644 "data/icons/hicolor/scalable/apps/$APP_ID.svg" \
  "$STAGE/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
install -Dm644 "data/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg" \
  "$STAGE/usr/share/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg"

# DBusActivatable in the desktop file needs a matching service file.
install -Dm644 /dev/stdin "$STAGE/usr/share/dbus-1/services/$APP_ID.service" <<EOF
[D-BUS Service]
Name=$APP_ID
Exec=/usr/bin/$PKG --gapplication-service
EOF

if [[ -n "$WITH_WHISPER" ]]; then
  say "Building whisper-cli to include"
  packaging/build-whisper.sh --staged "$STAGE"
fi

install -Dm644 packaging/deb/copyright "$STAGE/usr/share/doc/$PKG/copyright"
printf '%s (%s) unstable; urgency=low\n\n  * Release %s.\n\n -- Matthew Hagrelius <matthew@hagreli.us>  %s\n' \
  "$PKG" "$VERSION" "$VERSION" "$(date -R)" \
  | gzip -9n > "$STAGE/usr/share/doc/$PKG/changelog.Debian.gz"
chmod 644 "$STAGE/usr/share/doc/$PKG/changelog.Debian.gz"

# ---- work out the library dependencies ------------------------------------

# dpkg-shlibdeps reads the real ELF headers, so the versions track whatever this
# was actually linked against instead of a guess that rots.
say "Resolving dependencies"
mkdir -p "$STAGE/debian"
cat > "$STAGE/debian/control" <<EOF
Source: $PKG
Package: $PKG
Architecture: $ARCH
EOF

SHLIBS=""
if SHLIBS="$(cd "$STAGE" && dpkg-shlibdeps -O --ignore-missing-info "usr/bin/$PKG" 2>/dev/null)"; then
  SHLIBS="${SHLIBS#shlibs:Depends=}"
else
  SHLIBS="libc6, libgtk-4-1, libadwaita-1-0, libglib2.0-0"
  echo "  dpkg-shlibdeps unavailable; using fallback: $SHLIBS" >&2
fi
rm -rf "$STAGE/debian"

# dpkg-shlibdeps derives minimums from the ELF symbols actually referenced,
# which understates the runtime requirement: the stylesheet uses CSS custom
# properties (GTK 4.16) and the crate is compiled against the libadwaita 1.9 API
# level. Neither shows up as a symbol, so a user on GTK 4.14 would install
# happily and get an unstyled window. Raise the floors.
SHLIBS="$(printf '%s' "$SHLIBS" | sed \
  -e 's/libgtk-4-1 ([^)]*)/libgtk-4-1 (>= 4.16)/' \
  -e 's/libgtk-4-1\([,$]\)/libgtk-4-1 (>= 4.16)\1/' \
  -e 's/libadwaita-1-0 ([^)]*)/libadwaita-1-0 (>= 1.9)/' \
  -e 's/libadwaita-1-0\([,$]\)/libadwaita-1-0 (>= 1.9)\1/')"

INSTALLED_SIZE="$(du -sk --exclude=DEBIAN "$STAGE" | cut -f1)"

# ---- control files --------------------------------------------------------

mkdir -p "$STAGE/DEBIAN"
sed -e "s/@VERSION@/$VERSION/" \
    -e "s/@ARCH@/$ARCH/" \
    -e "s|@DEPENDS@|$SHLIBS|" \
    -e "s/@INSTALLED_SIZE@/$INSTALLED_SIZE/" \
    packaging/deb/control.in > "$STAGE/DEBIAN/control"

# ---- build ----------------------------------------------------------------

mkdir -p "$DIST"
DEB="$DIST/${PKG}_${VERSION}_${ARCH}.deb"
say "Packaging"
fakeroot dpkg-deb --build --root-owner-group "$STAGE" "$DEB" >/dev/null

echo
say "Built $DEB"
dpkg-deb --info "$DEB" | sed 's/^/  /'
echo "  Contents:"
dpkg-deb --contents "$DEB" | awk '{print "    " $6}' | grep -vE '/$' | sort

if [[ "${1:-}" == "--install" ]]; then
  echo
  say "Installing (needs sudo)"
  sudo apt-get install -y "$DEB"
fi

echo
echo "Install with:  sudo apt install $DEB"
