# Snipunix

**Screenshot. Annotate. Share.**

Snipunix is a Linux screenshot tool built for GNOME Wayland. This repository is
the **M2 prototype**: portal capture, a frozen-image region overlay with
move/resize handles, a magnifier, a delay countdown, clipboard copy, Save As,
Adjust Region, and a non-destructive annotation editor with PNG/JPEG/WebP
export. OCR, secure redaction, sharing, recording, and cloud features are not
part of M2.

## Status

| Area | M2 state |
| --- | --- |
| Full-screen capture | Works through `org.freedesktop.portal.Screenshot` v2 |
| Headless CLI capture | `snipunix --capture screen --output PATH` |
| Region selection | Frozen desktop, one fullscreen window per monitor, capture on mouse release |
| Move / resize | Eight handles, inside-drag move, keyboard nudge, minimum 8×8 logical px |
| Magnifier | 8× nearest-neighbour while dragging or resizing |
| Delay | `--delay 0..10` and a GUI spin button; countdown can be cancelled |
| Clipboard | Raw crop copied automatically on capture; Copy Annotated in the editor |
| Preview actions | Copy, Save As…, Annotate, Adjust Region |
| Adjust | Reopens the overlay with the previous rectangle and handles; Enter / Ctrl+C confirms |
| Cancel | Escape, right-click |
| Saving | `--output PATH`, a GUI Save As dialog, and PNG/JPEG/WebP export |
| Annotation editor | Pen, highlighter, line, arrow, rectangle, ellipse, text, eraser, crop |
| Selection | Move/resize with eight handles; edit text with a double-click |
| Undo / redo | Every edit, including crop and property changes; bounded history |
| Rendering | One tiny-skia + cosmic-text pipeline shared by screen and export |
| Resident window | Closing the window hides it while Snipunix owns the clipboard; Quit exits |
| Global shortcuts | Prototype, registered at runtime through the GlobalShortcuts portal |
| Flatpak | Manifest, offline source list, and a verified 0.3.0 build |
| OCR / QR / secure redaction / recording / audio / cloud | Not implemented (later milestones) |

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
cargo fmt --all -- --check
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

M2 has **84 passing tests**: 40 annotation, 29 core, 8 capture, 5 app, 2
overlay. See [docs/M2_TEST_EVIDENCE.md](docs/M2_TEST_EVIDENCE.md).

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
   **Copy**, **Save As…**, **Annotate**, and **Adjust Region**.
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
- **Annotate** opens the non-destructive editor (see below).
- **Adjust Region** reopens the frozen overlay for the last region.
- The crop is copied automatically after every capture and adjustment.
- Snipunix stays resident while it owns the clipboard: closing the main window
  hides it instead of exiting. Use the menu **Quit** or Ctrl+Q to exit and
  release the clipboard.

### Annotation editor

Choose **Annotate** from the preview bar, or press `Ctrl+E`. The captured image
is treated as an immutable base; annotations and the crop are stored separately,
so nothing is baked into the original.

Tool rail (top to bottom): Move, Pen, Highlighter, Line, Arrow, Rectangle,
Ellipse, Text, Eraser, Crop.

| Input | Action |
| --- | --- |
| Drag with Pen / Highlighter | Draw a freehand stroke (one undo step per stroke) |
| Drag with Line / Arrow / Rectangle / Ellipse | Draw the shape |
| Click with Text | Open the multiline UTF-8 text dialog |
| Double-click text with Move | Edit existing text |
| Drag an object with Move | Move it |
| Drag one of the eight handles | Resize it |
| Click with Eraser | Remove the topmost object |
| Drag with Eraser | Remove every touched object as one undo step |
| Drag with Crop, then Apply Crop | Crop the output rectangle |
| Scroll / middle-drag | Zoom about the cursor / pan |
| `Ctrl+Z` / `Ctrl+Shift+Z` | Undo / redo |

