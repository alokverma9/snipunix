#!/usr/bin/env bash
# Builds the Flatpak packages.
#
#   scripts/build-flatpak.sh stable
#   scripts/build-flatpak.sh beta
#
# The stable build uses the identifier `io.github.alokverma9.snipunix`. The
# beta build uses `io.github.alokverma9.snipunix.Beta`, so the two channels
# can be installed side by side and keep their own portal permissions.
#
# The build is offline: every crate is vendored into
# `flatpak/cargo-sources.json`. Regenerate that file with
# `flatpak/generate-cargo-sources.py` after `Cargo.lock` changes.

set -euo pipefail

repository="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository"

channel="${1:-stable}"
case "$channel" in
    stable)
        app_id="io.github.alokverma9.snipunix"
        ;;
    beta)
        app_id="io.github.alokverma9.snipunix.Beta"
        ;;
    *)
        echo "usage: $0 {stable|beta}" >&2
        exit 2
        ;;
esac

if ! command -v flatpak-builder >/dev/null; then
    echo "flatpak-builder is not installed" >&2
    exit 1
fi

build_dir=".flatpak-builder/build/$app_id"
repo_dir="flatpak/repo-$channel"

echo "Building $app_id…"
flatpak-builder --force-clean --disable-rofiles-fuse --repo="$repo_dir" \
    "$build_dir" "flatpak/$app_id.json"

echo
echo "Built $app_id into $repo_dir"
echo
echo "To try it locally:"
echo "  flatpak-builder --user --install --force-clean $build_dir flatpak/$app_id.json"
echo "  flatpak run $app_id"
