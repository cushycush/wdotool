//! Pure-function event mapping. Two sources, one target shape.
//!
//! - [`eis_to_rec`] turns a `reis::event::EiEvent` (libei portal
//!   receiver-mode event) into a [`RecEvent`] with absolute Move
//!   coordinates and the threshold + dedupe applied.
//! - [`evdev_to_rec`] turns a `evdev::InputEvent` into a [`RecEvent`]
//!   with delta Move coordinates and the time-throttled accumulator.
//!
//! Both share [`keycode_to_chord`], which maps a Linux input-event
//! keycode + modifier bitmask to a wdotool chord string. The mapping
//! is US-en QWERTY for now; non-US layouts produce some `keyNN`
//! fallbacks that the user can fix up post-recording. Layout-aware
//! decoding via xkbcommon is a separate, layered improvement that
//! affects both the recorder and the replayer the same way.
//!
//! All three are pure. State (modifier mask, motion accumulator,
//! pointer-position dedupe) is passed in by the caller and mutated
//! through `&mut`; the backend pumps own that state and feed it in
//! per event.

use reis::event::EiEvent;

use super::types::RecEvent;

/// Modifier bit positions, matching xkb's canonical indices. The libei
/// portal consistently maps the standard modifiers in these slots, and
/// evdev callers maintain a parallel bitmask using the same positions
/// so [`keycode_to_chord`] can be shared.
pub(super) const MOD_SHIFT: u32 = 1 << 0;
pub(super) const MOD_CTRL: u32 = 1 << 2;
pub(super) const MOD_ALT: u32 = 1 << 3;
pub(super) const MOD_SUPER: u32 = 1 << 6;

/// Per-stream state for the EIS event mapper. Owned by the portal
/// pump and threaded through every call to [`eis_to_rec`].
#[derive(Debug, Clone)]
pub struct EisState {
    /// Most recent absolute pointer position, for the dedupe in
    /// `PointerMotionAbsolute`. `i32::MIN` is the "never seen one"
    /// sentinel so the first event always emits.
    pub last_x: i32,
    pub last_y: i32,
    /// Sub-threshold relative motion accumulator.
    pub rel_x: f32,
    pub rel_y: f32,
    /// xkb modifier bitmask the server reports via
    /// `KeyboardModifiers`.
    pub mods: u32,
}

impl Default for EisState {
    fn default() -> Self {
        Self {
            last_x: i32::MIN,
            last_y: i32::MIN,
            rel_x: 0.0,
            rel_y: 0.0,
            mods: 0,
        }
    }
}

/// Stateful EIS → [`RecEvent`] mapper.
pub fn eis_to_rec(
    event: &EiEvent,
    t_ms: u64,
    state: &mut EisState,
    move_threshold_px: i32,
) -> Option<RecEvent> {
    use reis::ei::button::ButtonState;
    use reis::ei::keyboard::KeyState;
    match event {
        // Only record the press (release is implicit on replay via
        // `wdotool click`). xdotool button indices: 1=left, 2=middle,
        // 3=right, 8=back, 9=forward.
        EiEvent::Button(evt) => {
            if evt.state != ButtonState::Press {
                return None;
            }
            let button = match evt.button {
                0x110 /* BTN_LEFT */ => 1,
                0x112 /* BTN_MIDDLE */ => 2,
                0x111 /* BTN_RIGHT */ => 3,
                0x113 /* BTN_SIDE */ => 8,
                0x114 /* BTN_EXTRA */ => 9,
                _ => return None,
            };
            Some(RecEvent::Click { t_ms, button })
        }
        EiEvent::PointerMotionAbsolute(evt) => {
            let x = evt.dx_absolute as i32;
            let y = evt.dy_absolute as i32;
            let moved = state.last_x == i32::MIN
                || (x - state.last_x).abs() >= move_threshold_px
                || (y - state.last_y).abs() >= move_threshold_px;
            if !moved {
                return None;
            }
            state.last_x = x;
            state.last_y = y;
            Some(RecEvent::MoveAbs { t_ms, x, y })
        }
        EiEvent::PointerMotion(evt) => {
            state.rel_x += evt.dx;
            state.rel_y += evt.dy;
            let threshold = move_threshold_px as f32;
            if state.rel_x.abs() < threshold && state.rel_y.abs() < threshold {
                return None;
            }
            let dx = state.rel_x as i32;
            let dy = state.rel_y as i32;
            state.rel_x = 0.0;
            state.rel_y = 0.0;
            Some(RecEvent::MoveDelta { t_ms, dx, dy })
        }
        EiEvent::ScrollDiscrete(evt) => Some(RecEvent::Scroll {
            t_ms,
            dx: evt.discrete_dx,
            dy: evt.discrete_dy,
        }),
        EiEvent::ScrollDelta(evt) => Some(RecEvent::Scroll {
            t_ms,
            dx: evt.dx as i32,
            dy: evt.dy as i32,
        }),
        EiEvent::KeyboardModifiers(evt) => {
            state.mods = evt.depressed | evt.latched | evt.locked;
            None
        }
        EiEvent::KeyboardKey(evt) => {
            if evt.state != KeyState::Press {
                return None;
            }
            let chord = keycode_to_chord(evt.key, state.mods)?;
            Some(RecEvent::Key { t_ms, chord })
        }
        _ => None,
    }
}

