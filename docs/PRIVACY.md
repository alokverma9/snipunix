# Privacy

Snipunix is a local application. It does not upload anything, it does not
contain telemetry, and it does not collect usage data.

This document describes exactly what the application does with your data,
what leaves the machine if anything does, and how to verify the claims.

## The short version

- Screenshots, recordings, annotations, and recognised text stay in memory or
  in a file you chose.
- There is no account, no identifier, and no analytics.
- There is no crash reporting service. Crash reports are a local file you
  have to turn on and attach by hand.
- The only network request Snipunix ever makes is an explicit, user-initiated
  download of the OCR models, on a host build that does not have them.

## What is stored on your machine

| What | Where | When |
| --- | --- | --- |
| Settings | `$XDG_CONFIG_HOME/snipunix/settings.toml` | When you change a setting |
| Crash reports | `$XDG_STATE_HOME/snipunix/crashes` | Only if you turn **Save crash reports** on and the application panics |
| Exported diagnostics | Wherever you choose | Only when you press **Export diagnostics** |
| Saved captures | Wherever you choose | Only when you save or auto-save |
| Recordings | `$XDG_VIDEOS_DIR/Screen Recordings` by default | When you record |
| OCR models | `$XDG_DATA_HOME/snipunix/models` | When you download them |

Settings are written with mode `0600` and are replaced atomically, so a crash
during the write cannot leave a half-written file.

## What is never collected

- No usage analytics, no crash telemetry, no "diagnostics ping".
- No user identifier, no machine identifier, no IP address in any output file.
- No screenshot content, no recognised text, and no clipboard content in any
  diagnostics report.
- No file names from your directories in any report.

## The one network request

The OCR models are about 12 MB in total. The Flatpak and the deb bundle them,
so nothing is downloaded.

A host build does not bundle them. When you press **Download OCR models** in
Settings, or run `snipunix --download-ocr-models`, Snipunix fetches two files
over HTTPS from the URL pinned in the source:

- `https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.onnx`
- `https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.onnx`

Both files are verified against a SHA-256 hash that is pinned in the source
before they are stored. A file whose hash does not match is deleted and the
download fails.

The download never runs on its own. It is not part of the capture flow, the
OCR flow, or the start-up path. The OCR crate does not depend on the capture
crate, and the capture crate does not depend on the OCR crate, so there is no
path from "take a screenshot" to "open a socket".

## How the claims are verified

The release gate checks the promise in two ways.

**Statically.** `crates/snipunix-app/tests/no_network.rs` walks the source
tree and fails if any crate other than `snipunix-ocr` contains a network
primitive, if the capture crate depends on the OCR crate, or if the OCR crate
depends on the capture crate. It also checks that the model download is
guarded by a hash.

**At runtime.** `scripts/check-no-network.sh` runs a capture under `strace`
and fails if the process opens an internet socket or connects over IPv4 or
IPv6. The only socket the capture opens is the local D-Bus socket it uses to
talk to the desktop portal.

```sh
cargo test -p snipunix-app --test no_network
scripts/check-no-network.sh
```

## Verifying it yourself

If you do not trust the checks, watch the traffic:

```sh
sudo tcpdump -i lo -n "host $(hostname -I | awk '{print $1}')" -w /tmp/snipunix.pcap
snipunix --capture screen --output /tmp/shot.png
```

Only the portal's D-Bus traffic appears on the loopback interface, and nothing
leaves the machine.

## Remote crashes and support

When you report a problem, you decide what to share. A diagnostics report
describes your environment and contains nothing you produced. Attach it if
you want to; you never have to.

## Permissions Snipunix asks for

| Permission | Why |
| --- | --- |
| Screenshot | To capture the screen. The desktop asks for it; Snipunix never captures in the background. |
| ScreenCast | To record the screen. The desktop asks for it when you record. |
| GlobalShortcuts | To register the global capture shortcuts. |
| Microphone | Only if you turn microphone recording on in Settings. |
| Screen / ScreenCast | The portal grants these, not Snipunix directly. |

The application never runs as a background service and never starts a capture
on its own.
