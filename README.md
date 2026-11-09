# Snipunix

**Screenshot. Annotate. Share.**

Snipunix is a Linux screenshot tool built for GNOME Wayland. This repository is
the **M3 product**: portal capture, a frozen-image region overlay with
move/resize handles, a magnifier, a delay countdown, clipboard copy, Save As,
Adjust Region, a non-destructive annotation editor with PNG/JPEG/WebP export,
local OCR with text extraction and opaque redaction, persisted settings,
rebindable global shortcuts, a best-effort tray, first-run onboarding, and
Flatpak/deb packages.

Everything stays on the device. OCR uses a local ONNX model, and no screenshot
or recognised text is sent anywhere. The only network operation is an explicit,
user-initiated OCR model download for host builds that do not already have the
models installed.

## Status

| Area | M3 state |
| --- | --- |
| Full-screen capture | Works through `org.freedesktop.portal.Screenshot` v2 |
| Headless CLI capture | `snipunix --capture screen --output PATH` |
| Region selection | Frozen desktop, one fullscreen window per monitor, capture on mouse release |
| Move / resize | Eight handles, inside-drag move, keyboard nudge, minimum 8×8 logical px |
| Magnifier | 8× nearest-neighbour; configurable on/off and dim strength |
| Delay | `--delay 0..10`, GUI spin button, and persisted setting; cancellable countdown |
| Clipboard | Raw crop copied automatically on capture; Copy Annotated in the editor |
| Preview actions | Copy, Save As…, Annotate, Adjust Region |
| Annotation editor | Pen, highlighter, line, arrow, rectangle, ellipse, text, eraser, crop |
| Local OCR | `ocrs`/`rten` engine, bundled pinned models, line/word boxes, email/phone detection |
| OCR editor flow | Extract Text, selectable result, line-break and auto-copy options, Copy All, Redact detected |
| Redaction | Opaque black annotation, compound undo, destructive pixels in exports |
| Settings | Capture, Shortcuts, Files, Text Recognition, Appearance, About; saved immediately |
| Onboarding | First-run permission priming, shortcut setup, save folder, privacy summary |
| Global shortcuts | Configured triggers, returned-description display, rebind, failure guidance |
| Tray | Best-effort StatusNotifierItem: capture, show, settings, quit |
| Packaging | Flatpak 0.4.0 and `snipunix_0.4.0_amd64.deb`, both with OCR models |
| X11 capture / recording / audio / cloud / AI / telemetry / background service | Not implemented (out of scope) |

