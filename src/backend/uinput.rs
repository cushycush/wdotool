//! uinput fallback backend — kernel-level virtual input device via
//! `/dev/uinput`. No focus awareness and no window API, but works on any
//! compositor (or even bare X / console) when the process has uinput access.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use input_linux::{
    AbsoluteAxis, AbsoluteInfo, AbsoluteInfoSetup, EventKind, EventTime, InputEvent, InputId, Key,
    RelativeAxis, SynchronizeKind, UInputHandle,
};
use tracing::debug;
use xkbcommon::xkb;

use super::Backend;
use crate::error::{Result, WdoError};
use crate::types::{Capabilities, KeyDirection, MouseButton, WindowId, WindowInfo};

const NAME: &str = "uinput";
const DEVICE_NAME: &[u8] = b"wdotool virtual input";

pub struct UinputBackend {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    handle: UInputHandle<File>,
    keymap: SafeKeymap,
}

struct SafeKeymap(xkb::Keymap);
unsafe impl Send for SafeKeymap {}
unsafe impl Sync for SafeKeymap {}

impl UinputBackend {
    pub fn try_new() -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/uinput")
            .map_err(|e| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!(
                    "open /dev/uinput failed ({e}). The process needs write access — add \
                     the user to the `input` group or install a udev rule."
                ),
            })?;
        let handle = UInputHandle::new(file);

        configure_device(&handle)?;

        let id = InputId {
            bustype: 0x03, // BUS_USB — any non-zero value works; 0x03 is typical for virtual.
            vendor: 0x1234,
            product: 0x5678,
            version: 1,
        };
        // Advertise a generous absolute-pointer extent so uinput consumers
        // that care (e.g., remote input tools) don't clip our events.
        let abs_setup = [
            AbsoluteInfoSetup {
                axis: AbsoluteAxis::X,
                info: AbsoluteInfo {
                    value: 0,
                    minimum: 0,
                    maximum: 32767,
                    fuzz: 0,
                    flat: 0,
                    resolution: 0,
                },
            },
            AbsoluteInfoSetup {
                axis: AbsoluteAxis::Y,
                info: AbsoluteInfo {
                    value: 0,
                    minimum: 0,
                    maximum: 32767,
                    fuzz: 0,
                    flat: 0,
                    resolution: 0,
                },
            },
        ];
        handle
            .create(&id, DEVICE_NAME, 0, &abs_setup)
            .map_err(|e| WdoError::Backend {
                backend: NAME,
                source: anyhow::Error::new(e),
            })?;

        // Give the kernel + compositor time to pick up the new device via
        // udev. Without this, events emitted immediately after create() are
        // reliably dropped.
        thread::sleep(Duration::from_millis(200));

        let keymap = compile_default_keymap()?;
        debug!("uinput device created, keymap compiled");

        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                handle,
                keymap: SafeKeymap(keymap),
            })),
        })
    }
}

fn configure_device(handle: &UInputHandle<File>) -> Result<()> {
    let set_or = |r: io::Result<()>, what: &'static str| -> Result<()> {
        r.map_err(|e| WdoError::Backend {
            backend: NAME,
            source: anyhow::anyhow!("uinput {what}: {e}"),
        })
    };

    set_or(handle.set_evbit(EventKind::Key), "set EV_KEY bit")?;
    set_or(handle.set_evbit(EventKind::Relative), "set EV_REL bit")?;
    set_or(handle.set_evbit(EventKind::Absolute), "set EV_ABS bit")?;
    set_or(handle.set_evbit(EventKind::Synchronize), "set EV_SYN bit")?;

    // Enabling every Key variant costs one ioctl per key (~250 calls) but
    // only happens once at startup and means users never hit "this key code
    // wasn't registered" errors.
    for key in Key::iter() {
        set_or(handle.set_keybit(key), "set keybit")?;
    }

    for axis in [
        RelativeAxis::X,
        RelativeAxis::Y,
        RelativeAxis::Wheel,
        RelativeAxis::HorizontalWheel,
    ] {
        set_or(handle.set_relbit(axis), "set relbit")?;
    }

    for axis in [AbsoluteAxis::X, AbsoluteAxis::Y] {
        set_or(handle.set_absbit(axis), "set absbit")?;
    }

    Ok(())
}

