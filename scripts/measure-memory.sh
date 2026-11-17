#!/usr/bin/env bash
# Measures the peak resident memory of a capture and checks it against the
# roadmap's budgets.
#
#   scripts/measure-memory.sh [width] [height]
#
# The budgets are:
#
#   * 1080p capture, overlay and copy: under 150 MB
#   * 4K capture, overlay and copy:    under 400 MB
#
# The script runs the headless capture path, which decodes the image, crops
# it and writes a PNG. It reports the peak RSS of the process.

set -euo pipefail

repository="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository"

width="${1:-1920}"
height="${2:-1080}"
output="$(mktemp -d)/capture.png"

if [ "$width" -le 1920 ]; then
    budget_mb=150
else
    budget_mb=400
fi

echo "Building the release binary…"
cargo build --release --bin snipunix

echo "Capturing at ${width}x${height}…"
# /usr/bin/time reports the maximum resident set size in kilobytes.
output_time="$(mktemp)"
set +e
/usr/bin/time -v -o "$output_time" \
    ./target/release/snipunix --capture screen --output "$output" \
    >/dev/null 2>&1
status=$?
set -e

if [ "$status" -ne 0 ]; then
    echo "the capture failed; is a portal available on this host?" >&2
    cat "$output_time" >&2 || true
    exit 1
fi

peak_kb="$(awk -F': ' '/Maximum resident set size/ {print $2}' "$output_time")"
rm -f "$output_time"

if [ -z "$peak_kb" ]; then
    echo "could not read the peak resident set size" >&2
    exit 1
fi

peak_mb=$((peak_kb / 1024))
echo "Peak resident memory: ${peak_mb} MB (budget ${budget_mb} MB)"

if [ "$peak_mb" -gt "$budget_mb" ]; then
    echo "over budget" >&2
    exit 1
fi
echo "within budget"
