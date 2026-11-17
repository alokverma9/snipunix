#!/usr/bin/env bash
# Build a self-contained Snipunix .deb with the two local OCR models.
#
# Usage: packaging/build-deb.sh
# Environment:
#   SNIPUNIX_MODELS  use this directory instead of downloading models
#   DEB_ARCH         override the package architecture (default: dpkg)
#   SKIP_BUILD=1     reuse an existing target/release/snipunix
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
if [[ -z "$version" ]]; then
    echo "could not read the workspace version from Cargo.toml" >&2
    exit 1
fi
arch=${DEB_ARCH:-$(dpkg --print-architecture)}

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
    cargo build --release --locked --bin snipunix
fi
if [[ ! -x target/release/snipunix ]]; then
    echo "target/release/snipunix is missing; build it first" >&2
    exit 1
fi

models=${SNIPUNIX_MODELS:-${XDG_CACHE_HOME:-$HOME/.cache}/snipunix/models}
mkdir -p "$models"
if [[ ! -f "$models/text-detection.onnx" || ! -f "$models/text-recognition.onnx" ]]; then
    echo "downloading and verifying OCR models into $models"
    target/release/snipunix --download-ocr-models --ocr-models "$models"
fi

package_root=$(mktemp -d)
trap 'rm -rf "$package_root"' EXIT

install -Dm755 target/release/snipunix "$package_root/usr/bin/snipunix"
install -Dm644 data/io.github.alokverma9.snipunix.desktop \
    "$package_root/usr/share/applications/io.github.alokverma9.snipunix.desktop"
install -Dm644 data/io.github.alokverma9.snipunix.metainfo.xml \
    "$package_root/usr/share/metainfo/io.github.alokverma9.snipunix.metainfo.xml"
install -Dm644 data/io.github.alokverma9.snipunix.svg \
    "$package_root/usr/share/icons/hicolor/scalable/apps/io.github.alokverma9.snipunix.svg"
install -Dm644 "$models/text-detection.onnx" \
    "$package_root/usr/share/snipunix/models/text-detection.onnx"
install -Dm644 "$models/text-recognition.onnx" \
    "$package_root/usr/share/snipunix/models/text-recognition.onnx"
for locale in de es; do
    install -Dm644 "crates/snipunix-i18n/locales/$locale/LC_MESSAGES/snipunix.mo" \
        "$package_root/usr/share/locale/$locale/LC_MESSAGES/snipunix.mo"
done
install -Dm644 LICENSE "$package_root/usr/share/doc/snipunix/copyright"

mkdir -p "$package_root/DEBIAN"
cat > "$package_root/DEBIAN/control" <<EOF
Package: snipunix
Version: ${version}-1
Section: graphics
Priority: optional
Architecture: ${arch}
Depends: libgtk-4-1, libadwaita-1-0, xdg-desktop-portal, libgstreamer1.0-0, gstreamer1.0-plugins-base, gstreamer1.0-plugins-good, gstreamer1.0-plugins-ugly, gstreamer1.0-libav
Recommends: xdg-desktop-portal-gnome, gstreamer1.0-pulseaudio
Maintainer: Snipunix contributors <noreply@example.com>
Description: Screenshot, annotate, record, and local OCR for GNOME Wayland
 Snipunix captures a region or the full screen through the desktop
 Screenshot portal, records a monitor or region through ScreenCast,
 edits captures non-destructively, and extracts text with on-device
 OCR. Images, recordings, and recognised text never leave the machine.
 The interface is translated into German and Spanish.
EOF

mkdir -p dist
output="dist/snipunix_${version}_${arch}.deb"
rm -f "$output"
dpkg-deb --build --root-owner-group "$package_root" "$output" >/dev/null
echo "built $output"