/// Stateful evdev → [`RecEvent`] mapper.
///
/// Mouse motion is time-throttled. `rel_x`/`rel_y` accumulate
/// `REL_X`/`REL_Y` deltas; when at least `min_move_interval_ms` has
/// elapsed since the last emission, the accumulator flushes as a
/// single `Move` and the throttle clock resets. Below the interval,
/// motion accumulates without producing an event.
///
/// `mods` is a shared bitmask across all open input devices, in the
/// same xkb bit positions [`keycode_to_chord`] uses. The backend
/// holds it as `Arc<AtomicU32>` so multi-device mod-state is consistent
/// regardless of which device sees the modifier press / release.
///
/// Returns `Ok(None)` for events we don't capture (synchronization
/// frames, sub-threshold motion below the interval, modifier presses
/// that just update state). The boolean in the return tuple is
/// `should_stop_now`: today this is always `false`, but it's reserved
/// so future control-key bindings (e.g., a Stop hotkey baked into the
/// substrate) can signal the pump without leaking through `RecEvent`.
pub fn evdev_to_rec(
    ev: &evdev::InputEvent,
    t_ms: u64,
    mods: &std::sync::atomic::AtomicU32,
    rel_x: &mut i32,
    rel_y: &mut i32,
    last_move_ms: &std::sync::atomic::AtomicU64,
    min_move_interval_ms: u64,
) -> Option<RecEvent> {
    use evdev::{EventSummary, KeyCode, RelativeAxisCode};
    use std::sync::atomic::Ordering;

    fn modifier_bit(k: KeyCode) -> Option<u32> {
        Some(match k {
            KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT => MOD_SHIFT,
            KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL => MOD_CTRL,
            KeyCode::KEY_LEFTALT | KeyCode::KEY_RIGHTALT => MOD_ALT,
            KeyCode::KEY_LEFTMETA | KeyCode::KEY_RIGHTMETA => MOD_SUPER,
            _ => return None,
        })
    }

    match ev.destructure() {
        EventSummary::Key(_, code, value) => {
            // value: 0=release, 1=press, 2=repeat. Modifiers track
            // press/release for the shared bitmask. Plain keys emit
            // only on initial press; repeats and releases discarded.
            if let Some(bit) = modifier_bit(code) {
                let m = mods.load(Ordering::Relaxed);
                let new = match value {
                    0 => m & !bit,
                    1 | 2 => m | bit,
                    _ => m,
                };
                mods.store(new, Ordering::Relaxed);
                return None;
            }

            // Mouse buttons share the keycode space with KEY_*.
            let button = match code {
                KeyCode::BTN_LEFT => Some(1),
                KeyCode::BTN_MIDDLE => Some(2),
                KeyCode::BTN_RIGHT => Some(3),
                KeyCode::BTN_SIDE => Some(8),
                KeyCode::BTN_EXTRA => Some(9),
                _ => None,
            };
            if let Some(btn) = button {
                if value == 1 {
                    return Some(RecEvent::Click { t_ms, button: btn });
                }
                return None;
            }

            if value != 1 {
                return None;
            }
            let chord = keycode_to_chord(code.0 as u32, mods.load(Ordering::Relaxed))?;
            Some(RecEvent::Key { t_ms, chord })
        }
        EventSummary::RelativeAxis(_, axis, value) => {
            match axis {
                RelativeAxisCode::REL_X => {
                    *rel_x = rel_x.saturating_add(value);
                }
                RelativeAxisCode::REL_Y => {
                    *rel_y = rel_y.saturating_add(value);
                }
                RelativeAxisCode::REL_WHEEL | RelativeAxisCode::REL_WHEEL_HI_RES => {
                    return Some(RecEvent::Scroll {
                        t_ms,
                        dx: 0,
                        dy: value,
                    });
                }
                RelativeAxisCode::REL_HWHEEL | RelativeAxisCode::REL_HWHEEL_HI_RES => {
                    return Some(RecEvent::Scroll {
                        t_ms,
                        dx: value,
                        dy: 0,
                    });
                }
                _ => return None,
            }
            // Time-throttle Move emission. Without this, every mouse
            // tick (1000Hz on a gaming mouse) saturates downstream
            // queues.
            let last = last_move_ms.load(Ordering::Relaxed);
            if t_ms.saturating_sub(last) < min_move_interval_ms {
                return None;
            }
            last_move_ms.store(t_ms, Ordering::Relaxed);
            let dx = std::mem::take(rel_x);
            let dy = std::mem::take(rel_y);
            Some(RecEvent::MoveDelta { t_ms, dx, dy })
        }
        _ => None,
    }
}

