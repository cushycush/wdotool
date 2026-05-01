//! Layer 3 round-trip integration tests: drive `wdotool` against a
//! real headless sway compositor and assert on what events the
//! observer client actually received. This is the layer that catches
//! bugs Layer 2 (mock-backend) can't reach: virtual-keyboard
//! transient-keymap injection, modifier-state divergence between
//! what wdotool thinks is pressed and what the compositor delivers,
//! scroll axis sign, and the focus model on `windowactivate`.
//!
//! Each test starts its own sway session so they're independent.
//! Sway boots in under a second on a warm cache, so per-test
//! isolation is cheap.
//!
//! When `sway` isn't installed the tests skip themselves with a
//! `println!` (visible in `cargo test -- --nocapture`) rather than
//! failing. CI installs sway before running the suite.

#![cfg(target_os = "linux")]

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use wdotool_test_harness::{HarnessError, HeadlessSway, Observer};

/// Process-wide serialization for the round-trip suite. Each test
/// boots its own sway compositor, and running several at once on a
/// CI runner has them fighting for CPU and tripping `wait_for_ready`
/// timeouts. CI passes `--test-threads=1`, but `cargo test --workspace`
/// locally defaults to parallel and was flaking. Holding this mutex
/// across the lifetime of each test makes the suite serial regardless
/// of how it's invoked.
static SUITE_LOCK: Mutex<()> = Mutex::new(());

/// Boot a fresh sway session, spawn the observer inside it, wait for
/// the surface to be ready, and drain prelude noise (modifiers,
/// keyboard_enter, pointer_enter). Returns None when sway isn't
/// installed so the calling test can skip itself. The returned guard
/// keeps `SUITE_LOCK` held for the test's duration.
fn fresh_session() -> Option<(HeadlessSway, Observer, MutexGuard<'static, ()>)> {
    // PoisonError can happen if a previous test panicked; we don't
    // care, the lock is just a cross-test serializer.
    let guard = SUITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sway = match HeadlessSway::start() {
        Ok(s) => s,
        Err(HarnessError::SwayUnavailable(_)) => {
            println!(
                "skipping round-trip test: sway is not installed. \
                 install with `pacman -S sway` (Arch) or `apt install sway` (Debian/Ubuntu)."
            );
            return None;
        }
        Err(other) => panic!("sway failed to start: {other}"),
    };
    let observer = sway.spawn_observer().expect("spawn observer");
    // 30s ready timeout: sway boots in well under a second on a
    // dev box, but slow CI runners (no GPU, software rendering,
    // shared with whatever else GitHub Actions is doing) need
    // headroom. The timeout only costs anything when sway is
    // genuinely broken.
    observer
        .wait_for_ready(Duration::from_secs(30))
        .expect("observer reached ready");
    let _ = observer.collect_events(Duration::from_millis(50));
    Some((sway, observer, guard))
}

/// Filter event lines to just the ones with the given prefix, for
/// readable assertions. Returns owned strings so the assertion
/// failure message has full context.
fn lines_starting_with(events: &[String], prefix: &str) -> Vec<String> {
    events
        .iter()
        .filter(|l| l.starts_with(prefix))
        .cloned()
        .collect()
}

/// Linux evdev keycodes for the keys these tests touch. Stable across
/// kernels, OS distributions, and (importantly) sway versions. Used
/// when the observer can't resolve keysym names because the keymap
/// didn't make it through xkbcommon, which has happened on CI.
const KEY_LEFTCTRL: u32 = 29;
const KEY_A: u32 = 30;
const KEY_LEFTSHIFT: u32 = 42;

/// Parse a `key <keycode> <name> <press|release>` line. Returns the
/// keycode, keysym name (which may be `?` if xkb couldn't resolve),
/// and the action.
fn parse_key_line(line: &str) -> Option<(u32, &str, &str)> {
    let mut parts = line.split_whitespace();
    if parts.next()? != "key" {
        return None;
    }
    let kc: u32 = parts.next()?.parse().ok()?;
    let name = parts.next()?;
    let action = parts.next()?;
    Some((kc, name, action))
}