The right panel sets stroke colour (eight presets plus a custom colour), stroke
width, text size, highlighter opacity (Light/Medium/Strong), and the export
format. Property changes apply to the current selection when compatible and are
undoable.

**Copy Annotated** publishes the composed result to the clipboard. **Export…**
opens a native save dialog for PNG, JPEG, or WebP with a timestamped name; if
the target exists, Snipunix asks before replacing it. Exports contain no capture
metadata. **Close Editor** returns to the preview bar, and a new capture closes
any open editor.

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
which is outside the current milestones.

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

M2 was built and installed as version 0.3.0 (commit
`61fa3a29482ab8b269a3a44071087a43e6f48d8a61d59e91d3db07836b9272e2`) from 433
generated Cargo sources. Headless capture, the annotation editor, Copy
Annotated, and PNG export were exercised inside the sandbox. Stop any host
`snipunix` process before launching the Flatpak: both use the same application
ID, so the host instance would receive the activation.

## Repository layout

```text
crates/snipunix-core      Geometry, monitor layout, coordinate mapping,
                          selection state machine (move/resize/nudge), RGBA crop
crates/snipunix-capture   Portal screenshot backend, URI handling, PNG decode,
                          portal probe
crates/snipunix-annotate  Document model, geometry, undo history, view
                          transform, shared renderer, PNG/JPEG/WebP export
crates/snipunix-overlay   GDK monitor discovery, fullscreen overlay windows,
                          GSK renderer, handles, magnifier, countdown
crates/snipunix-app       CLI, libadwaita shell, preview bar, clipboard,
                          Save As, Adjust Region, annotation editor, shortcuts
data/                     Desktop entry, AppStream metadata, icon
flatpak/                  Manifest and Cargo source generator
docs/                     Architecture, dependencies, test evidence
```

## Known limitations

- **One monitor was available for live testing.** Per-monitor windows and
  virtual-desktop coordinates are implemented, but multi-monitor and mixed-DPI
  behaviour were not exercised on hardware. Do not treat them as validated.
- The portal does not report monitor geometry. Snipunix derives one uniform
  scale per axis from the GDK bounding box and the image size. Mixed-DPI desktops
  can therefore crop incorrectly.
- Monitor hotplug cancels the overlay in code, but hotplug could not be
  triggered for evidence.
- Window mode (snap-to-content) is not implemented; Snipunix supports region
  and screen capture only.
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
- X11 is only a fallback socket in the manifest. M2 was tested on Wayland; X11
  interaction was not validated on a real session.
- WebP export is lossless because `image 0.25.10` only exposes the lossless
  WebP encoder; there is no WebP quality control.
- JPEG quality is fixed at 90 in the UI; the encoder itself accepts 1–100.
- Annotation text uses the system font resolved by fontconfig. Screen and export
  share the resolved layout, but the exact typeface can differ across machines.
- Crop is rectangular only; there is no freeform, rotated, or content-aware
  crop, and no secure redaction/pixelation in this milestone.
- Multi-monitor and mixed-DPI annotation/export were not certified.
- Automated tests cover pure logic and real host interaction through `ydotool`;
  they do not cover every compositor/portal version.

## Documentation

- [M2 architecture](docs/M2_ARCHITECTURE.md)
- [M2 implementation plan](docs/M2_IMPLEMENTATION_PLAN.md)
- [M2 test evidence](docs/M2_TEST_EVIDENCE.md)
- [M2 completion report](M2_COMPLETION_REPORT.md)
- [M1 architecture](docs/M1_ARCHITECTURE.md) and
  [M1 test evidence](docs/M1_TEST_EVIDENCE.md) (historical)
- [M0 architecture](docs/M0_ARCHITECTURE.md) and
  [M0 test evidence](docs/M0_TEST_EVIDENCE.md) (historical)
- [Dependencies and licenses](docs/DEPENDENCIES.md)
- Phase 0 research reports remain in `research/`.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
