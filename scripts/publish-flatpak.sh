#!/usr/bin/env bash
# Publishes a channel to a Flatpak repository.
#
#   scripts/publish-flatpak.sh stable <remote-ostree-url>
#   scripts/publish-flatpak.sh beta <remote-ostree-url>
#
# The repository is a plain OSTree repository. The remote can be an
# `ostree` server, a static file host, or a Git repository that Flatpak can
# pull from.
#
# The stable and beta channels use separate repositories so a user can follow
# one without the other:
#
#   stable: https://example.org/snipunix-stable.flatpakrepo
#   beta:   https://example.org/snipunix-beta.flatpakrepo
#
# To follow the beta channel:
#
#   flatpak remote-add --if-not-exists snipunix-beta \
#       https://example.org/snipunix-beta.flatpakrepo
#   flatpak install snipunix-beta io.github.alokverma9.snipunix.Beta

set -euo pipefail

repository="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository"

channel="${1:-stable}"
remote="${2:-}"

case "$channel" in
    stable)
        app_id="io.github.alokverma9.snipunix"
        ;;
    beta)
        app_id="io.github.alokverma9.snipunix.Beta"
        ;;
    *)
        echo "usage: $0 {stable|beta} <remote-ostree-url>" >&2
        exit 2
        ;;
esac

local_repo="flatpak/repo-$channel"

if [ ! -d "$local_repo" ]; then
    echo "the local repository $local_repo does not exist; run scripts/build-flatpak.sh $channel first" >&2
    exit 1
fi

if [ -z "$remote" ]; then
    echo "no remote given; the repository is in $local_repo"
    echo "publish it with: flatpak build-export $remote $local_repo"
    exit 0
fi

echo "Exporting $app_id to $remote…"
flatpak build-export --update-appstream "$remote" "$local_repo" "$app_id"

echo
echo "Published $app_id."
echo
echo "Users follow the channel with:"
echo "  flatpak remote-add --if-not-exists snipunix-$channel $remote"
echo "  flatpak install snipunix-$channel $app_id"
