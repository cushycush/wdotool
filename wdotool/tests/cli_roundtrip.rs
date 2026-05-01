//! Layer 3 round-trip integration tests: drive `wdotool` against a
//! real headless sway compositor and assert on what events the
//! observer client actually received. This is the layer that catches
//! bugs Layer 2 (mock-backend) can't reach: virtual-keyboard
//! transient-keymap injection, modifier-state divergence between
//! what wdotool thinks is pressed and what the compositor delivers,
//! scroll axis sign, and the focus model on `windowactivate`.
//!
//! Each test starts its own sway session so they're independent.
//! Sway boots in well under a second on a warm cache, so per-test
//! isolation is cheap.
//!
//! When `sway` isn't installed the tests skip themselves with a
//! `println!` (visible in `cargo test -- --nocapture`) rather than
//! failing. CI installs sway before running the suite.

#![cfg(target_os = "linux")]

use std::time::Duration;

use wdotool_test_harness::{HarnessError, HeadlessSway};

/// Try to start sway. If sway isn't installed, print a skip line and
/// return None so the test exits without failing. Every test in this
/// file uses this helper.
fn try_sway() -> Option<HeadlessSway> {
    match HeadlessSway::start() {
        Ok(s) => Some(s),
        Err(HarnessError::SwayUnavailable(_)) => {
            println!(
                "skipping round-trip test: sway is not installed. \
                 install with `pacman -S sway` (Arch) or `apt install sway` (Debian/Ubuntu)."
            );
            None
        }
        Err(other) => panic!("sway failed to start: {other}"),
    }
}

#[test]
fn observer_reaches_ready_inside_headless_sway() {
    let Some(sway) = try_sway() else { return };
    let observer = sway.spawn_observer().expect("spawn observer");
    let prelude = observer
        .wait_for_ready(Duration::from_secs(3))
        .expect("observer reached ready");
    // Among the prelude lines we expect at minimum a `keymap_changed`
    // (sway sends one once we get keyboard focus) and the `ready`
    // marker.
    assert!(prelude.iter().any(|l| l == "ready"), "prelude: {prelude:?}");
}

#[test]
fn key_a_round_trips_through_wlroots_backend() {
    let Some(sway) = try_sway() else { return };
    let observer = sway.spawn_observer().expect("spawn observer");
    observer
        .wait_for_ready(Duration::from_secs(3))
        .expect("observer ready");
    // Drain any residual prelude noise (modifiers, keyboard_enter).
    let _ = observer.collect_events(Duration::from_millis(50));

    let out = sway.run_wdotool(&["key", "a"]).expect("run wdotool");
    assert!(out.status.success(), "wdotool failed: {:?}", out);

    let events = observer.collect_events(Duration::from_millis(300));
    let press = events
        .iter()
        .find(|l| l.starts_with("key ") && l.ends_with(" press"));
    let release = events
        .iter()
        .find(|l| l.starts_with("key ") && l.ends_with(" release"));
    assert!(
        press.is_some() && release.is_some(),
        "expected one press and one release; got: {events:?}"
    );
    // The keysym name in the middle column is what we really care
    // about: it should be "a" regardless of the linux keycode (which
    // is layout-dependent if anyone ever runs this with a non-US
    // keymap).
    assert!(press.unwrap().contains(" a "), "press line: {press:?}");
    assert!(release.unwrap().contains(" a "), "release line: {release:?}");
}

#[test]
fn key_ctrl_shift_a_emits_modifiers_in_xdotool_order() {
    let Some(sway) = try_sway() else { return };
    let observer = sway.spawn_observer().expect("spawn observer");
    observer
        .wait_for_ready(Duration::from_secs(3))
        .expect("observer ready");
    let _ = observer.collect_events(Duration::from_millis(50));

    let out = sway.run_wdotool(&["key", "ctrl+shift+a"]).expect("run wdotool");
    assert!(out.status.success(), "wdotool failed: {:?}", out);

    let events = observer.collect_events(Duration::from_millis(300));
    // Just the key lines, in order.
    let keys: Vec<&String> = events.iter().filter(|l| l.starts_with("key ")).collect();

    // Expected order: Press(Control_L), Press(Shift_L), Press(a),
    // Release(a), Release(Shift_L), Release(Control_L). Six lines,
    // bookended by the modifier release.
    assert_eq!(keys.len(), 6, "events: {events:?}");
    assert!(
        keys[0].contains("Control_L") && keys[0].ends_with(" press"),
        "first: {}", keys[0]
    );
    assert!(
        keys[1].contains("Shift_L") && keys[1].ends_with(" press"),
        "second: {}", keys[1]
    );
    assert!(
        keys[2].contains(" a ") && keys[2].ends_with(" press"),
        "third: {}", keys[2]
    );
    assert!(
        keys[3].contains(" a ") && keys[3].ends_with(" release"),
        "fourth: {}", keys[3]
    );
    assert!(
        keys[4].contains("Shift_L") && keys[4].ends_with(" release"),
        "fifth: {}", keys[4]
    );
    assert!(
        keys[5].contains("Control_L") && keys[5].ends_with(" release"),
        "sixth: {}", keys[5]
    );
}

#[test]
fn scroll_emits_axis_event_with_correct_sign() {
    let Some(sway) = try_sway() else { return };
    let observer = sway.spawn_observer().expect("spawn observer");
    observer
        .wait_for_ready(Duration::from_secs(3))
        .expect("observer ready");
    let _ = observer.collect_events(Duration::from_millis(50));

    let out = sway.run_wdotool(&["scroll", "0", "3"]).expect("run wdotool");
    assert!(out.status.success(), "wdotool failed: {:?}", out);

    let events = observer.collect_events(Duration::from_millis(300));
    // Look for a vertical axis event with positive value (positive dy
    // = scroll down per wdotool's documented sign convention).
    let axis = events
        .iter()
        .find(|l| l.starts_with("pointer_axis vertical "))
        .unwrap_or_else(|| panic!("no vertical axis event in: {events:?}"));
    let value: f64 = axis.split_whitespace().nth(2).unwrap().parse().unwrap();
    assert!(value > 0.0, "expected positive vertical scroll, got {value}");
}
