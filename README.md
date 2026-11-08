# Snipunix

**Screenshot. Annotate. Share.**

Snipunix is a Linux screenshot tool built for GNOME Wayland. This repository is
the **M1 prototype**: portal capture, a frozen-image region overlay with
move/resize handles, a magnifier, a delay countdown, clipboard copy, Save As,
and Adjust Region. Annotation, OCR, sharing, recording, and cloud features are
not part of M1.

## Status

| Area | M1 state |
| --- | --- |
| Full-screen capture | Works through `org.freedesktop.portal.Screenshot` v2 |
| Headless CLI capture | `snipunix --capture screen --output PATH` |
| Region selection | Frozen desktop, one fullscreen window per monitor, capture on mouse release |
| Move / resize | Eight handles, inside-drag move, keyboard nudge, minimum 8×8 logical px |
| Magnifier | 8× nearest-neighbour while dragging or resizing |
| Delay | `--delay 0..10` and a GUI spin button; countdown can be cancelled |
| Clipboard | Crop is copied automatically on capture |
| Preview actions | Copy, Save As…, Adjust Region |
| Adjust | Reopens the overlay with the previous rectangle and handles; Enter / Ctrl+C confirms |
| Cancel | Escape, right-click |
| Saving | `--output PATH` and a GUI Save As dialog |
| Resident window | Closing the window hides it while Snipunix owns the clipboard; Quit exits |
| Global shortcuts | Prototype, registered at runtime through the GlobalShortcuts portal |
| Flatpak | Manifest, offline source list, and a verified M1 build |
| Annotation / OCR / QR / recording / audio | Not implemented (M2+) |

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

M1 has **44 passing tests**: 29 core, 8 capture, 5 app, 2 overlay. See
[docs/M1_TEST_EVIDENCE.md](docs/M1_TEST_EVIDENCE.md).

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

Wait five seconds before freezing the screen:

```sh
./target/debug/snipunix --capture region --delay 5
```

Show portal capabilities:

```sh
./target/debug/snipunix --portal-info
```

Register the prototype global shortcuts:

```sh
./target/debug/snipunix --register-shortcuts
```

### Capture workflow

1. Drag a rectangle. The frozen overlay dims the desktop and restores the
   selected pixels; a badge reports the crop size.
2. Release the primary button. Snipunix crops the region, copies it to the
   clipboard, optionally saves it (`--output`), and shows a preview with
   **Copy**, **Save As…**, and **Adjust Region**.
3. If the selection is not right, choose **Adjust Region**. The overlay reopens
   with the previous rectangle; move or resize it, then press Enter or Ctrl+C.
   The adjusted crop replaces the preview and the clipboard contents, and
   overwrites the same `--output` file if one was supplied.

Full-screen captures show Copy and Save As… but Adjust Region is disabled
because there is no region context.

### Overlay controls

| Input | Action |
| --- | --- |
| Drag with the primary button | Draw a new selection |
| Release the primary button | Capture immediately (normal capture flow) |
| Drag inside an existing selection | Move it (Adjust mode) |
| Drag one of the eight handles | Resize (Adjust mode) |
| Arrow keys / Shift+arrow | Nudge by 1 px / 10 px (Adjust mode) |
| Enter, Ctrl+C, or double-click | Copy the selection (Adjust mode) |
| Escape | Clear a selected rectangle; from Idle/Dragging, cancel |
| Right-click | Cancel |
| Click / Escape during countdown | Cancel the delayed capture |

A new drag is captured on release to match Windows Snipping Tool. Handles and
move are available while adjusting a finished capture, and while dragging a new
selection before release.

### Preview and clipboard

- **Copy** (Ctrl+C) copies the current crop again.
- **Save As…** (Ctrl+S) opens a native PNG save dialog with a timestamped name.
- **Adjust Region** reopens the frozen overlay for the last region.
- The crop is copied automatically after every capture and adjustment.
- Snipunix stays resident while it owns the clipboard: closing the main window
  hides it instead of exiting. Use the menu **Quit** or Ctrl+Q to exit and
  release the clipboard.

### Delay

`--delay 0..10` (and the toolbar spin button) starts a transparent fullscreen
countdown on the primary monitor. The countdown is destroyed before the portal
request, so it cannot appear in the screenshot. Clicking the countdown or
pressing Escape cancels without taking a screenshot.

## Portal permissions

The first screenshot request may show a desktop permission dialog. On GNOME,
the persisted decision is also visible under **Settings → Privacy →
Screenshots**. If the request is denied, Snipunix shows a typed error instead of
panicking:

- `PermissionDenied` / `PermissionDeniedWithDetail`
- `Cancelled` / `CancelledWithDetail`
- `PortalUnavailable`
- `Timeout` (15 seconds)

Snipunix hides its own window before requesting the capture and waits 180 ms so
the compositor removes it; the app window does not appear in the screenshot.

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
which is outside M1.

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

M1 was built and installed as version 0.2.0. A sandboxed region drag produced a
600×400 PNG. Stop any host `snipunix` process before launching the Flatpak: both
use the same application ID, so the host instance would receive the activation.

## Repository layout

```text
crates/snipunix-core      Geometry, monitor layout, coordinate mapping,
                          selection state machine (move/resize/nudge), RGBA crop
crates/snipunix-capture   Portal screenshot backend, URI handling, PNG decode,
                          portal probe
crates/snipunix-overlay   GDK monitor discovery, fullscreen overlay windows,
                          GSK renderer, handles, magnifier, countdown
crates/snipunix-app       CLI, libadwaita shell, preview bar, clipboard,
                          Save As, Adjust Region, shortcuts
data/                     Desktop entry, AppStream metadata, icon
flatpak/                  Manifest and Cargo source generator
docs/                     Architecture, dependencies, test evidence
```

## Known limitations

- **One monitor was available for live testing.** Per-monitor windows and
  virtual-desktop coordinates are implemented, but multi-monitor and mixed-DPI
  behaviour were not exercised on hardware. Do not treat them as validated.
- The portal does not report monitor geometry. M1 derives one uniform scale per
  axis from the GDK bounding box and the image size. Mixed-DPI desktops can
  therefore crop incorrectly.
- Monitor hotplug cancels the overlay in code, but hotplug could not be
  triggered for evidence.
- Window mode (snap-to-content) is not implemented; M1 supports region and
  screen capture only.
- The countdown is shown on the primary monitor only.
- The clipboard is owned by the process. Quitting Snipunix releases the
  clipboard; closing the window only hides it.
- A second `--capture region` launch while Snipunix is already running activates
  the existing window instead of starting a new capture; the arguments of the
  second process are not forwarded. Headless `--capture screen --output` is
  unaffected because it does not start GTK.
- Global shortcuts stop working when the app process exits. Registration is
  session-scoped; there is no background service or autostart entry.
- The Flatpak build was validated on this host; other runtimes and distributions
  are untested.
- X11 is only a fallback socket in the manifest. M1 was tested on Wayland; X11
  interaction was not validated on a real session.
- Automated tests cover pure logic and real host interaction through `ydotool`;
  they do not cover every compositor/portal version.

## Documentation

- [M1 architecture](docs/M1_ARCHITECTURE.md)
- [M1 test evidence](docs/M1_TEST_EVIDENCE.md)
- [M1 completion report](M1_COMPLETION_REPORT.md)
- [M0 architecture](docs/M0_ARCHITECTURE.md) and
  [M0 test evidence](docs/M0_TEST_EVIDENCE.md) (historical)
- [Dependencies and licenses](docs/DEPENDENCIES.md)
- Phase 0 research reports remain in `research/`.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
