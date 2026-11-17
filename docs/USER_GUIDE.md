# Snipunix user guide

Snipunix captures a screen or a region, lets you annotate the result, and
copies or saves it. It also records the screen.

Everything happens on your machine. There is no account, no telemetry, and no
upload.

## Contents

- [Install](#install)
- [Take a screenshot](#take-a-screenshot)
- [Adjust the region](#adjust-the-region)
- [Annotate](#annotate)
- [Recognise and redact text](#recognise-and-redact-text)
- [Record the screen](#record-the-screen)
- [Settings](#settings)
- [Global shortcuts](#global-shortcuts)
- [Troubleshooting](#troubleshooting)

## Install

### Flatpak

```sh
flatpak install flathub io.github.alokverma9.snipunix
flatpak run io.github.alokverma9.snipunix
```

The Flatpak bundles the OCR models, so no download is needed.

### Debian and Ubuntu

```sh
sudo apt install ./snipunix_0.6.0_amd64.deb
snipunix
```

A host build does **not** bundle the OCR models. Install them once with
`snipunix --download-ocr-models`, or point the application at a directory that
already contains them.

### From source

```sh
cargo build --release
./target/release/snipunix
```

See the [README](../README.md) for the build dependencies.

## Take a screenshot

1. Start Snipunix, or press your global shortcut.
2. Choose **Capture Region** or **Capture Screen** in the window, or in the
   tray menu if your desktop shows a tray icon.
3. If you use **Capture Region**, the screen freezes and dims. Drag a
   rectangle to select the area you want.
4. Release the mouse. The capture is copied to the clipboard.

The delay control next to the capture buttons waits up to ten seconds before
the screen freezes, which is useful for capturing a menu that only appears on
hover.

The **Adjust Region** button reopens the selection overlay with your last
rectangle so you can correct it without taking a new screenshot.

## Adjust the region

While the selection overlay is open:

| Action | Result |
| --- | --- |
| Drag | Create a new selection |
| Drag inside the selection | Move the selection |
| Drag a corner or edge handle | Resize the selection |
| Arrow keys | Move the selection one pixel |
| Shift plus an arrow | Move the selection ten pixels |
| `Enter` or `Ctrl+C` | Confirm the selection |
| `Escape` | Cancel and leave nothing behind |

The smallest selection is 8×8 logical pixels. A dimension badge shows the
size of the selection, and the magnifier shows the pixels under the pointer.

## Annotate

Press **Annotate** to open the editor on the latest capture, or `Ctrl+E` in
the main window.

The left rail holds the tools:

| Tool | What it does |
| --- | --- |
| Pen | Freehand strokes |
| Highlighter | Translucent broad strokes |
| Arrow | A line with an arrowhead |
| Rect | A rectangle outline |
| Ellipse | An ellipse outline |
| Text | A UTF-8 text box, including emoji and non-Latin scripts |
| Eraser | Removes annotations you drag over |

The top bar holds undo, redo, zoom out, zoom in, fit, reset crop, and the
copy and export buttons. The properties panel on the right sets the stroke
colour, the stroke width, and the text size.

- **Zoom** with the scroll wheel, **pan** with a middle-click drag.
- **Undo** is `Ctrl+Z`, **redo** is `Ctrl+Shift+Z`. Every gesture is one step.
- **Crop** is applied from the crop control and is undoable.
- **Copy Annotated** puts the annotated result on the clipboard.
- **Export…** writes a PNG, JPEG, or WebP file. JPEG has a quality control.

Annotations stay editable until you export. Exporting always writes the
current state of the document.

## Recognise and redact text

The editor can read the text in the current crop on your own machine.

1. Open the editor and press **Extract Text**.
2. The result appears in a dialog with the recognised text.
3. **Copy All** puts the text on the clipboard.
4. **Remove line breaks** joins the lines into flowing text.

Snipunix also finds email addresses and phone numbers. **Redact detected**
covers them with opaque black rectangles. The pixels really are removed from
the exported file, not merely covered.

The heuristic is not a guarantee. Use manual rectangles for anything else.

## Record the screen

Press **Record Screen** or **Record Region**, or use the tray menu.

1. The desktop asks which source to record. Choose a monitor.
2. The recording starts. A small window shows the elapsed time.
3. **Pause** and **Stop** are in that window.
4. The file is written to `Videos/Screen Recordings` unless you changed the
   folder in Settings.

Snipunix remembers the source you allowed, so the next recording starts
immediately. **Forget remembered source** in Settings brings the picker back.

Audio is off by default. Enable the microphone, the system output, or both in
Settings. Each is recorded separately and mixed into the file.

Recordings are H.264 in an MP4 container, or Matroska if you prefer it.

## Settings

Open Settings with `Ctrl+,` or from the menu.

| Page | What it holds |
| --- | --- |
| Capture | Default mode, delay, copy after capture, remember last region, include pointer |
| Shortcuts | Global shortcut registration and triggers |
| Files | Auto-save, save folder, filename template, format, JPEG quality |
| Recording | Output folder, frame rate, bitrate, pointer, audio, remembered source |
| Text Recognition | Auto-copy, remove line breaks, model folder, model download |
| Appearance | Magnifier, dim strength, reduced motion |
| About | Version, licence, privacy summary, crash reports, diagnostics export |

Settings are written immediately when you change them. You never have to
confirm a dialog to keep a setting.

### Diagnostics

**Save crash reports** is off by default. When you turn it on, Snipunix writes
a small file to `$XDG_STATE_HOME/snipunix/crashes` if the application
panics. The file contains the panic message, the source location, and a
backtrace. It never contains screen content or recognised text.

**Export diagnostics** writes a report about your installation to a file you
choose. Attach it to a bug report. The report describes the session type, the
portal versions, the available encoders, and your settings. It contains no
screen content, no recognised text, and no file names from your directories.

## Global shortcuts

Snipunix asks the desktop's GlobalShortcuts portal for two shortcuts. The
default triggers are `Ctrl+Shift+S` for a region and `Ctrl+Shift+A` for the
whole screen. You can change them in Settings.

The portal may hand back a different trigger than the one you asked for. The
**Registered triggers** row shows what was actually accepted.

If the portal refuses the request, Snipunix shows the exact manual steps and
never claims success. You can always bind a shortcut yourself:

- GNOME: Settings → Keyboard → View and Customize Shortcuts → Custom Shortcuts
- KDE Plasma: System Settings → Shortcuts → Custom Shortcuts

Then add `snipunix --capture region` and `snipunix --capture screen`.

Registration lasts for the session. There is no background service, so the
shortcuts stop working when Snipunix exits.

## Troubleshooting

**The screenshot portal is unavailable.**
Install `xdg-desktop-portal` and a desktop backend such as
`xdg-desktop-portal-gnome`. `snipunix --diagnostics` prints what the portal
reports.

**The desktop asks for permission every time.**
The permission is granted to the application, not to the portal. Check the
privacy panel of your desktop for the Screenshot entry.

**The selection is offset from the pointer.**
Mixed-DPI desktops with a uniform scale cannot be described exactly by the
portal. See [PLATFORM_LIMITATIONS.md](PLATFORM_LIMITATIONS.md).

**The clipboard is empty after closing the window.**
Snipunix keeps running so the copied image stays on the clipboard. Use
**Quit** to exit; quitting releases the clipboard.

**OCR says the model is missing.**
Run `snipunix --download-ocr-models`, or install the models into the folder
shown in Settings.

**The recording is empty or fails to start.**
Run `snipunix --record-selftest` to check the encoders. You need
`gstreamer1.0-plugins-ugly` for the software H.264 encoder or
`gstreamer1.0-vaapi` for the hardware one.

For anything else, run `snipunix --diagnostics` and include the output in your
report.
