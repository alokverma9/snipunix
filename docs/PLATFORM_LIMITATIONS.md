# Platform limitations

Snipunix is developed and tested on GNOME Wayland. Other desktops work to
different degrees. This document is honest about where the boundaries are.

## Tested configuration

| | |
| --- | --- |
| Distribution | Ubuntu 26.04 |
| Desktop | GNOME 50.1 |
| Session | Wayland |
| Monitors | one, 1920×1080 |
| Scale | 1.0 |
| GStreamer | 1.28.2 with `x264enc` |
| Rust | 1.98 |

Everything else in this document is *implemented but not verified on
hardware*, or *not implemented at all*. The two are called out separately.

## Desktop environments

### GNOME Wayland

Fully supported. The Screenshot portal, the ScreenCast portal, and the
GlobalShortcuts portal are all available in the GNOME desktop portal
backend.

### KDE Plasma Wayland

Expected to work. The portals are implemented by `xdg-desktop-portal-kde`.
This was **not verified on a KDE session** during development. The
differences that are likely to show up:

- The Screenshot portal may not report the same `AvailableTargets` bits.
- The ScreenCast portal may present a different source picker.
- The GlobalShortcuts portal may be missing entirely, in which case the
  manual binding instructions are the fallback.

### sway and other wlroots compositors

Expected to work. The portals are compositor-independent, but a wlroots
compositor has no `xdg-desktop-portal-gnome`; you need
`xdg-desktop-portal-wlr` and its Screenshot backend, which is optional in
some distributions.

**Not verified.**

### X11

X11 is a fallback. The capture path works through the X11 backend
(`--backend x11`), including root-window capture with XShm and per-window
capture. The overlay on X11 uses an override-redirect window.

**Not verified on a real X11 session.** The X11 code is exercised by unit
tests and by the `X11Capture` diagnostics, but the interactive overlay has
not been run against a live X server with a compositor.

## Monitors and scaling

This is the weakest area.

The Screenshot portal does not report monitor geometry. Snipunix derives one
uniform scale per axis by comparing the GDK bounding box of the desktop with
the size of the captured image. On a desktop with mixed-DPI monitors, that
single scale is wrong for at least one of the monitors.

| Configuration | Status |
| --- | --- |
| One monitor, scale 1.0 | Verified |
| One monitor, scale 2.0 | Implemented, not verified |
| Two monitors, same scale | Implemented, not verified |
| Two monitors, mixed scale | Known to be incorrect; the portal cannot express it |
| Fractional scale (1.25, 1.5) | Implemented, not verified |
| Rotated monitors | Partially implemented |
| Monitor hotplug during a capture | Cancels the overlay; not verified |

The overlay draws one full-screen window per monitor and maps the frozen
image onto them. The mapping code is covered by property tests, but the
end-to-end behaviour on a mixed-DPI desktop has not been observed.

## Capture targets

| Target | Status |
| --- | --- |
| Region | Verified |
| Full screen | Verified |
| X11 window | Implemented, not verified on a live session |
| Wayland window | Not implemented. The Screenshot portal does not offer it. |
| "Snap to content" window detection | Not implemented. The roadmap called for a heuristic; it was not built. |

## Recording

| Aspect | Status |
| --- | --- |
| Monitor recording | Verified |
| Region recording from the CLI | Verified |
| Region recording from the window | Implemented, not verified end to end |
| Microphone audio | Unit-tested only. No capture device was available. |
| System audio | Unit-tested only. No capture device was available. |
| Hardware encoding (`vah264enc`) | Preference-ordered but never measured |
| Software encoding (`x264enc`) | Verified at about 32 % of eight threads at 1080p30 |
| Pause and resume | Implemented, not verified with a real stream |
| Persisted source selection | Verified through the restore token |
| GIF export | Not implemented |
| A live preview while recording | Not implemented |

The recording indicator is a separate window, so it appears in the recording
itself.

A force-killed recording cannot finalise the container. The normal stop and
cancel paths are safe.

## Text recognition

| Aspect | Status |
| --- | --- |
| English text | Verified |
| Other languages | Not shipped. The models are English only and there is no language picker. |
| Small or rotated text | Recognised poorly; the model is a general-purpose detector |
| Reading order | Approximate. The model returns boxes, not a reading order. |
| Confidence | Not surfaced in the user interface |
| Redaction coverage | A heuristic for email addresses and phone numbers, not a guarantee |

Text is laid out with the system font resolved by fontconfig. The screen and
the export share the same layout, but the typeface can differ across
machines.

## The interface

| Aspect | Status |
| --- | --- |
| Keyboard-only operation | Implemented for the window, the overlay, and the editor. The settings pages rely on the libadwaita defaults. |
| Screen reader | Labels are set on the overlay and the countdown. The rest is not certified. |
| High contrast | Follows the GTK theme. Not certified. |
| 200 % text scaling | The badge and the toolbar scale. Not certified at every window size. |
| Reduced motion | A setting that disables overlay animations |

## The tray

The tray is best-effort. It needs a StatusNotifierItem host, which GNOME only
provides through the AppIndicator extension. On a stock GNOME shell the
application starts, captures, records, and quits without a tray and shows no
error.

## The clipboard

Snipunix owns the clipboard while it runs. Closing the window hides the
window but keeps the process alive so the image stays available. Quitting
releases the clipboard. The clipboard survives a 10-second idle period and
the application window closing; it does not survive quitting the process.

## Packaging

| Package | Status |
| --- | --- |
| Flatpak | Built and run on this host. The portal permissions are exercised from M0 onwards. |
| deb | Built and inspected from a staging root. It was not installed into the live system. |

The Flatpak manifest requests `--socket=wayland`, `--socket=fallback-x11`,
`--socket=pulseaudio`, `--device=dri`, and `--filesystem=xdg-videos`. It does
not request network access, which the runtime enforces.

## When something does not work

1. Run `snipunix --diagnostics`. It reports the session type, the portal
   versions, the available encoders, and the model directory.
2. Export the report with `snipunix --diagnostics-output report.txt` and
   attach it to a bug report.
3. The report contains no screen content and no recognised text.