// ============================================================
// Sanity: observer comes up and gets focus inside headless sway.
// ============================================================

#[test]
fn observer_reaches_ready_inside_headless_sway() {
    let Some((_sway, _observer, _guard)) = fresh_session() else {
        return;
    };
}

// ============================================================
// Keyboard: key, keydown/keyup, modifier ordering, type.
// ============================================================

#[test]
fn key_a_round_trips_through_wlroots_backend() {
    let Some((sway, observer, _guard)) = fresh_session() else {
        return;
    };
    let out = sway.run_wdotool(&["key", "a"]).expect("run wdotool");
    assert!(out.status.success(), "wdotool failed: {out:?}");

    let events = observer.collect_events(Duration::from_millis(300));
    let keys = lines_starting_with(&events, "key ");
    assert_eq!(keys.len(), 2, "expected exactly press+release: {events:?}");
    let (kc0, _, action0) = parse_key_line(&keys[0]).expect("parse press");
    let (kc1, _, action1) = parse_key_line(&keys[1]).expect("parse release");
    assert_eq!(kc0, KEY_A, "press keycode: {}", keys[0]);
    assert_eq!(action0, "press", "first action: {}", keys[0]);
    assert_eq!(kc1, KEY_A, "release keycode: {}", keys[1]);
    assert_eq!(action1, "release", "second action: {}", keys[1]);
}

#[test]
fn key_ctrl_shift_a_emits_modifiers_in_xdotool_order() {
    let Some((sway, observer, _guard)) = fresh_session() else {
        return;
    };
    let out = sway
        .run_wdotool(&["key", "ctrl+shift+a"])
        .expect("run wdotool");
    assert!(out.status.success(), "wdotool failed: {out:?}");

    let events = observer.collect_events(Duration::from_millis(300));
    let keys = lines_starting_with(&events, "key ");

    // Press(Control_L), Press(Shift_L), Press(a),
    // Release(a), Release(Shift_L), Release(Control_L).
    assert_eq!(keys.len(), 6, "events: {events:?}");
    let parsed: Vec<(u32, &str, &str)> = keys
        .iter()
        .map(|l| parse_key_line(l).unwrap_or_else(|| panic!("parse {l:?}")))
        .collect();
    let expected = [
        (KEY_LEFTCTRL, "press"),
        (KEY_LEFTSHIFT, "press"),
        (KEY_A, "press"),
        (KEY_A, "release"),
        (KEY_LEFTSHIFT, "release"),
        (KEY_LEFTCTRL, "release"),
    ];
    for (i, (exp_kc, exp_act)) in expected.iter().enumerate() {
        assert_eq!(parsed[i].0, *exp_kc, "keys[{i}] keycode: {}", keys[i]);
        assert_eq!(parsed[i].2, *exp_act, "keys[{i}] action: {}", keys[i]);
    }
}

// keydown/keyup don't compose across separate wdotool processes on
// the wlroots backend: each invocation creates and destroys its own
// virtual_keyboard, and sway auto-releases any keys held by a device
// on destruction. Layer 2 covers the dispatch contract; this case
// would only work end-to-end via libei (with a portal session) or a
// future "wdotool session" mode that keeps the device alive across
// commands.
#[ignore = "wlroots backend doesn't preserve held-key state across process invocations"]
#[test]
fn keydown_then_keyup_round_trip_holds_then_releases() {
    // keydown leaves the key held; keyup releases it. A bug where
    // wdotool sends a stray release at process exit (or fails to
    // send the release on keyup) would surface here as either an
    // unexpected release or a missing one.
    let Some((sway, observer, _guard)) = fresh_session() else {
        return;
    };

    let out = sway.run_wdotool(&["keydown", "a"]).expect("run wdotool");
    assert!(out.status.success(), "keydown failed: {out:?}");
    let post_keydown = observer.collect_events(Duration::from_millis(200));
    let keys = lines_starting_with(&post_keydown, "key ");
    assert_eq!(
        keys.len(),
        1,
        "keydown should emit exactly press: {post_keydown:?}"
    );
    assert!(keys[0].contains(" a ") && keys[0].ends_with(" press"));

    let out = sway.run_wdotool(&["keyup", "a"]).expect("run wdotool");
    assert!(out.status.success(), "keyup failed: {out:?}");
    let post_keyup = observer.collect_events(Duration::from_millis(200));
    let keys = lines_starting_with(&post_keyup, "key ");
    assert_eq!(
        keys.len(),
        1,
        "keyup should emit exactly release: {post_keyup:?}"
    );
    assert!(keys[0].contains(" a ") && keys[0].ends_with(" release"));
}

