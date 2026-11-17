# Snipunix 0.6.0 release notes

**Date:** 2026-11-17
**Channel:** stable (and a parallel beta channel)

0.6.0 is the release candidate milestone. It does not change what the
application does; it makes the application releasable: the interface is
translated, the diagnostic story is finished, the performance numbers are
measured rather than assumed, the supply chain is audited, and the privacy
promise is verified by a test.

## Highlights

- **Translations.** The interface is available in German and Spanish. The
  catalogues follow the GNU gettext conventions and are embedded in the
  binary, so a translation never depends on a locale directory being present.
- **A diagnostics export.** Settings → About has an **Export…** button, and
  the command line has `snipunix --diagnostics-output PATH`. The report
  describes the session, the portals, the encoders, the models, and the
  settings. It contains no screen content and no recognised text.
- **Crash reports, off by default.** A new setting writes a local crash
  report when the application panics. It is off until you turn it on, it never
  leaves the machine, and it never contains user content.
- **A beta channel.** The Flatpak can be built as
  `io.github.alokverma9.snipunix.Beta`, installed next to the stable release,
  with its own settings and its own portal permissions.
- **Measured performance.** Criterion benchmarks cover the decode, crop,
  encode, and rasterise paths, and a script checks the peak memory against
  the roadmap's budgets.
- **An audited supply chain.** `deny.toml` records the licence policy and the
  accepted advisories, and `cargo deny` and `cargo audit` are part of the
  release gate.
- **A verified privacy promise.** A test walks the source tree and fails if a
  network primitive appears outside the OCR crate, and a script runs a capture
  under `strace` and fails if an internet socket is opened.

## Upgrade notes

- The settings schema moved from version 1 to version 2. Old settings files
  are read as they are; the only new key is the diagnostics section, and its
  defaults keep crash reporting off.
- Global shortcuts registered by an earlier version stay registered for the
  session. They have to be registered again after the upgrade because the
  registration is session-scoped.
- The application identifier is unchanged for the stable build, so the
  desktop portal permissions you already granted keep working.

## Known issues

The known issues are unchanged from 0.5.0 and are listed in
[PLATFORM_LIMITATIONS.md](PLATFORM_LIMITATIONS.md). The most important ones:

- Only a single monitor was available for live testing, so mixed-DPI
  behaviour is implemented but unverified.
- The X11 backend is a fallback and was not verified on a live X11 session.
- Microphone and system audio are unit-tested but were not verified with a
  capture device.
- The tray needs a StatusNotifierItem host, which a stock GNOME shell does
  not provide.

## Verifying the release

```sh
scripts/check-release.sh          # formatting, clippy, tests, catalogues, supply chain
cargo audit                       # known vulnerabilities
scripts/check-no-network.sh       # the capture opens no internet socket
cargo bench --workspace --benches # the performance numbers
```

## Thanks

Thank you to everyone who tried the beta channel and reported a problem.
Every report made the next release better.
