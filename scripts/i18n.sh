#!/usr/bin/env bash
# Maintain the Snipunix translation catalogues.
#
#   scripts/i18n.sh extract   regenerate po/snipunix.pot from the sources
#   scripts/i18n.sh merge     merge the template into every po/<lang>.po
#   scripts/i18n.sh compile   compile po/<lang>.po into the embedded .mo files
#   scripts/i18n.sh check     verify the committed .mo files match the .po files
#   scripts/i18n.sh all       extract, merge and compile
#
# `msgfmt` and `msgmerge` come from GNU gettext. Install them with
# `sudo apt install gettext`.

set -euo pipefail

repository="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
po_directory="$repository/po"
locale_directory="$repository/crates/snipunix-i18n/locales"

command -v msgfmt >/dev/null || {
    echo "msgfmt not found; install GNU gettext" >&2
    exit 1
}

extract() {
    python3 "$repository/tools/i18n-extract.py" "$po_directory/snipunix.pot"
}

merge() {
    for catalogue in "$po_directory"/*.po; do
        [ -e "$catalogue" ] || continue
        echo "merging $(basename "$catalogue")"
        msgmerge --quiet --update --backup=none "$catalogue" "$po_directory/snipunix.pot"
    done
}

compile() {
    mkdir -p "$locale_directory"
    for catalogue in "$po_directory"/*.po; do
        [ -e "$catalogue" ] || continue
        locale="$(basename "$catalogue" .po)"
        target="$locale_directory/$locale/LC_MESSAGES"
        mkdir -p "$target"
        msgfmt --check --statistics --output-file="$target/snipunix.mo" "$catalogue"
    done
}

check() {
    status=0
    for catalogue in "$po_directory"/*.po; do
        [ -e "$catalogue" ] || continue
        locale="$(basename "$catalogue" .po)"
        committed="$locale_directory/$locale/LC_MESSAGES/snipunix.mo"
        if [ ! -f "$committed" ]; then
            echo "$locale: the compiled catalogue is missing" >&2
            status=1
            continue
        fi
        temporary="$(mktemp)"
        msgfmt --output-file="$temporary" "$catalogue"
        if ! cmp -s "$temporary" "$committed"; then
            echo "$locale: the compiled catalogue is out of date; run scripts/i18n.sh compile" >&2
            status=1
        fi
        rm -f "$temporary"
    done
    exit "$status"
}

case "${1:-all}" in
    extract) extract ;;
    merge) merge ;;
    compile) compile ;;
    check) check ;;
    all)
        extract
        merge
        compile
        ;;
    *)
        echo "usage: $0 {extract|merge|compile|check|all}" >&2
        exit 2
        ;;
esac