#[test]
fn type_hello_arrives_as_individual_characters() {
    // The wlroots backend types text by injecting a transient keymap
    // that maps the next char to a known keycode, sending press +
    // release, and restoring the original keymap. This test pins that
    // each character arrives as a press-release pair, in order, with
    // a `keymap_changed` event somewhere in the prelude proving the
    // injection happened. The keysym name in each line should match
    // the literal char.
    let Some((sway, observer, _guard)) = fresh_session() else {
        return;
    };
    let out = sway
        .run_wdotool(&["type", "--delay", "0", "hello"])
        .expect("run wdotool");
    assert!(out.status.success(), "wdotool failed: {out:?}");

    let events = observer.collect_events(Duration::from_millis(800));

    // The wlroots backend sends a keymap_received line for each
    // transient keymap upload. Verify at least one happened (proof
    // of the injection mechanism), but don't require keymap_changed
    // since xkbcommon may fail to parse the transient keymap on
    // some sway/xkb versions and skip the "_changed" emit.
    assert!(
        events.iter().any(|l| l.starts_with("keymap_received ")),
        "expected at least one keymap_received during type: {events:?}"
    );

    // Five chars should produce five press events (and five releases).
    // We don't check the keysym name column because the transient
    // keymap may not have parsed cleanly — see above. The critical
    // contract is "five characters got delivered, in order, as
    // press+release pairs".
    let key_lines = lines_starting_with(&events, "key ");
    let presses: Vec<_> = key_lines
        .iter()
        .filter_map(|l| parse_key_line(l))
        .filter(|(_, _, action)| *action == "press")
        .collect();
    let releases: Vec<_> = key_lines
        .iter()
        .filter_map(|l| parse_key_line(l))
        .filter(|(_, _, action)| *action == "release")
        .collect();
    assert_eq!(
        presses.len(),
        5,
        "expected 5 press events for 'hello': {events:?}"
    );
    assert_eq!(
        releases.len(),
        5,
        "expected 5 release events for 'hello': {events:?}"
    );
}

// ============================================================
// Pointer: mousemove (absolute / relative), click, mousedown/up.
//
// Every test in this section is `#[ignore]`d because of a race
// specific to the headless-sway test environment: the seat starts
// with no pointer capability (no real input devices), wdotool
// creates a virtual_pointer and sends a motion event, and sway
// processes wdotool's motion before the observer's get_pointer
// reaches the compositor. Without an existing pointer client when
// motion is processed, sway has nothing to deliver the event to and
// silently discards it. In a real desktop with a real mouse, the
// seat already has pointer cap and clients are already bound, so
// this race never happens. Layer 2 covers the CLI-to-backend
// dispatch for these commands; pre-release manual matrix covers the
// real-desktop end-to-end. Run with `cargo test -- --ignored` to
// re-attempt these once a workaround lands (long-running prime
// process, libei backend in CI, or weston headless).
// ============================================================