fn compile_default_keymap() -> Result<xkb::Keymap> {
    let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    xkb::Keymap::new_from_names(&ctx, "", "", "", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
        .ok_or_else(|| WdoError::Backend {
            backend: NAME,
            source: anyhow::anyhow!(
                "xkb_keymap_new_from_names returned null (missing xkb config?)"
            ),
        })
}

fn current_time() -> EventTime {
    // Wall-clock time is fine for uinput; the kernel timestamps its own view
    // of the events as they pass through anyway.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    EventTime::new(now.as_secs() as _, now.subsec_micros() as _)
}

fn sync_event() -> InputEvent {
    InputEvent {
        time: current_time(),
        kind: EventKind::Synchronize,
        code: SynchronizeKind::Report as u16,
        value: 0,
    }
}

fn key_event(code: u16, pressed: bool) -> InputEvent {
    InputEvent {
        time: current_time(),
        kind: EventKind::Key,
        code,
        value: if pressed { 1 } else { 0 },
    }
}

fn rel_event(axis: RelativeAxis, delta: i32) -> InputEvent {
    InputEvent {
        time: current_time(),
        kind: EventKind::Relative,
        code: axis.code(),
        value: delta,
    }
}

fn abs_event(axis: AbsoluteAxis, value: i32) -> InputEvent {
    InputEvent {
        time: current_time(),
        kind: EventKind::Absolute,
        code: axis.code(),
        value,
    }
}

fn write_events(handle: &UInputHandle<File>, evs: &[InputEvent]) -> Result<()> {
    // input-linux uses the `sys::input_event` representation for write; the
    // public InputEvent is layout-compatible via #[repr(C)] + transmute.
    let raw: Vec<input_linux::sys::input_event> = evs.iter().map(|e| (*e).into()).collect();
    handle.write(&raw).map_err(|e| WdoError::Backend {
        backend: NAME,
        source: anyhow::Error::new(e),
    })?;
    Ok(())
}

/// Walk the keymap for the evdev keycode (xkb-keycode minus 8) that produces
/// `target` at level 0 or 1. Returns (keycode, needs_shift). The shift bit is
/// what the caller uses to decide whether to hold Shift_L around the press.
fn find_keysym(keymap: &xkb::Keymap, target: xkb::Keysym) -> Option<(u32, bool)> {
    if target.raw() == 0 {
        return None;
    }
    for keycode in keymap.min_keycode().raw()..=keymap.max_keycode().raw() {
        for level in 0..=1 {
            let syms = keymap.key_get_syms_by_level(xkb::Keycode::new(keycode), 0, level);
            if syms.contains(&target) {
                return Some((keycode.saturating_sub(8), level == 1));
            }
        }
    }
    None
}

/// Name-keyed wrapper around `find_keysym` — looks up xkb keysym by its
/// textual name ("Return", "Shift_L", "U20AC", …).
fn resolve_keycode(keymap: &xkb::Keymap, name: &str) -> Option<(u32, bool)> {
    find_keysym(keymap, xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS))
}

fn mouse_button_code(btn: MouseButton) -> Option<u16> {
    // linux/input-event-codes.h — BTN_* constants
    Some(match btn {
        MouseButton::Left => 0x110,
        MouseButton::Right => 0x111,
        MouseButton::Middle => 0x112,
        MouseButton::Back => 0x113,
        MouseButton::Forward => 0x114,
        MouseButton::Other(_) => return None,
    })
}

