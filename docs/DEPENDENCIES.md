# Dependencies and licenses

Snipunix uses a deliberately small direct dependency set. Versions are pinned in
`Cargo.toml`; the full transitive graph is recorded in `Cargo.lock`. M1 added no
new direct dependencies. M2 adds the CPU rasteriser (`tiny-skia`) and the text
stack (`cosmic-text` with `swash`/`fontdb`), and enables the `image` JPEG and
WebP features. M3 adds settings serialization (`serde`, `toml`), model
verification and download (`sha2`, `ureq`), the on-device OCR engine
(`ocrs`/`rten`/`rten-imageproc`), and the tray (`ksni`). M5 adds the GStreamer
bindings (`gstreamer`), a Ctrl+C handler for headless recordings (`ctrlc`), and
the `snipunix-record` workspace crate. The `ashpd` `screencast` feature enables
the PipeWire ScreenCast portal client.

The OCR path and tray are pure Rust. `ureq` is only used by the explicit
`--download-ocr-models` path and brings an HTTP/TLS stack; it is not used during
capture, OCR, or normal startup. Fontconfig is read at runtime from the
host/runtime fonts and adds no build dependency.

## Direct Rust dependencies

| Crate | Version | License | Purpose |
| --- | --- | --- | --- |
| `gtk4` | 0.11.5 | MIT | GTK 4 bindings for the shell, overlay, and editor widgets |
| `gdk4` | 0.11.5 | MIT | Displays, monitors, textures, input constants |
| `glib` | 0.22.10 | MIT | GLib types, main-loop integration, markup escaping |
| `gio` | 0.22.10 | MIT | `GApplication` actions and menus; monitor-list signals |
| `libadwaita` | 0.9.2 | MIT (Rust bindings) | GNOME shell widgets; the libadwaita library itself is LGPL-2.1-or-later |
| `ashpd` | 0.14.0 | MIT | Screenshot, ScreenCast, and GlobalShortcuts portal clients |
| `gstreamer` | 0.24.5 | MIT | In-process PipeWire consumption, H.264 encoding, MP4/Matroska muxing |
| `ctrlc` | 3.5.2 | MIT OR Apache-2.0 | Ctrl+C stop for headless recordings |
| `image` | 0.25.10 | MIT OR Apache-2.0 | PNG/JPEG decode and PNG/JPEG/WebP encode |
| `tiny-skia` | 0.12.0 | BSD-3-Clause | CPU vector rasteriser for annotations and export |
| `cosmic-text` | 0.19.0 | MIT OR Apache-2.0 | Multiline text shaping and layout |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | Typed error enums |
| `anyhow` | 1.0.104 | MIT OR Apache-2.0 | Top-level CLI error context |
| `tracing` | 0.1.44 | MIT | Structured logging |
| `tracing-subscriber` | 0.3.23 | MIT | Log formatting and `RUST_LOG` filter |
| `async-io` | 2.6.0 | Apache-2.0 OR MIT | Timer and headless blocking executor |
| `async-channel` | 2.5.0 | Apache-2.0 OR MIT | Portal event channel |
| `futures-lite` | 2.6.1 | Apache-2.0 OR MIT | Async combinators and streams |
| `url` | 2.5.8 | MIT OR Apache-2.0 | Portal URI parsing |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | Settings serialization derives |
| `toml` | 1.2.1 | MIT OR Apache-2.0 | Settings TOML encode/decode |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | SHA-256 model verification |
| `ureq` | 3.5.1 | MIT OR Apache-2.0 | Explicit OCR-model download (not used by capture/OCR) |
| `ocrs` | 0.14.0 | MIT OR Apache-2.0 | On-device OCR engine |
| `rten` | 0.28.0 | MIT OR Apache-2.0 | Neural-network runtime for `ocrs` |
| `rten-imageproc` | 0.28.0 | MIT OR Apache-2.0 | Image and bounding-box processing for `ocrs` |
| `ksni` | 0.3.7 | Unlicense | StatusNotifierItem tray |

