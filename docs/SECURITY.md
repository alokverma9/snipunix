# Security

Snipunix handles material that a user may not want to share: screenshots of
their desktop, the text in them, and recordings of their screen. This
document describes how the project handles that material and how the release
gate checks the claims.

## Reporting a vulnerability

Report a security problem privately to the maintainers rather than in a
public issue. Include the version, the desktop, the steps to reproduce, and
what an attacker gains. A fix and a new release follow a private report.

Crashes and ordinary bugs belong in the issue tracker. Use **Export
diagnostics** in Settings → About, or `snipunix --diagnostics-output`, and
attach the report. It contains no screen content and no recognised text.

## The threat model

Snipunix is a local desktop application. The realistic threats are:

| Threat | What is done about it |
| --- | --- |
| A captured screen leaking to a third party | Nothing leaves the machine. See [PRIVACY.md](PRIVACY.md). |
| A malicious portal handing over an oversized or malformed image | The image size is checked before the decoder allocates, and the maximum is 512 MiB. |
| A malicious portal handing over a file outside the sandbox | The portal URI is parsed and only `file://` URIs inside the allowed directories are read. |
| A crafted image crashing the decoder | The decoder is the `image` crate; a malformed file produces an error, not a panic. The crop and geometry code is fuzzed. |
| Untrusted input reaching the OCR model | The model is a local ONNX file with a pinned hash; the input is a pixel buffer, not code. |
| A dependency with a known vulnerability | `cargo deny` and `cargo audit` are part of the release gate. |
| A malicious Flatpak | The manifest requests no network access and no broad filesystem access. |

What is *not* in the threat model: a compromised desktop, a malicious
compositor that lies about the captured image, or a local attacker who can
already read the user's screen.

## Sandbox

The Flatpak is the recommended installation. Its manifest requests only what
the application needs:

```
--share=ipc
--socket=wayland
--socket=fallback-x11
--socket=pulseaudio
--device=dri
--filesystem=xdg-videos
```

There is no `--socket=session-bus`, because the portals are reached through
the session bus that the runtime provides by default. There is no
`--share=network`, so the runtime blocks internet access entirely. The OCR
models are bundled into the image, so the download path is not needed inside
the sandbox.

The deb build has no sandbox. It relies on the desktop portal for the same
boundary.

## Unsafe code

The rule for the codebase is: no `unsafe` outside the modules that talk to
C libraries directly.

| Crate | `unsafe` |
| --- | --- |
| `snipunix-core` | forbidden |
| `snipunix-capture` | forbidden |
| `snipunix-annotate` | forbidden |
| `snipunix-settings` | forbidden |
| `snipunix-ocr` | forbidden |
| `snipunix-overlay` | forbidden |
| `snipunix-record` | forbidden |
| `snipunix-i18n` | forbidden |
| `snipunix-diagnostics` | forbidden |
| `snipunix-app` | forbidden |

Every crate sets `unsafe_code = "forbid"` in its lint table, so an `unsafe`
block is a compile error rather than a review comment.

## Input validation

The portal is the untrusted input boundary.

- **Images.** The dimensions are read from the encoded header before the
  decoder allocates, and an image above 512 MiB of RGBA is rejected.
- **URIs.** The portal returns a `file://` URI. The parser accepts only
  local files and rejects anything with a host, a port, or a scheme other
  than `file`.
- **Coordinates.** The selection code works with newtypes over `f64` so
  logical desktop pixels and captured image pixels cannot be mixed. The
  geometry crate is fuzzed in `crates/snipunix-core/tests/fuzz.rs`.
- **Settings.** The settings file is parsed as TOML, then clamped into
  range. An invalid file falls back to the defaults instead of failing the
  start-up.
- **Shortcuts.** The portal trigger syntax is passed through to the portal,
  never to a shell.

## Supply chain

| Check | Tool | Gate |
| --- | --- | --- |
| Known vulnerabilities | `cargo audit` | No vulnerability may be open. |
| Licences and sources | `cargo deny check` | Every licence must be compatible with GPL-3.0-or-later; only crates.io is allowed. |
| Wildcard dependencies | `cargo deny check bans` | Denied. |
| Reproducibility | `cargo build --locked` | The Flatpak build is offline and uses the vendored `flatpak/cargo-sources.json`. |

The ignored advisories are listed in `deny.toml` and `.cargo/audit.toml` with
the reason each one is acceptable. All four are "the crate is unmaintained"
notices for crates that have been merged into another crate; none of them
reports a vulnerability.

To run the checks:

```sh
cargo deny check
cargo audit
```

## Privacy

The privacy promise is checked by two gates, described in
[PRIVACY.md](PRIVACY.md):

- `cargo test -p snipunix-app --test no_network` walks the source tree and
  fails if a network primitive appears outside the OCR crate, or if the
  capture and OCR crates become connected.
- `scripts/check-no-network.sh` runs a capture under `strace` and fails if
  an internet socket is opened.

## Crash reporting

Crash reporting is **off by default** and is a local file, not a service.

- The setting is `diagnostics.crash_reports` in `settings.toml` and defaults
  to `false`.
- When it is off, a panic is logged and nothing is written to disk.
- When it is on, a file with the timestamp, the version, the panic message,
  the source location, the thread name, and a backtrace is written to
  `$XDG_STATE_HOME/snipunix/crashes`. The directory is pruned to the newest
  ten files.
- The file never contains screen content, recognised text, or clipboard
  content.
- The user deletes it, or turns the setting off, whenever they want.

## Release gate

The checks that must pass before a release:

```sh
scripts/check-release.sh
```

which runs:

1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`
4. `scripts/i18n.sh check`
5. `cargo deny check`

Plus, for a release build:

6. `cargo audit`
7. `scripts/check-no-network.sh`
8. The M1–M5 acceptance criteria re-run on the release build.
9. No P1 or P2 bug is open.
10. The Flatpak and the deb are reproducible from `Cargo.lock`.