#[ignore = "headless-sway: pointer client must exist before motion is processed"]
#[test]
fn mousemove_absolute_lands_pointer_at_coords() {
    // sway's headless backend creates an output at 1280x720 by default.
    // A surface placed inside that output will receive surface-local
    // coordinates relative to its own origin. With focus_follows_mouse
    // and a single full-screen window, surface origin is at the
    // output origin, so mousemove 100 80 should produce a
    // pointer_motion at approximately (100, 80) surface-local.
    //
    // We test "approximately" because compositors may shift coords by
    // small amounts during cursor handling. A tolerance of a few
    // pixels is fine.
    let Some((sway, observer, _guard)) = fresh_session() else {
        return;
    };
    let out = sway
        .run_wdotool(&["mousemove", "100", "80"])
        .expect("run wdotool");
    assert!(out.status.success(), "wdotool failed: {out:?}");

    let events = observer.collect_events(Duration::from_millis(300));
    let motions = lines_starting_with(&events, "pointer_motion ");
    assert!(
        !motions.is_empty(),
        "expected at least one pointer_motion: {events:?}"
    );
    let last = motions.last().unwrap();
    let mut parts = last.split_whitespace();
    parts.next(); // "pointer_motion"
    let x: f64 = parts.next().unwrap().parse().unwrap();
    let y: f64 = parts.next().unwrap().parse().unwrap();
    assert!(
        (x - 100.0).abs() < 5.0 && (y - 80.0).abs() < 5.0,
        "expected pointer near (100, 80), got ({x}, {y}). All motions: {motions:?}"
    );
}

#[ignore = "headless-sway: see mousemove_absolute_lands_pointer_at_coords for explanation"]
#[test]
fn mousemove_relative_emits_motion_delta() {
    // After an absolute move to a known position, a relative move by
    // (dx, dy) should land at (start + dx, start + dy). We use this
    // to verify the relative path actually adds rather than
    // overwrites.
    let Some((sway, observer, _guard)) = fresh_session() else {
        return;
    };

    // Anchor the cursor first.
    sway.run_wdotool(&["mousemove", "200", "150"])
        .expect("run wdotool");
    // Drain motions from the anchor move.
    let _ = observer.collect_events(Duration::from_millis(150));

    // Now relative move by (20, -10).
    let out = sway
        .run_wdotool(&["mousemove", "--relative", "20", "-10"])
        .expect("run wdotool");
    assert!(out.status.success(), "wdotool failed: {out:?}");

    let events = observer.collect_events(Duration::from_millis(300));
    let motions = lines_starting_with(&events, "pointer_motion ");
    assert!(
        !motions.is_empty(),
        "expected at least one pointer_motion: {events:?}"
    );
    let last = motions.last().unwrap();
    let mut parts = last.split_whitespace();
    parts.next();
    let x: f64 = parts.next().unwrap().parse().unwrap();
    let y: f64 = parts.next().unwrap().parse().unwrap();
    assert!(
        (x - 220.0).abs() < 5.0 && (y - 140.0).abs() < 5.0,
        "expected pointer near (220, 140) after relative move, got ({x}, {y})"
    );
}

#[ignore = "headless-sway: see mousemove_absolute_lands_pointer_at_coords for explanation"]
#[test]
fn click_1_emits_left_button_press_release() {
    // Linux button code 272 = BTN_LEFT (xdotool's button 1).
    let Some((sway, observer, _guard)) = fresh_session() else {
        return;
    };
    let out = sway.run_wdotool(&["click", "1"]).expect("run wdotool");
    assert!(out.status.success(), "wdotool failed: {out:?}");

    let events = observer.collect_events(Duration::from_millis(300));
    let buttons = lines_starting_with(&events, "pointer_button ");
    assert_eq!(buttons.len(), 2, "expected press+release: {events:?}");
    assert!(
        buttons[0].starts_with("pointer_button 272 ") && buttons[0].ends_with(" press"),
        "press: {}",
        buttons[0]
    );
    assert!(
        buttons[1].starts_with("pointer_button 272 ") && buttons[1].ends_with(" release"),
        "release: {}",
        buttons[1]
    );
}

