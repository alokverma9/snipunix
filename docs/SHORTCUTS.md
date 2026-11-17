# Keyboard shortcuts

Three kinds of shortcut appear in Snipunix:

1. **Application shortcuts** work whenever the main window has the focus.
2. **Global shortcuts** work anywhere, once the desktop's GlobalShortcuts
   portal has accepted them.
3. **Overlay shortcuts** work while the selection overlay is open.

## Application shortcuts

These are registered on the application window and are active while Snipunix
has the focus.

| Shortcut | Action |
| --- | --- |
| `Ctrl+Shift+S` | Capture a region |
| `Ctrl+Shift+A` | Capture the whole screen |
| `Ctrl+Shift+W` | Capture a window (X11 sessions only) |
| `Ctrl+Shift+R` | Record the screen |
| `Ctrl+Shift+Alt+R` | Record a region |
| `Ctrl+E` | Open the annotation editor |
| `Ctrl+Z` | Undo in the editor |
| `Ctrl+Shift+Z` | Redo in the editor |
| `Ctrl+C` | Copy the capture or the annotated result |
| `Ctrl+S` | Save the capture as a file |
| `Ctrl+,` | Open Settings |
| `Ctrl+Shift+O` | Extract text from the current crop |
| `Ctrl+Q` | Quit |

## Global shortcuts

Global shortcuts are requested through the desktop's GlobalShortcuts portal.
They are registered for the session only: they stop working when Snipunix
exits, because there is no background service.

| Default trigger | Action |
| --- | --- |
| `Ctrl+Shift+S` | Capture a region |
| `Ctrl+Shift+A` | Capture the whole screen |

The triggers are configurable in Settings → Shortcuts. The portal uses its own
syntax, for example `CTRL+SHIFT+s`.

The portal may accept a trigger that differs from the one you asked for, and
it may refuse the request entirely. Settings shows the trigger that was
actually accepted under **Registered triggers**. When the request fails,
Snipunix explains how to bind a manual shortcut instead and never reports a
success it did not get.

Manual bindings that work with any desktop:

```
snipunix --capture region
snipunix --capture screen
```

## Overlay shortcuts

These apply while the frozen selection overlay is on screen.

| Key | Action |
| --- | --- |
| `Escape` | Cancel. Nothing is copied and no window is left behind. |
| `Enter` | Confirm the selection and capture it |
| `Ctrl+C` | Confirm the selection and capture it |
| Arrow keys | Move the selection one pixel |
| `Shift` plus an arrow | Move the selection ten pixels |
| Mouse drag | Create a new selection |
| Drag inside the selection | Move the selection |
| Drag a handle | Resize the selection |
| Right-click | Cancel |

## Countdown shortcuts

While the delay countdown is running:

| Key | Action |
| --- | --- |
| `Escape` | Cancel the countdown |

## Editor shortcuts

The annotation editor inherits the application shortcuts. In addition:

| Shortcut | Action |
| --- | --- |
| Scroll wheel | Zoom |
| Middle-button drag | Pan |
| `Ctrl+Z` | Undo one gesture |
| `Ctrl+Shift+Z` | Redo one gesture |

## Command line

The command line is the escape hatch when a shortcut is not available.

| Command | Effect |
| --- | --- |
| `snipunix` | Start the application window |
| `snipunix --capture screen --output out.png` | Capture without opening the window |
| `snipunix --capture region` | Capture a region interactively |
| `snipunix --capture window --window 0x400001` | Capture one X11 window |
| `snipunix --record screen --record-duration 30 --record-output demo.mp4` | Record for 30 seconds |
| `snipunix --record region --record-region 100,100,1280,720` | Record a region |
| `snipunix --ocr shot.png` | Print the text in an image |
| `snipunix --list-windows` | List the capturable X11 windows |
| `snipunix --diagnostics` | Print a diagnostics report |
| `snipunix --diagnostics-output report.txt` | Write a diagnostics report to a file |
| `snipunix --portal-info` | Print the Screenshot portal version |
| `snipunix --download-ocr-models` | Download and verify the OCR models |
| `snipunix --help` | Print the full list of options |