`cosmic-text` brings `swash` (Apache-2.0 OR MIT), `fontdb` (MIT),
`fontconfig-parser` (MIT), and `ttf-parser` (MIT OR Apache-2.0) as its text
stack. `tiny-skia` brings `tiny-skia-path` (BSD-3-Clause). `image` brings the
`png`, `zune-jpeg`/`zune-core`, and WebP codecs under their upstream licenses.
`ocrs` brings `rten-*`, `rayon`, and `rten-sequence`/`rten-tensor`. `ksni` brings
`zbus`, `tokio`, and `async-*`. `ureq` brings its HTTP, cookie, compression, and
TLS stack (`rustls`, `native-tls`, `webpki-roots`). `ksni`'s `Unlicense` is a
public-domain dedication with no attribution requirement.

The eight workspace crates (`snipunix-core`, `snipunix-capture`,
`snipunix-annotate`, `snipunix-settings`, `snipunix-ocr`, `snipunix-overlay`,
`snipunix-record`, `snipunix-app`) are GPL-3.0-or-later.

GStreamer itself is a system library (LGPL-2.1-or-later) linked through the MIT
`gstreamer` bindings. The recording engine uses the GStreamer core, base, good,
ugly (x264), libav, and PulseAudio plugin sets at runtime; these are not
vendored into this repository. `snipunix-record` links only to the GStreamer
API, not to the plugin code.

## Transitive licenses

`cargo metadata --format-version 1 --locked` reports 332 packages in the M5
graph (M3: 307). License expressions across the full graph:

| Count | License expression |
| --- | --- |
| 157 | MIT OR Apache-2.0 |
| 78 | MIT |
| 30 | Apache-2.0 OR MIT |
| 18 | Unicode-3.0 |
| 8 | GPL-3.0-or-later (this workspace) |
| 5 | MIT/Apache-2.0 |
| 4 | Zlib OR Apache-2.0 OR MIT |
| 3 | Unlicense OR MIT |
| 3 | Apache-2.0 |
| 3 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| 3 | BSD-3-Clause |
| 2 | MIT OR Zlib OR Apache-2.0 |
| 2 | BSD-3-Clause OR Apache-2.0 |
| 2 | ISC |
| 2 | Zlib |
| 2 | MIT OR Apache-2.0 OR Zlib |
| 1 each | 0BSD OR MIT OR Apache-2.0; BSD-2-Clause; Unlicense; MIT OR Apache-2.0 OR LGPL-2.1-or-later; Apache-2.0 AND ISC; Apache-2.0 OR ISC OR MIT; Apache-2.0 OR GPL-2.0-only; Apache-2.0 WITH LLVM-exception; (MIT OR Apache-2.0) AND Unicode-3.0; CDLA-Permissive-2.0 |

No dependency is copyleft-only, except the GPL-3.0-or-later application itself
and the optional `self_cell` expression (`Apache-2.0 OR GPL-2.0-only`), which can
be used under Apache-2.0. The `libadwaita` and GTK system libraries linked at
runtime are LGPL-2.1-or-later; the Rust binding crates are MIT. The
`CDLA-Permissive-2.0` entry is the `webpki-root-certs` CA bundle data used by
the explicit download path.

## Flatpak runtime

The manifest targets:

- `org.gnome.Platform//50`
- `org.gnome.Sdk//50`
- `org.freedesktop.Sdk.Extension.rust-stable//25.08`

These are distribution-provided runtime components, not vendored into this
repository. Their licenses are documented by their upstream projects.

`org.gnome.Platform//50` ships the GStreamer runtime and the x264/libav plugins
used by recording. The manifest adds `--socket=pulseaudio` for microphone and
system-audio capture and `--filesystem=xdg-videos` for the default recording
folder.

## Regenerating the Flatpak source list

```sh
python3 flatpak/generate-cargo-sources.py Cargo.lock \
    -o flatpak/cargo-sources.json
```

The script intentionally supports only crates.io sources. If a future change
adds a git dependency, the build stops with an explicit error instead of
silently producing an incomplete source list.
