# Snipunix

**Screenshot. Annotate. Share.**

Snipunix is a Linux screenshot tool built for GNOME Wayland. This repository is
the **M0 prototype**: portal capture, a frozen-image region-selection overlay,
PNG saving, and a GlobalShortcuts portal spike. Annotation, OCR, sharing,
recording, and cloud features are not part of M0.

## Status

| Area | M0 state |
| --- | --- |
| Full-screen capture | Works through `org.freedesktop.portal.Screenshot` v2 |
| Headless CLI capture | `snipunix --capture screen --output PATH` |
| Region selection | Frozen desktop, one fullscreen window per monitor, auto-capture on mouse release |
| Cancel | Escape, right-click |
| Saving | `--output PATH`; the GUI displays the result but has no Save dialog yet |
| Global shortcuts | Prototype, registered at runtime through the GlobalShortcuts portal |
| Flatpak | Minimal manifest and build instructions in `flatpak/` |
| Annotation / OCR / QR / recording / audio | Not implemented (M1+) |

Tested on Ubuntu 26.04, GNOME 50.1, Wayland, one 1920×1080 monitor. See
[Known limitations](#known-limitations) before relying on any behaviour.

## Requirements

- Rust 1.85 or newer (edition 2024)
- GTK 4 and libadwaita development files
- `xdg-desktop-portal` with the GNOME backend
- Flatpak and `flatpak-builder` only for the Flatpak build

On Ubuntu:

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
    xdg-desktop-portal xdg-desktop-portal-gnome flatpak flatpak-builder
```

The Rust dependencies and their licenses are listed in
[docs/DEPENDENCIES.md](docs/DEPENDENCIES.md).

## Build and test

```sh
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Run

Launch the GUI:

```sh
./target/debug/snipunix
```

Capture the whole desktop without opening a window:

```sh
./target/debug/snipunix --capture screen --output /tmp/snipunix-screen.png
```

Capture a region from the GUI overlay and save it:

```sh
./target/debug/snipunix --capture region --output /tmp/snipunix-region.png
```

Show portal capabilities:

```sh
./target/debug/snipunix --portal-info
```

Register the prototype global shortcuts:

```sh
./target/debug/snipunix --register-shortcuts
```

### Overlay controls

| Input | Action |
| --- | --- |
| Drag with the primary button | Draw the selection rectangle |
| Release the primary button | Capture the selection immediately |
| Escape | Cancel the overlay (during a drag the first Escape clears the rectangle) |
| Right-click | Cancel the overlay |

The overlay shows a frozen copy of the desktop, dims it, and restores the
undimmed image inside the selection. A size badge reports the crop in captured
image pixels. The image used by the overlay is the same portal capture that is
cropped on release, so what you select is what you save.

Enter and double-click are still wired as defensive confirmations, but a normal
drag captures on release and the `Selected` state is transient.

## Portal permissions

The first screenshot request may show a desktop permission dialog. On GNOME,
the persisted decision is also visible under **Settings → Privacy →
Screenshots**. If the request is denied, Snipunix shows a typed error instead of
panicking:

- `PermissionDenied` / `PermissionDeniedWithDetail`
- `Cancelled` / `CancelledWithDetail`
- `PortalUnavailable`
- `Timeout` (15 seconds)

The Screenshot portal writes a temporary `Screenshot-*.png` file (usually in
`~/Pictures`). Snipunix reads it, removes it when the path looks like a
portal-created screenshot, and keeps the pixels in memory. Portal cleanup is
best-effort; a failure is logged and never blocks the capture.

## Global shortcuts (prototype)

The portal identifies callers by application ID. A host build must have a
matching desktop entry installed, otherwise GNOME returns
`org.freedesktop.portal.Error.NotAllowed: An app id is required`.

Install the host desktop entry and put `snipunix` in `PATH` for local testing:

```sh
install -Dm644 data/io.github.alokverma9.snipunix.desktop \
    ~/.local/share/applications/io.github.alokverma9.snipunix.desktop
install -Dm644 data/io.github.alokverma9.snipunix.svg \
    ~/.local/share/icons/hicolor/scalable/apps/io.github.alokverma9.snipunix.svg
mkdir -p ~/.local/bin
ln -sf "$PWD/target/debug/snipunix" ~/.local/bin/snipunix
update-desktop-database ~/.local/share/applications
```

Then launch `snipunix --register-shortcuts` and keep the application running.
Snipunix asks `org.freedesktop.host.portal.Registry` to give the process the
`io.github.alokverma9.snipunix` identity before calling the GlobalShortcuts
portal.

- `Ctrl+Shift+S` — capture a region
- `Ctrl+Shift+A` — capture the full screen

In testing GNOME accepted both bindings and delivered them while another window
was focused. **The shortcuts are session-scoped:** closing Snipunix stops the
listener, so they only work while the application is running. Persistent
shortcuts that launch a closed application need a background/autostart service,
which is outside M0.

Inside Flatpak the app ID comes from the sandbox; host registration is skipped.

## Flatpak

The manifest is `flatpak/io.github.alokverma9.snipunix.json`. The Cargo source
list is generated from `Cargo.lock`, so the build does not need network access
inside the sandbox.

```sh
flatpak remote-add --user --if-not-exists flathub \
    https://flathub.org/repo/flathub.flatpakrepo

flatpak install --user flathub \
    org.gnome.Platform//50 \
    org.gnome.Sdk//50 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08

python3 flatpak/generate-cargo-sources.py Cargo.lock \
    -o flatpak/cargo-sources.json

flatpak-builder --user --install --force-clean --disable-rofiles-fuse \
    build-flatpak flatpak/io.github.alokverma9.snipunix.json

flatpak run io.github.alokverma9.snipunix
```

`--disable-rofiles-fuse` avoids a rofiles-fuse dependency on hosts that do not
have it. The build itself is offline: every crate is extracted from the
generated source list.

Headless capture inside the sandbox, with `/tmp` shared only for this test:

```sh
flatpak run --filesystem=/tmp io.github.alokverma9.snipunix \
    --capture screen --output /tmp/flatpak-screen.png
```

The Flatpak gets `wayland`, `fallback-x11`, `ipc`, and `dri` access. It does not
request broad filesystem access; screenshot files are handled through the
portal. In the sandbox the portal returns a document-portal path such as
`/run/user/1000/doc/<id>/Screenshot-5.png`. That path is read-only, so the
best-effort deletion logs a warning and the document portal manages its
lifetime.

## Repository layout

```text
crates/snipunix-core      Geometry, monitor layout, coordinate mapping,
                          selection state machine, RGBA crop
crates/snipunix-capture   Portal screenshot backend, URI handling, PNG decode,
                          portal probe
crates/snipunix-overlay   GDK monitor discovery, fullscreen overlay windows,
                          GSK renderer and input
crates/snipunix-app       CLI, libadwaita shell, capture flow, shortcuts
data/                     Desktop entry, AppStream metadata, icon
flatpak/                  Manifest and Cargo source generator
docs/                     Architecture, dependencies, test evidence
```

## Known limitations

- **One monitor was available for live testing.** Per-monitor windows and
  virtual-desktop coordinates are implemented, but multi-monitor and mixed-DPI
  behaviour were not exercised on hardware. Do not treat them as validated.
- The portal does not report monitor geometry. M0 derives one uniform scale per
  axis from the GDK bounding box and the image size. Mixed-DPI desktops can
  therefore crop incorrectly.
- The overlay uses one fullscreen window per monitor. Dragging across monitors
  and hotplug during a selection are not handled.
- There is no move/resize of an existing selection, magnifier, countdown,
  window snap, keyboard nudging, or toolbar.
- The GUI has no Save dialog. Use `--output PATH` to write a PNG; otherwise the
  crop is only displayed.
- The app quits when its last window closes, so global shortcuts do not survive
  closing the app. They also must be registered again after a restart.
- A second `--capture region` launch while Snipunix is already running activates
  the existing window instead of starting a new capture; the arguments of the
  second process are not forwarded. Headless `--capture screen --output` is
  unaffected because it does not start GTK.
- The Flatpak build was validated on this host; other runtimes and distributions
  are untested.
- X11 is only a fallback socket in the manifest. The M0 overlay was tested on
  Wayland; X11 interaction was not validated on a real session.
- Escape and drag/release were exercised manually. Automated tests cover the
  pure geometry, selection, URI, and CLI logic, not compositor interaction.

## Documentation

- [M0 architecture](docs/M0_ARCHITECTURE.md)
- [M0 test evidence](docs/M0_TEST_EVIDENCE.md)
- [Dependencies and licenses](docs/DEPENDENCIES.md)
- [M0 completion report](M0_COMPLETION_REPORT.md)
- Phase 0 research reports remain in `research/`.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
