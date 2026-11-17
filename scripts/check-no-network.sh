#!/usr/bin/env bash
# Verifies that Snipunix opens no network connection during a normal capture.
#
#   scripts/check-no-network.sh
#
# The check runs the headless capture under `strace` and fails if the process
# opens an internet socket or connects to an address outside the local
# machine. Unix sockets are expected: the application talks to the desktop
# portal over D-Bus.
#
# The static counterpart of this check lives in
# `crates/snipunix-app/tests/no_network.rs`.

set -euo pipefail

repository="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository"

if ! command -v strace >/dev/null; then
    echo "strace is not installed; skipping the runtime network check" >&2
    exit 0
fi

cargo build --release --bin snipunix

output="$(mktemp -d)/capture.png"
trace="$(mktemp)"

cleanup() {
    rm -f "$trace"
}
trap cleanup EXIT

# The capture itself may fail on a machine without a portal; the network
# check is still meaningful, so the exit status of the capture is ignored.
set +e
strace -f -qq -e trace=socket,connect -o "$trace" \
    ./target/release/snipunix --capture screen --output "$output" >/dev/null 2>&1
set -e

echo "Socket and connect calls made during the capture:"
grep -E "socket\(|connect\(" "$trace" | sed 's/^/  /' || true

if grep -qE "socket\(AF_INET" "$trace"; then
    echo
    echo "the process opened an internet socket" >&2
    exit 1
fi

if grep -qE "connect\([0-9]+, \{sa_family=AF_INET" "$trace"; then
    echo
    echo "the process connected over IPv4" >&2
    exit 1
fi

if grep -qE "connect\([0-9]+, \{sa_family=AF_INET6" "$trace"; then
    echo
    echo "the process connected over IPv6" >&2
    exit 1
fi

echo
echo "no internet socket was opened during the capture"
