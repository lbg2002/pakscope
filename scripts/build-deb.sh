#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
command -v cargo >/dev/null || { echo 'Missing cargo; install Rust first.' >&2; exit 1; }
command -v dpkg-deb >/dev/null || { echo 'Missing dpkg-deb.' >&2; exit 1; }
VERSION="$(sed -n '/^\[package\]/,/^\[/{s/^version = "\(.*\)"/\1/p;}' Cargo.toml | head -1)"
ARCH="$(dpkg --print-architecture)"
[[ -n "$VERSION" ]] || { echo 'Cannot read version' >&2; exit 1; }
[[ -f Cargo.lock ]] || cargo generate-lockfile
cargo build --locked --release --bin pakscope --bin pakscope-scan
STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT
install -Dm755 target/release/pakscope "$STAGING/usr/bin/pakscope"
install -Dm755 target/release/pakscope-scan "$STAGING/usr/bin/pakscope-scan"
install -Dm644 assets/io.github.lbg2002.PakScope.desktop "$STAGING/usr/share/applications/io.github.lbg2002.PakScope.desktop"
install -Dm644 assets/io.github.lbg2002.PakScope.svg "$STAGING/usr/share/icons/hicolor/scalable/apps/io.github.lbg2002.PakScope.svg"
mkdir -p "$STAGING/DEBIAN" dist
cat > "$STAGING/DEBIAN/control" <<EOC
Package: pakscope
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Maintainer: PakScope Contributors <noreply@github.com>
Depends: libgtk-4-1 (>= 4.12), libadwaita-1-0 (>= 1.4)
Description: Lightweight read-only Linux application inventory
 Discover installed desktop applications from APT, Snap, Flatpak and
 selected AppImage directories without elevated privileges.
EOC
dpkg-deb --build --root-owner-group "$STAGING" "dist/pakscope_${VERSION}_${ARCH}.deb"
echo "Created dist/pakscope_${VERSION}_${ARCH}.deb"