#[async_trait]
impl Backend for UinputBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            key_input: true,
            text_input: true, // best-effort: same caveat as libei
            pointer_move_absolute: true,
            pointer_move_relative: true,
            pointer_button: true,
            scroll: true,
            list_windows: false,
            active_window: false,
            activate_window: false,
            close_window: false,
        }
    }

    async fn key(&self, keysym: &str, dir: KeyDirection) -> Result<()> {
        let (keycode, needs_shift, shift_kc) = {
            let inner = self.inner.lock().unwrap();
            let (kc, needs_shift) =
                resolve_keycode(&inner.keymap.0, keysym).ok_or_else(|| WdoError::Keysym {
                    input: keysym.into(),
                    reason: format!("keysym '{keysym}' not found in default keymap"),
                })?;
            let shift_kc = resolve_keycode(&inner.keymap.0, "Shift_L").map(|(kc, _)| kc);
            (kc as u16, needs_shift, shift_kc.map(|k| k as u16))
        };

        let inner = self.inner.lock().unwrap();
        let mut events = Vec::with_capacity(6);
        match dir {
            KeyDirection::Press => {
                if needs_shift {
                    if let Some(kc) = shift_kc {
                        events.push(key_event(kc, true));
                    }
                }
                events.push(key_event(keycode, true));
            }
            KeyDirection::Release => {
                events.push(key_event(keycode, false));
                if needs_shift {
                    if let Some(kc) = shift_kc {
                        events.push(key_event(kc, false));
                    }
                }
            }
            KeyDirection::PressRelease => {
                if needs_shift {
                    if let Some(kc) = shift_kc {
                        events.push(key_event(kc, true));
                    }
                }
                events.push(key_event(keycode, true));
                events.push(key_event(keycode, false));
                if needs_shift {
                    if let Some(kc) = shift_kc {
                        events.push(key_event(kc, false));
                    }
                }
            }
        }
        events.push(sync_event());
        write_events(&inner.handle, &events)
    }

    async fn type_text(&self, text: &str, delay: Duration) -> Result<()> {
        // uinput has the same limitation as libei: no way to supply a
        // keymap. Each character must already be expressible in the
        // compositor's active layout.
        let (resolutions, shift_kc) = {
            let inner = self.inner.lock().unwrap();
            let mut out: Vec<(u32, bool)> = Vec::new();
            let mut missing: Vec<char> = Vec::new();
            for c in text.chars() {
                let sym = xkb::Keysym::from_char(c);
                match find_keysym(&inner.keymap.0, sym) {
                    Some(pair) => out.push(pair),
                    None => missing.push(c),
                }
            }
            if !missing.is_empty() {
                tracing::warn!(
                    "uinput type_text: {} char(s) not in default keymap: {:?}",
                    missing.len(),
                    missing.iter().take(8).collect::<Vec<_>>()
                );
            }
            let shift_kc = resolve_keycode(&inner.keymap.0, "Shift_L").map(|(kc, _)| kc as u16);
            (out, shift_kc)
        };

        let total = resolutions.len();
        for (idx, (kc, needs_shift)) in resolutions.into_iter().enumerate() {
            // Strictly scope the MutexGuard so it can't straddle the
            // subsequent .await below (MutexGuard is !Send).
            {
                let inner = self.inner.lock().unwrap();
                let mut events = Vec::with_capacity(5);
                let kc = kc as u16;
                if needs_shift {
                    if let Some(sk) = shift_kc {
                        events.push(key_event(sk, true));
                    }
                }
                events.push(key_event(kc, true));
                events.push(key_event(kc, false));
                if needs_shift {
                    if let Some(sk) = shift_kc {
                        events.push(key_event(sk, false));
                    }
                }
                events.push(sync_event());
                write_events(&inner.handle, &events)?;
            }

            if !delay.is_zero() && idx + 1 < total {
                tokio::time::sleep(delay).await;
            }
        }
        Ok(())
    }

    async fn mouse_move(&self, x: i32, y: i32, absolute: bool) -> Result<()> {
        let inner = self.inner.lock().unwrap();
        let events = if absolute {
            vec![
                abs_event(AbsoluteAxis::X, x),
                abs_event(AbsoluteAxis::Y, y),
                sync_event(),
            ]
        } else {
            let mut v = Vec::with_capacity(3);
            if x != 0 {
                v.push(rel_event(RelativeAxis::X, x));
            }
            if y != 0 {
                v.push(rel_event(RelativeAxis::Y, y));
            }
            if v.is_empty() {
                return Ok(());
            }
            v.push(sync_event());
            v
        };
        write_events(&inner.handle, &events)
    }

    async fn mouse_button(&self, btn: MouseButton, dir: KeyDirection) -> Result<()> {
        let code = mouse_button_code(btn)
            .ok_or_else(|| WdoError::InvalidArg(format!("unsupported mouse button: {btn:?}")))?;
        let inner = self.inner.lock().unwrap();
        let events: Vec<InputEvent> = match dir {
            KeyDirection::Press => vec![key_event(code, true), sync_event()],
            KeyDirection::Release => vec![key_event(code, false), sync_event()],
            KeyDirection::PressRelease => vec![
                key_event(code, true),
                sync_event(),
                key_event(code, false),
                sync_event(),
            ],
        };
        write_events(&inner.handle, &events)
    }

    async fn scroll(&self, dx: f64, dy: f64) -> Result<()> {
        let inner = self.inner.lock().unwrap();
        let mut events = Vec::with_capacity(3);
        if dx != 0.0 {
            events.push(rel_event(RelativeAxis::HorizontalWheel, dx.round() as i32));
        }
        if dy != 0.0 {
            // Evdev convention: REL_WHEEL positive = away from user (up).
            // xdotool's convention: positive dy = down. Flip sign to match.
            events.push(rel_event(RelativeAxis::Wheel, -(dy.round() as i32)));
        }
        if events.is_empty() {
            return Ok(());
        }
        events.push(sync_event());
        write_events(&inner.handle, &events)
    }

    async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        Err(WdoError::NotSupported {
            backend: NAME,
            what: "list_windows — uinput is kernel-level and has no window API",
        })
    }

    async fn active_window(&self) -> Result<Option<WindowInfo>> {
        Err(WdoError::NotSupported {
            backend: NAME,
            what: "active_window — uinput has no window API",
        })
    }

    async fn activate_window(&self, _id: &WindowId) -> Result<()> {
        Err(WdoError::NotSupported {
            backend: NAME,
            what: "activate_window — uinput has no window API",
        })
    }

    async fn close_window(&self, _id: &WindowId) -> Result<()> {
        Err(WdoError::NotSupported {
            backend: NAME,
            what: "close_window — uinput has no window API",
        })
    }
}

impl Drop for UinputBackend {
    fn drop(&mut self) {
        if let Ok(inner) = self.inner.lock() {
            let _ = inner.handle.dev_destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_button_codes_match_linux_input_event_codes() {
        // These are stable ABI constants from <linux/input-event-codes.h>.
        // Getting them wrong swaps "back" and "forward" mouse buttons, which
        // breaks user scripts silently — hence the anchoring test.
        assert_eq!(mouse_button_code(MouseButton::Left), Some(0x110)); // BTN_LEFT
        assert_eq!(mouse_button_code(MouseButton::Right), Some(0x111)); // BTN_RIGHT
        assert_eq!(mouse_button_code(MouseButton::Middle), Some(0x112)); // BTN_MIDDLE
        assert_eq!(mouse_button_code(MouseButton::Back), Some(0x113)); // BTN_SIDE
        assert_eq!(mouse_button_code(MouseButton::Forward), Some(0x114)); // BTN_EXTRA

        // Unknown indices reject — caller surfaces InvalidArg with the original.
        assert_eq!(mouse_button_code(MouseButton::Other(99)), None);
    }
}
