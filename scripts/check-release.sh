#!/usr/bin/env bash
# Runs the release gates that can be checked without a desktop session.
#
#   scripts/check-release.sh
#
# The gates are:
#
#   * cargo fmt --check
#   * cargo clippy --all-targets -- -D warnings
#   * cargo test --workspace
#   * scripts/i18n.sh check      (the compiled catalogues match the sources)
#   * cargo deny check           (licences and advisories)

set -euo pipefail

repository="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository"

step() {
    echo
    echo "=== $1 ==="
}

step "formatting"
cargo fmt --all -- --check

step "clippy"
cargo clippy --workspace --all-targets -- -D warnings

step "tests"
cargo test --workspace

step "translations"
if command -v msgfmt >/dev/null; then
    scripts/i18n.sh check
else
    echo "msgfmt is not installed; skipping the catalogue check" >&2
fi

step "supply chain"
if command -v cargo-deny >/dev/null; then
    cargo deny check
else
    echo "cargo-deny is not installed; skipping the supply-chain check" >&2
fi

echo
echo "all local release gates passed"