Tested on Ubuntu 26.04, GNOME 50.1, Wayland, one 1920×1080 monitor. See
[Known limitations](#known-limitations) before relying on any behaviour.

## Requirements

- Rust 1.85 or newer (edition 2024)
- GTK 4 and libadwaita development files
- `xdg-desktop-portal` with the GNOME backend
- Flatpak and `flatpak-builder` only for the Flatpak build
- `dpkg-deb` only for the deb build

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

M3 has **105 passing tests**: 43 annotation, 29 core, 8 capture, 6 app, 2 overlay,
9 OCR, and 8 settings. The two model-gated OCR tests need the pinned models:

```sh
SNIPUNIX_OCR_TEST_MODELS=/path/to/models cargo test --workspace
```

See [docs/M3_TEST_EVIDENCE.md](docs/M3_TEST_EVIDENCE.md) for commands, outputs,
and the verification gaps.

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

Recognise text in an existing image and print it:

```sh
./target/debug/snipunix --ocr /tmp/snipunix-screen.png \
    --ocr-models /path/to/models
```

Join recognised lines with spaces:

```sh
./target/debug/snipunix --ocr /tmp/snipunix-screen.png \
    --ocr-models /path/to/models --remove-line-breaks
```

Download and verify the pinned models into a directory:

```sh
./target/debug/snipunix --download-ocr-models --ocr-models /path/to/models
```

Show portal capabilities:

```sh
./target/debug/snipunix --portal-info
```

Register the configured global shortcuts:

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
- **Save As…** (Ctrl+S) opens a native save dialog with a timestamped name and
  the configured format.
- **Annotate** opens the non-destructive editor.
- **Adjust Region** reopens the frozen overlay for the last region.
- The crop is copied automatically after every capture and adjustment when
  **Copy after capture** is enabled.
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

## Local OCR and redaction

**Extract Text** renders the current composed crop through the same tiny-skia
pipeline used for export, including annotations and redactions, then runs OCR on
a worker thread so the window stays responsive.

The result dialog shows:

- selectable monospace text and a line count;
- every recognised line box drawn on the canvas, clickable to select that line;
- **Remove line breaks** and **Auto-copy** toggles;
- **Copy All**;
- **Redact detected**;
- **Close**.

`Redact detected` finds email addresses and phone numbers and covers them with
opaque black boxes. The boxes are ordinary `Redaction` annotations: they move,
resize, hit-test, undo, and export with the rest of the document. A multi-box
redaction is one undo step. Re-running OCR on the redacted result finds no
sensitive text.

OCR model discovery checks, in order: the explicit `--ocr-models` directory,
`SNIPUNIX_OCR_MODEL_DIR`, the settings model directory, XDG data directories,
`/usr/share/snipunix/models`, and `/app/share/snipunix/models`. Flatpak and deb
install both pinned files, so installed builds need no download.

The model URLs and SHA-256 digests are constants:

| File | SHA-256 |
| --- | --- |
| `text-detection.onnx` | `a917b23dbd9524b465df7e922641b3ff2981623df4ded5a0234004ef2fee7cfe` |
| `text-recognition.onnx` | `86c145c2edb96c8caed5b1ebb8f44d706408922211451c309c625157dd6061c5` |

## Settings and onboarding

Open **Settings…** from the menu or press `Ctrl+,`. The preferences dialog has
six pages:

- **Capture** — default mode, delay, copy after capture, remember last region.
- **Shortcuts** — register on startup, region/screen triggers, Register Now, and
  the trigger descriptions the portal actually returned.
- **Files** — auto-save, save folder, filename template, format, JPEG quality.
- **Text Recognition** — auto-copy, remove line breaks, model folder, Download
  Models.
- **Appearance** — magnifier, dim strength, reduced motion.
- **About** — version, app ID, license, and the local-processing statement.

Every control saves immediately. Numeric values are clamped before they are
written, and the file is written atomically.

Location: `$XDG_CONFIG_HOME/snipunix/settings.toml` (normally
`~/.config/snipunix/settings.toml`). A missing file means defaults; invalid TOML
is reported as a typed error and the app falls back to defaults instead of
crashing.

On first launch, a welcome dialog explains the screenshot portal, offers to
register global shortcuts, lets the user choose a save folder, and repeats the
privacy guarantee. **Skip**, **Get Started**, and closing the window all mark
onboarding complete so it does not return on every launch.

## Global shortcuts

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

The default triggers are:

- `Ctrl+Shift+S` — capture a region
- `Ctrl+Shift+A` — capture the full screen

The portal may remap a request. Snipunix displays and persists the
`trigger_description` the portal returned rather than claiming the requested
binding was accepted. On refusal or cancellation it opens a fallback dialog with
the exact GNOME/KDE paths, manual commands, and an **Open Keyboard Settings**
action.

**The shortcuts are session-scoped:** closing Snipunix stops the listener, so
they only work while the application is running. Persistent shortcuts that
launch a closed application need a background/autostart service, which is
outside the current milestones. Inside Flatpak the app ID comes from the
sandbox; host registration is skipped.

## Tray

Snipunix registers a StatusNotifierItem with Capture Region, Capture Screen,
Show Snipunix, Settings…, and Quit. On default GNOME the icon only appears when
an AppIndicator/StatusNotifier extension is enabled.

The tray is best-effort by design. If the service or watcher is unavailable, the
failure is logged at debug level and the app continues without a toast or modal;
every tray action also exists in the window and CLI. Inside Flatpak the
well-known D-Bus name is disabled so the sandbox owns it normally.

## Delay

`--delay 0..10` and the Capture settings page start a transparent fullscreen
countdown on the primary monitor. The countdown is destroyed before the portal
request, so it cannot appear in the screenshot. Clicking the countdown or
pressing Escape cancels without taking a screenshot.

## Portal permissions

The first screenshot request may show a desktop permission dialog. On GNOME,
the persisted decision is also visible under **Settings → Privacy →
Screenshots**. If the request is denied, Snipunix shows a typed error and a
privacy-settings dialog with an action that opens the relevant panel:

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

## Packaging

### Flatpak

The manifest is `flatpak/io.github.alokverma9.snipunix.json`. It has two modules:
`snipunix-ocr-models` downloads the two ONNX files with pinned SHA-256 digests,
and `snipunix` builds offline from `flatpak/cargo-sources.json`.

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

M3 was built and installed as version 0.4.0 (commit
`9a4200f52d6ab6ba065210622c2237ffff2333c79be767e6d08390605f210958`) from 601
generated Cargo sources. The models are at `/app/share/snipunix/models`, and
sandboxed OCR runs without a model-path override or network access.

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

Stop any host `snipunix` process before launching the Flatpak: both use the same
application ID, so the host instance would receive the activation.

### deb

```sh
packaging/build-deb.sh
```

The script builds a release binary, uses models from `SNIPUNIX_MODELS` or
downloads/verifies them with `snipunix --download-ocr-models`, stages the
binary, desktop entry, metainfo, icon, copyright, and both model files, then
calls `dpkg-deb`.

```text
dist/snipunix_0.4.0_amd64.deb
Depends:    libgtk-4-1, libadwaita-1-0, xdg-desktop-portal
Recommends: xdg-desktop-portal-gnome
Models:     /usr/share/snipunix/models
```

## Repository layout

```text
crates/snipunix-core      Geometry, monitor layout, coordinate mapping,
                          selection state machine, RGBA crop
crates/snipunix-capture   Portal screenshot backend, URI handling, PNG decode,
                          portal probe
crates/snipunix-annotate  Document model, geometry, undo history, view
                          transform, shared renderer, redaction, PNG/JPEG/WebP
crates/snipunix-settings  Settings model, XDG path, TOML, clamping, atomic save
crates/snipunix-ocr       ocrs/rten pipeline, text/box model, PII detection,
                          model discovery, verified download, pixel redaction
crates/snipunix-overlay   GDK monitor discovery, fullscreen overlay windows,
                          GSK renderer, handles, magnifier, countdown
crates/snipunix-app       CLI, libadwaita shell, preview bar, clipboard,
                          annotation editor, OCR dialog, settings UI,
                          onboarding, shortcuts, tray
data/                     Desktop entry, AppStream metadata, icon
flatpak/                  Manifest and Cargo source generator
packaging/                Reproducible deb builder
docs/                     Architecture, dependencies, test evidence
```

## Known limitations

- **One monitor was available for live testing.** Per-monitor windows and
  virtual-desktop coordinates are implemented, but multi-monitor and mixed-DPI
  behaviour were not exercised on hardware. OCR and redaction box mapping were
  not certified across mixed-DPI monitors either.
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
- The tray icon requires an AppIndicator/StatusNotifier extension on GNOME. The
  app works without it, but the icon is not visible on a stock shell.
- The bundled OCR models are English text models. Other languages would need
  different models installed in a searched directory; the app does not ship a
  language picker.
- `Redact detected` covers email addresses and phone numbers found by the
  heuristic. It is not a guarantee that every sensitive string was found; use
  manual rectangles for anything else.
- Debug builds run OCR slowly on large screenshots. The release binary is the
  supported path for real captures; the UI stays responsive because OCR runs on
  a worker thread.
- A host build without bundled models needs an explicit `--download-ocr-models`
  or an installed model directory. The app never downloads automatically from
  the capture or OCR flow.
- GUI automation coverage is partial: `ydotool` pointer clicks were unreliable
  on the test GNOME session, so the OCR dialog, settings tabs, toggles, and
  onboarding completion were verified by rendering and unit/model tests rather
  than an automated end-to-end click path. See the test evidence for details.
- The Flatpak and deb were validated on this host only. The deb was inspected
  from a staging root, not installed into the live system.
- X11 is only a fallback socket in the manifest. M3 was tested on Wayland; X11
  interaction was not validated on a real session.
- WebP export is lossless because `image 0.25.10` only exposes the lossless
  WebP encoder; there is no WebP quality control.
- Annotation text uses the system font resolved by fontconfig. Screen and export
  share the resolved layout, but the exact typeface can differ across machines.
- Crop is rectangular only; there is no freeform, rotated, or content-aware
  crop. Redaction is an opaque rectangle, not reversible blur or pixelation.
- Automated tests cover pure logic, the real OCR engine, and package artifacts;
  they do not cover every compositor/portal version.

## Documentation

- [M3 architecture](docs/M3_ARCHITECTURE.md)
- [M3 implementation plan](docs/M3_IMPLEMENTATION_PLAN.md)
- [M3 test evidence](docs/M3_TEST_EVIDENCE.md)
- [M3 completion report](M3_COMPLETION_REPORT.md)
- [M2 architecture](docs/M2_ARCHITECTURE.md),
  [plan](docs/M2_IMPLEMENTATION_PLAN.md), and
  [test evidence](docs/M2_TEST_EVIDENCE.md) (historical)
- [M1 architecture](docs/M1_ARCHITECTURE.md) and
  [M1 test evidence](docs/M1_TEST_EVIDENCE.md) (historical)
- [M0 architecture](docs/M0_ARCHITECTURE.md) and
  [M0 test evidence](docs/M0_TEST_EVIDENCE.md) (historical)
- [Dependencies and licenses](docs/DEPENDENCIES.md)
- Phase 0 research reports remain in `research/`.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
