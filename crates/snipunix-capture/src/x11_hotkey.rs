//! X11 `XGrabKey` global-shortcut fallback.
//!
//! The GlobalShortcuts portal is the preferred mechanism on Wayland. On a
//! plain X11 session there may be no portal backend (or no application ID), so
//! this module grabs the configured key combinations directly on the root
//! window and forwards presses on a worker thread.
//!
//! Only the keys used by Snipunix's default triggers are mapped: modifier
//! tokens (`CTRL`, `SHIFT`, `ALT`, `SUPER`) plus printable ASCII, function keys,
//! and the common navigation keys. An unsupported trigger is reported instead
//! of being silently ignored.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, KeyPressEvent, ModMask, Window};
use x11rb::rust_connection::RustConnection;

use crate::CaptureError;

/// A running X11 hotkey grabber.
pub struct X11HotkeyService {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for X11HotkeyService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11HotkeyService")
            .field("running", &!self.stop.load(Ordering::Relaxed))
            .finish()
    }
}

impl Drop for X11HotkeyService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // The worker polls with a short timeout, so a join is safe and keeps
        // teardown deterministic.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Grabs `triggers` on the X11 root window. `on_press` receives the index of
/// the trigger that fired. It runs on the worker thread and must not touch
/// GTK directly.
pub fn spawn(
    triggers: &[String],
    on_press: impl Fn(usize) + Send + 'static,
) -> Result<X11HotkeyService, CaptureError> {
    if std::env::var_os("DISPLAY").is_none() {
        return Err(CaptureError::X11Unavailable(
            "DISPLAY is not set; X11 key grabs are unavailable".to_string(),
        ));
    }
    if triggers.is_empty() {
        return Err(CaptureError::Unsupported(
            "no X11 shortcut triggers were configured".to_string(),
        ));
    }
    let parsed: Vec<ParsedTrigger> = triggers
        .iter()
        .map(|trigger| ParsedTrigger::parse(trigger))
        .collect::<Result<_, _>>()?;

    let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(1);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread = thread::Builder::new()
        .name("snipunix-x11-hotkeys".to_string())
        .spawn(move || run(parsed, on_press, thread_stop, ready_sender))
        .map_err(|error| {
            CaptureError::X11(format!("could not spawn the hotkey thread: {error}"))
        })?;

    match ready_receiver.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(X11HotkeyService {
            stop,
            thread: Some(thread),
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(error) => {
            // Tell a worker that is still connecting to stop, then detach it.
            // Joining here could block the caller if the X server is wedged.
            stop.store(true, Ordering::Relaxed);
            tracing::warn!(%error, "the X11 hotkey worker did not report readiness");
            Err(CaptureError::X11(format!(
                "the X11 hotkey worker did not start: {error}"
            )))
        }
    }
}

fn run(
    triggers: Vec<ParsedTrigger>,
    on_press: impl Fn(usize),
    stop: Arc<AtomicBool>,
    ready: std::sync::mpsc::SyncSender<Result<(), CaptureError>>,
) {
    let setup = RustConnection::connect(None).map(|(connection, screen)| {
        let root = connection.setup().roots[screen].root;
        (connection, root)
    });
    let (connection, root) = match setup {
        Ok(value) => value,
        Err(error) => {
            let _ = ready.send(Err(CaptureError::X11Unavailable(error.to_string())));
            return;
        }
    };

    if let Err(error) = grab_all(&connection, root, &triggers) {
        let _ = ready.send(Err(error));
        return;
    }
    connection.flush().ok();
    // Force a round-trip so the server has processed the GrabKey requests and
    // queued any BadAccess errors before we inspect the event queue.
    if let Ok(cookie) = connection.get_input_focus() {
        let _ = cookie.reply();
    }
    let mut errors = Vec::new();
    while let Ok(Some(event)) = connection.poll_for_event() {
        if let x11rb::protocol::Event::Error(error) = event {
            errors.push(format!("{error:?}"));
        }
    }
    if !errors.is_empty() {
        let _ = ready.send(Err(CaptureError::X11(format!(
            "could not grab the shortcut keys: {}",
            errors.join("; ")
        ))));
        return;
    }
    if ready.send(Ok(())).is_err() {
        return;
    }
    tracing::debug!(triggers = triggers.len(), "X11 global shortcuts are active");

    while !stop.load(Ordering::Relaxed) {
        match connection.poll_for_event() {
            Ok(Some(x11rb::protocol::Event::KeyPress(event))) => {
                if let Some(index) = match_trigger(&event, &triggers) {
                    on_press(index);
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => thread::sleep(Duration::from_millis(8)),
            Err(error) => {
                tracing::warn!(%error, "X11 hotkey connection failed; stopping");
                break;
            }
        }
    }
    // Release every grab before the connection closes.
    let _ = connection.ungrab_key(x11rb::protocol::xproto::Grab::ANY, root, ModMask::ANY);
    let _ = connection.flush();
}

struct ParsedTrigger {
    keycode: u8,
    masks: Vec<u16>,
}

fn grab_all(
    connection: &RustConnection,
    root: Window,
    triggers: &[ParsedTrigger],
) -> Result<(), CaptureError> {
    for trigger in triggers {
        for mask in &trigger.masks {
            connection
                .grab_key(
                    false,
                    root,
                    ModMask::from(*mask),
                    trigger.keycode,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )
                .map_err(|error| CaptureError::X11(error.to_string()))?;
        }
    }
    Ok(())
}

fn match_trigger(event: &KeyPressEvent, triggers: &[ParsedTrigger]) -> Option<usize> {
    let state = u16::from(event.state);
    triggers
        .iter()
        .position(|trigger| trigger.keycode == event.detail && trigger.masks.contains(&state))
}

impl ParsedTrigger {
    fn parse(trigger: &str) -> Result<Self, CaptureError> {
        let mut base = 0u16;
        let mut key = None;
        for token in trigger.split('+') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            match token.to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" => base |= u16::from(ModMask::CONTROL),
                "SHIFT" => base |= u16::from(ModMask::SHIFT),
                "ALT" | "MOD1" => base |= u16::from(ModMask::M1),
                "SUPER" | "META" | "WIN" | "MOD4" => base |= u16::from(ModMask::M4),
                other => key = Some(other.to_string()),
            }
        }
        let key = key
            .ok_or_else(|| CaptureError::Unsupported(format!("shortcut '{trigger}' has no key")))?;
        let keysym = keysym_for(&key).ok_or_else(|| {
            CaptureError::Unsupported(format!(
                "shortcut key '{key}' is not supported by the X11 grabber"
            ))
        })?;
        let keycode = keycode_for(keysym).ok_or_else(|| {
            CaptureError::Unsupported(format!(
                "shortcut key '{key}' is not present in the X11 keymap"
            ))
        })?;
        // CapsLock and NumLock must not defeat the grab. Grab the four
        // combinations of LockMask and Mod2Mask (the conventional NumLock).
        let lock = u16::from(ModMask::LOCK);
        let num = u16::from(ModMask::M2);
        let extra = [0u16, lock, num, lock | num];
        let masks = extra
            .into_iter()
            .map(|bits| base | bits)
            .collect::<Vec<_>>();
        Ok(Self { keycode, masks })
    }
}

fn keysym_for(key: &str) -> Option<u32> {
    if key.len() == 1 {
        let character = key.chars().next()?;
        if character.is_ascii_graphic() || character == ' ' {
            return Some(character as u32);
        }
    }
    Some(match key {
        "SPACE" => 0x0020,
        "TAB" => 0xff09,
        "RETURN" | "ENTER" => 0xff0d,
        "ESC" | "ESCAPE" => 0xff1b,
        "BACKSPACE" => 0xff08,
        "DELETE" | "DEL" => 0xffff,
        "INSERT" => 0xff63,
        "HOME" => 0xff50,
        "END" => 0xff57,
        "PAGEUP" | "PRIOR" => 0xff55,
        "PAGEDOWN" | "NEXT" => 0xff56,
        "LEFT" => 0xff51,
        "UP" => 0xff52,
        "RIGHT" => 0xff53,
        "DOWN" => 0xff54,
        "PRINT" | "PRINTSCREEN" => 0xff61,
        other => {
            let function = other.strip_prefix('F')?;
            let number: u32 = function.parse().ok()?;
            if (1..=24).contains(&number) {
                0xffbe + number - 1
            } else {
                return None;
            }
        }
    })
}

fn keycode_for(keysym: u32) -> Option<u8> {
    let (connection, screen) = RustConnection::connect(None).ok()?;
    let _ = screen;
    let setup = connection.setup();
    let count = setup.max_keycode - setup.min_keycode + 1;
    let mapping = connection
        .get_keyboard_mapping(setup.min_keycode, count)
        .ok()?
        .reply()
        .ok()?;
    let per_keycode = usize::from(mapping.keysyms_per_keycode);
    if per_keycode == 0 {
        return None;
    }
    for (index, chunk) in mapping.keysyms.chunks(per_keycode).enumerate() {
        if chunk.contains(&keysym) {
            return u8::try_from(usize::from(setup.min_keycode) + index).ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_printable_and_named_keysyms() {
        assert_eq!(keysym_for("s"), Some(0x73));
        assert_eq!(keysym_for("S"), Some(0x53));
        assert_eq!(keysym_for("F12"), Some(0xffc9));
        assert_eq!(keysym_for("ENTER"), Some(0xff0d));
        assert_eq!(keysym_for("NOTAKEY"), None);
    }

    #[test]
    fn parses_default_triggers_when_a_display_is_available() {
        if std::env::var_os("DISPLAY").is_none() {
            return;
        }
        let trigger = ParsedTrigger::parse("CTRL+SHIFT+s").unwrap();
        assert_ne!(trigger.keycode, 0);
        assert_eq!(trigger.masks.len(), 4);
    }
}