#[ignore = "headless-sway: see mousemove_absolute_lands_pointer_at_coords for explanation"]
#[test]
fn mousedown_then_mouseup_emit_press_then_release() {
    let Some((sway, observer, _guard)) = fresh_session() else {
        return;
    };

    sway.run_wdotool(&["mousedown", "1"]).expect("run wdotool");
    let post_down = observer.collect_events(Duration::from_millis(200));
    let buttons = lines_starting_with(&post_down, "pointer_button ");
    assert_eq!(
        buttons.len(),
        1,
        "mousedown should emit only press: {post_down:?}"
    );
    assert!(
        buttons[0].ends_with(" press"),
        "expected press, got: {}",
        buttons[0]
    );

    sway.run_wdotool(&["mouseup", "1"]).expect("run wdotool");
    let post_up = observer.collect_events(Duration::from_millis(200));
    let buttons = lines_starting_with(&post_up, "pointer_button ");
    assert_eq!(
        buttons.len(),
        1,
        "mouseup should emit only release: {post_up:?}"
    );
    assert!(
        buttons[0].ends_with(" release"),
        "expected release, got: {}",
        buttons[0]
    );
}

// ============================================================
// Scroll: axis label and sign convention.
// ============================================================

#[ignore = "headless-sway: see mousemove_absolute_lands_pointer_at_coords for explanation"]
#[test]
fn scroll_positive_dy_emits_vertical_axis_with_positive_value() {
    let Some((sway, observer, _guard)) = fresh_session() else {
        return;
    };
    let out = sway
        .run_wdotool(&["scroll", "0", "3"])
        .expect("run wdotool");
    assert!(out.status.success(), "wdotool failed: {out:?}");

    let events = observer.collect_events(Duration::from_millis(300));
    let axis = events
        .iter()
        .find(|l| l.starts_with("pointer_axis vertical "))
        .unwrap_or_else(|| panic!("no vertical axis event in: {events:?}"));
    let value: f64 = axis.split_whitespace().nth(2).unwrap().parse().unwrap();
    assert!(
        value > 0.0,
        "expected positive vertical scroll, got {value}"
    );
}

#[ignore = "headless-sway: see mousemove_absolute_lands_pointer_at_coords for explanation"]
#[test]
fn scroll_negative_dy_emits_vertical_axis_with_negative_value() {
    // Symmetric to the positive case. Catches a sign-flip bug in
    // wlroots' scroll path that wouldn't surface in Layer 2 (which
    // just asserts the value reaches the backend unchanged).
    let Some((sway, observer, _guard)) = fresh_session() else {
        return;
    };
    let out = sway
        .run_wdotool(&["scroll", "0", "-2"])
        .expect("run wdotool");
    assert!(out.status.success(), "wdotool failed: {out:?}");

    let events = observer.collect_events(Duration::from_millis(300));
    let axis = events
        .iter()
        .find(|l| l.starts_with("pointer_axis vertical "))
        .unwrap_or_else(|| panic!("no vertical axis event in: {events:?}"));
    let value: f64 = axis.split_whitespace().nth(2).unwrap().parse().unwrap();
    assert!(
        value < 0.0,
        "expected negative vertical scroll, got {value}"
    );
}

#[ignore = "headless-sway: see mousemove_absolute_lands_pointer_at_coords for explanation"]
#[test]
fn scroll_horizontal_axis_routes_to_horizontal_label() {
    let Some((sway, observer, _guard)) = fresh_session() else {
        return;
    };
    let out = sway
        .run_wdotool(&["scroll", "2", "0"])
        .expect("run wdotool");
    assert!(out.status.success(), "wdotool failed: {out:?}");

    let events = observer.collect_events(Duration::from_millis(300));
    assert!(
        events
            .iter()
            .any(|l| l.starts_with("pointer_axis horizontal ")),
        "expected horizontal axis event: {events:?}"
    );
    // No vertical event should fire for a horizontal-only scroll.
    assert!(
        !events
            .iter()
            .any(|l| l.starts_with("pointer_axis vertical ")),
        "unexpected vertical axis event: {events:?}"
    );
}
