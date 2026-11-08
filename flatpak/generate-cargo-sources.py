#!/usr/bin/env python3
"""Generate Flatpak Cargo sources from Cargo.lock.

This is a small, dependency-free equivalent of flatpak-cargo-generator.py for
projects that only use crates.io. It writes a JSON source list that extracts
every crate into ``cargo/vendor`` and a Cargo config that redirects
``crates-io`` to those vendored sources.

Usage:
    python3 flatpak/generate-cargo-sources.py Cargo.lock -o flatpak/cargo-sources.json
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib

CRATES_IO = "https://static.crates.io/crates"
VENDOR = "cargo/vendor"


def generate(cargo_lock_path: str) -> list[dict]:
    with open(cargo_lock_path, "rb") as handle:
        lock = tomllib.load(handle)

    sources: list[dict] = []
    for package in lock.get("package", []):
        source = package.get("source")
        if source is None:
            continue
        if not source.startswith("registry+https://github.com/rust-lang/crates.io-index"):
            raise SystemExit(
                f"unsupported non-crates.io source for {package['name']} "
                f"{package['version']}: {source}"
            )
        checksum = package.get("checksum")
        if not checksum:
            raise SystemExit(
                f"missing checksum for {package['name']} {package['version']}"
            )
        name = package["name"]
        version = package["version"]
        destination = f"{VENDOR}/{name}-{version}"
        sources.append(
            {
                "type": "archive",
                "archive-type": "tar-gzip",
                "url": f"{CRATES_IO}/{name}/{name}-{version}.crate",
                "sha256": checksum,
                "dest": destination,
            }
        )
        sources.append(
            {
                "type": "inline",
                "contents": json.dumps({"package": checksum, "files": {}}),
                "dest": destination,
                "dest-filename": ".cargo-checksum.json",
            }
        )

    sources.append(
        {
            "type": "inline",
            "contents": (
                "[source.crates-io]\n"
                'replace-with = "vendored-sources"\n\n'
                "[source.vendored-sources]\n"
                f'directory = "{VENDOR}"\n'
            ),
            "dest": "cargo",
            "dest-filename": "config",
        }
    )
    return sources


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cargo_lock", help="path to Cargo.lock")
    parser.add_argument(
        "-o", "--output", default="flatpak/cargo-sources.json", help="output JSON file"
    )
    args = parser.parse_args()
    sources = generate(args.cargo_lock)
    with open(args.output, "w", encoding="utf-8") as handle:
        json.dump(sources, handle, indent=2)
        handle.write("\n")
    print(f"wrote {len(sources)} sources to {args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
