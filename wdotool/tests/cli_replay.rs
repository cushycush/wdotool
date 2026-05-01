//! `wdotool replay <file>` integration tests against the mock
//! backend. Exercises the JSON parsing, the per-`RecEvent` dispatch
//! to the right `Backend` trait method, the timing-via-Gap behavior,
//! and the `--speed` multiplier.

mod common;

use std::io::Write;
use std::time::{Duration, Instant};

use wdotool_core::backend::mock::BackendCall;
use wdotool_core::types::{KeyDirection, MouseButton};
use wdotool_core::WdoError;

/// Build a tempfile holding the given JSON trace and return its path.
/// The TempDir is dropped on scope exit, which deletes the file; bind
/// the returned `(path, _dir)` so the file outlives the `replay` call.
fn write_trace(json: &str) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(json.as_bytes()).unwrap();
    drop(f);
    (path.to_string_lossy().into_owned(), dir)
}

#[tokio::test]
async fn replay_dispatches_key_event() {
    let (path, _dir) = write_trace(r#"[{"kind":"key","t_ms":0,"chord":"a"}]"#);
    let r = common::run(&["replay", &path]).await;
    assert!(r.exit.is_success(), "error: {:?}", r.error);
    // The replay path uses the same run_key helper as `wdotool key`,
    // so for a plain (no-modifier) chord we get exactly one
    // PressRelease call.
    assert_eq!(
        r.calls,
        vec![BackendCall::Key {
            keysym: "a".into(),
            dir: KeyDirection::PressRelease,
        }]
    );
}

#[tokio::test]
async fn replay_dispatches_chord_with_modifier_ordering() {
    // Modifier ordering matches `wdotool key`: press all modifiers in
    // declared order, press+release the leaf, release modifiers in
    // reverse. Same Layer 2 contract as cli_keyboard pins.
    let (path, _dir) = write_trace(r#"[{"kind":"key","t_ms":0,"chord":"ctrl+shift+a"}]"#);
    let r = common::run(&["replay", &path]).await;
    assert!(r.exit.is_success(), "error: {:?}", r.error);
    assert_eq!(
        r.calls,
        vec![
            BackendCall::Key {
                keysym: "Control_L".into(),
                dir: KeyDirection::Press
            },
            BackendCall::Key {
                keysym: "Shift_L".into(),
                dir: KeyDirection::Press
            },
            BackendCall::Key {
                keysym: "a".into(),
                dir: KeyDirection::PressRelease
            },
            BackendCall::Key {
                keysym: "Shift_L".into(),
                dir: KeyDirection::Release
            },
            BackendCall::Key {
                keysym: "Control_L".into(),
                dir: KeyDirection::Release
            },
        ]
    );
}

#[tokio::test]
async fn replay_dispatches_click_as_press_release_pair() {
    // RecEvent::Click is pseudo-atomic on replay: the type's docs say
    // "release is implicit on replay". We dispatch as a PressRelease.
    let (path, _dir) = write_trace(r#"[{"kind":"click","t_ms":0,"button":1}]"#);
    let r = common::run(&["replay", &path]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![BackendCall::MouseButton {
            btn: MouseButton::Left,
            dir: KeyDirection::PressRelease,
        }]
    );
}

#[tokio::test]
async fn replay_dispatches_move_abs_and_move_delta() {
    // MoveAbs -> mouse_move(absolute=true). MoveDelta -> mouse_move(absolute=false).
    let (path, _dir) = write_trace(
        r#"[
            {"kind":"move_abs","t_ms":0,"x":100,"y":200},
            {"kind":"move_delta","t_ms":1,"dx":-5,"dy":7}
        ]"#,
    );
    let r = common::run(&["replay", &path]).await;
    assert!(r.exit.is_success(), "error: {:?}", r.error);
    assert_eq!(
        r.calls,
        vec![
            BackendCall::MouseMove {
                x: 100,
                y: 200,
                absolute: true,
            },
            BackendCall::MouseMove {
                x: -5,
                y: 7,
                absolute: false,
            },
        ]
    );
}

#[tokio::test]
async fn replay_dispatches_scroll() {
    let (path, _dir) = write_trace(r#"[{"kind":"scroll","t_ms":0,"dx":0,"dy":3}]"#);
    let r = common::run(&["replay", &path]).await;
    assert!(r.exit.is_success());
    assert_eq!(r.calls, vec![BackendCall::Scroll { dx: 0.0, dy: 3.0 }]);
}

#[tokio::test]
async fn replay_sleeps_on_gap_events() {
    // Replay should pause on Gap. Use a 200ms gap and assert the
    // wall-clock of the replay call is at least 150ms (loose lower
    // bound to absorb sleep jitter on slow CI).
    let (path, _dir) = write_trace(
        r#"[
            {"kind":"key","t_ms":0,"chord":"a"},
            {"kind":"gap","t_ms":1,"ms":200},
            {"kind":"key","t_ms":201,"chord":"b"}
        ]"#,
    );
    let start = Instant::now();
    let r = common::run(&["replay", &path]).await;
    let elapsed = start.elapsed();
    assert!(r.exit.is_success());
    assert!(
        elapsed >= Duration::from_millis(150),
        "expected replay to sleep on Gap, but it took only {elapsed:?}"
    );
    // Both keys still arrived.
    assert_eq!(r.calls.len(), 2);
}

#[tokio::test]
async fn replay_speed_flag_scales_gap_durations() {
    // 200ms Gap at speed=4.0 should sleep for ~50ms, well under the
    // 150ms lower bound the unscaled test asserts. We assert the
    // upper bound here to confirm speed actually had an effect.
    let (path, _dir) = write_trace(
        r#"[
            {"kind":"key","t_ms":0,"chord":"a"},
            {"kind":"gap","t_ms":1,"ms":200},
            {"kind":"key","t_ms":201,"chord":"b"}
        ]"#,
    );
    let start = Instant::now();
    let r = common::run(&["replay", "--speed", "4.0", &path]).await;
    let elapsed = start.elapsed();
    assert!(r.exit.is_success());
    assert!(
        elapsed < Duration::from_millis(150),
        "expected speed=4 to skip most of the Gap, but took {elapsed:?}"
    );
    assert_eq!(r.calls.len(), 2);
}

#[tokio::test]
async fn replay_rejects_zero_or_negative_speed() {
    let (path, _dir) = write_trace(r#"[]"#);
    let r = common::run(&["replay", "--speed", "0", &path]).await;
    let err = r.error.expect("expected error for speed=0");
    assert!(matches!(err, WdoError::InvalidArg(_)), "got: {err:?}");
}

#[tokio::test]
async fn replay_rejects_malformed_json() {
    let (path, _dir) = write_trace(r#"this is not json"#);
    let r = common::run(&["replay", &path]).await;
    let err = r.error.expect("expected error for malformed JSON");
    match err {
        WdoError::InvalidArg(msg) => {
            assert!(msg.contains("RecEvent"), "msg: {msg}");
        }
        other => panic!("expected InvalidArg, got {other:?}"),
    }
}

#[tokio::test]
async fn replay_rejects_missing_file() {
    // No file at this path. Error should mention the path so the user
    // can debug.
    let r = common::run(&["replay", "/no/such/trace/here.json"]).await;
    let err = r.error.expect("expected error for missing file");
    match err {
        WdoError::InvalidArg(msg) => {
            assert!(msg.contains("/no/such/trace/here.json"), "msg: {msg}");
        }
        other => panic!("expected InvalidArg, got {other:?}"),
    }
}

#[tokio::test]
async fn replay_handles_empty_trace() {
    // An empty array is valid; just produces no backend calls.
    let (path, _dir) = write_trace(r#"[]"#);
    let r = common::run(&["replay", &path]).await;
    assert!(r.exit.is_success());
    assert_eq!(r.calls.len(), 0);
}
