# Dependencies and licenses

Snipunix uses a deliberately small direct dependency set. Versions are pinned in
`Cargo.toml`; the full transitive graph is recorded in `Cargo.lock`. M1 added no
new direct dependencies (the overlay crate now also uses the existing `gio`
workspace dependency for monitor-list change signals).

## Direct Rust dependencies

| Crate | Version | License | Purpose |
| --- | --- | --- | --- |
| `gtk4` | 0.11.5 | MIT | GTK 4 bindings for the shell and overlay widgets |
| `gdk4` | 0.11.5 | MIT | Displays, monitors, textures, input constants |
| `glib` | 0.22.10 | MIT | GLib types, main-loop integration, markup escaping |
| `gio` | 0.22.10 | MIT | `GApplication` actions and menus; overlay monitor-list signals |
| `libadwaita` | 0.9.2 | MIT (Rust bindings) | GNOME shell widgets; the libadwaita library itself is LGPL-2.1-or-later |
| `ashpd` | 0.14.0 | MIT | Screenshot and GlobalShortcuts portal clients |
| `image` | 0.25.10 | MIT OR Apache-2.0 | PNG decode/encode |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | Typed error enums |
| `anyhow` | 1.0.104 | MIT OR Apache-2.0 | Top-level CLI error context |
| `tracing` | 0.1.44 | MIT | Structured logging |
| `tracing-subscriber` | 0.3.23 | MIT | Log formatting and `RUST_LOG` filter |
| `async-io` | 2.6.0 | Apache-2.0 OR MIT | Timer and headless blocking executor |
| `async-channel` | 2.5.0 | Apache-2.0 OR MIT | Portal event channel |
| `futures-lite` | 2.6.1 | Apache-2.0 OR MIT | Async combinators and streams |
| `url` | 2.5.8 | MIT OR Apache-2.0 | Portal URI parsing |

The four workspace crates (`snipunix-core`, `snipunix-capture`,
`snipunix-overlay`, `snipunix-app`) are GPL-3.0-or-later.

## Transitive licenses

`cargo metadata --format-version 1 --locked` reports these license expressions
across the full graph:

| Count | License expression |
| --- | --- |
| 74 | MIT OR Apache-2.0 |
| 51 | MIT |
| 18 | Unicode-3.0 |
| 17 | Apache-2.0 OR MIT |
| 4 | GPL-3.0-or-later (this workspace) |
| 3 | Unlicense OR MIT |
| 2 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| 2 | MIT OR Zlib OR Apache-2.0 |
| 2 | BSD-3-Clause OR Apache-2.0 |
| 1 each | 0BSD OR MIT OR Apache-2.0; Zlib OR Apache-2.0 OR MIT; MIT OR Apache-2.0 OR LGPL-2.1-or-later; Apache-2.0 WITH LLVM-exception; (MIT OR Apache-2.0) AND Unicode-3.0; Zlib |

No dependency is copyleft-only except the GPL-3.0-or-later application itself.
The `libadwaita` and GTK system libraries linked at runtime are
LGPL-2.1-or-later; the Rust binding crates are MIT.

## Flatpak runtime

The manifest targets:

- `org.gnome.Platform//50`
- `org.gnome.Sdk//50`
- `org.freedesktop.Sdk.Extension.rust-stable//25.08`

These are distribution-provided runtime components, not vendored into this
repository. Their licenses are documented by their upstream projects.

## Regenerating the Flatpak source list

```sh
python3 flatpak/generate-cargo-sources.py Cargo.lock \
    -o flatpak/cargo-sources.json
```

The script intentionally supports only crates.io sources. If a future change
adds a git dependency, the build stops with an explicit error instead of
silently producing an incomplete source list.