/// Build a wdotool chord string from a Linux input-event keycode + xkb
/// modifier bitmask. US-en QWERTY mapping; non-US layouts will see
/// some letters map to the wrong keysym, which the user can fix
/// post-recording. Anything not in the table falls through to `keyNN`
/// so events are never silently dropped.
pub fn keycode_to_chord(key: u32, mods: u32) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    if mods & MOD_CTRL != 0 {
        parts.push("ctrl");
    }
    if mods & MOD_ALT != 0 {
        parts.push("alt");
    }
    if mods & MOD_SHIFT != 0 {
        parts.push("shift");
    }
    if mods & MOD_SUPER != 0 {
        parts.push("super");
    }

    let name = match key {
        // Editing / navigation
        1 => "Escape",
        14 => "BackSpace",
        15 => "Tab",
        28 => "Return",
        57 => "space",
        96 => "Return", // KP_Enter
        103 => "Up",
        108 => "Down",
        105 => "Left",
        106 => "Right",
        102 => "Home",
        107 => "End",
        104 => "Prior", // Page Up
        109 => "Next",  // Page Down
        110 => "Insert",
        111 => "Delete",

        // Function keys
        59..=68 => {
            static FK: &[&str] = &["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10"];
            FK[(key - 59) as usize]
        }
        87 => "F11",
        88 => "F12",

        // Top number row
        2 => "1",
        3 => "2",
        4 => "3",
        5 => "4",
        6 => "5",
        7 => "6",
        8 => "7",
        9 => "8",
        10 => "9",
        11 => "0",
        12 => "minus",
        13 => "equal",

        // Letter rows (QWERTY)
        16 => "q",
        17 => "w",
        18 => "e",
        19 => "r",
        20 => "t",
        21 => "y",
        22 => "u",
        23 => "i",
        24 => "o",
        25 => "p",
        26 => "bracketleft",
        27 => "bracketright",
        43 => "backslash",
        30 => "a",
        31 => "s",
        32 => "d",
        33 => "f",
        34 => "g",
        35 => "h",
        36 => "j",
        37 => "k",
        38 => "l",
        39 => "semicolon",
        40 => "apostrophe",
        41 => "grave",
        44 => "z",
        45 => "x",
        46 => "c",
        47 => "v",
        48 => "b",
        49 => "n",
        50 => "m",
        51 => "comma",
        52 => "period",
        53 => "slash",

        _ => {
            let fallback = format!("key{key}");
            if parts.is_empty() {
                return Some(fallback);
            }
            parts.push(fallback.as_str());
            return Some(parts.join("+"));
        }
    };
    parts.push(name);
    Some(parts.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keycode_chord_plain_letter() {
        assert_eq!(keycode_to_chord(38, 0).as_deref(), Some("l"));
    }

    #[test]
    fn keycode_chord_with_ctrl() {
        assert_eq!(keycode_to_chord(38, MOD_CTRL).as_deref(), Some("ctrl+l"));
    }

    #[test]
    fn keycode_chord_with_super() {
        assert_eq!(keycode_to_chord(36, MOD_SUPER).as_deref(), Some("super+j"));
    }

    #[test]
    fn keycode_chord_orders_modifiers_consistently() {
        // ctrl, alt, shift, super — ordering is fixed regardless of
        // the bitmask order.
        let all = MOD_SUPER | MOD_SHIFT | MOD_ALT | MOD_CTRL;
        assert_eq!(
            keycode_to_chord(38, all).as_deref(),
            Some("ctrl+alt+shift+super+l")
        );
    }

    #[test]
    fn keycode_chord_unknown_key_falls_through() {
        // KEY_RESERVED + unmapped fallback path emits keNN so events
        // aren't silently dropped.
        assert_eq!(keycode_to_chord(200, 0).as_deref(), Some("key200"));
        assert_eq!(
            keycode_to_chord(200, MOD_CTRL).as_deref(),
            Some("ctrl+key200")
        );
    }

    #[test]
    fn keycode_chord_function_keys() {
        assert_eq!(keycode_to_chord(59, 0).as_deref(), Some("F1"));
        assert_eq!(keycode_to_chord(68, 0).as_deref(), Some("F10"));
        assert_eq!(keycode_to_chord(87, 0).as_deref(), Some("F11"));
        assert_eq!(keycode_to_chord(88, 0).as_deref(), Some("F12"));
    }

    #[test]
    fn keycode_chord_navigation() {
        assert_eq!(keycode_to_chord(28, 0).as_deref(), Some("Return"));
        assert_eq!(keycode_to_chord(1, 0).as_deref(), Some("Escape"));
        assert_eq!(
            keycode_to_chord(57, MOD_SUPER).as_deref(),
            Some("super+space")
        );
    }
}
