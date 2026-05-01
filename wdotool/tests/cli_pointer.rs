//! Pointer subcommands: `mousemove`, `click`, `mousedown`, `mouseup`,
//! `scroll`. Asserts on the exact backend call sequence so regressions
//! in absolute-vs-relative handling, --output translation, button
//! mapping, and scroll axis ordering are caught locally.

mod common;

use wdotool_core::backend::mock::{BackendCall, MockBackend};
use wdotool_core::detector::Environment;
use wdotool_core::types::{KeyDirection, MouseButton, OutputInfo};
use wdotool_core::WdoError;

#[tokio::test]
async fn mousemove_default_is_absolute() {
    let r = common::run(&["mousemove", "100", "200"]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![BackendCall::MouseMove {
            x: 100,
            y: 200,
            absolute: true,
        }]
    );
}

#[tokio::test]
async fn mousemove_relative_flag_flips_absolute_to_false() {
    let r = common::run(&["mousemove", "--relative", "10", "-5"]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![BackendCall::MouseMove {
            x: 10,
            y: -5,
            absolute: false,
        }]
    );
}

#[tokio::test]
async fn mousemove_with_output_translates_to_global_coords() {
    // The CLI consults list_outputs(), finds DP-2 at origin (1920, 0),
    // and adds the user-supplied (50, 60) before calling mouse_move
    // with absolute=true.
    let mock = MockBackend::new();
    mock.set_outputs(vec![
        OutputInfo {
            name: "DP-1".into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1,
        },
        OutputInfo {
            name: "DP-2".into(),
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
            scale: 1,
        },
    ]);
    let r = common::run_with(
        &["mousemove", "--output", "DP-2", "50", "60"],
        mock,
        Environment::default(),
    )
    .await;
    assert!(r.exit.is_success(), "stderr: {}", r.stderr);
    // Two calls: one to enumerate outputs, one to move.
    assert_eq!(
        r.calls,
        vec![
            BackendCall::ListOutputs,
            BackendCall::MouseMove {
                x: 1970,
                y: 60,
                absolute: true,
            },
        ]
    );
}

#[tokio::test]
async fn mousemove_with_output_on_no_enumerate_backend_errors() {
    // MockBackend defaults to empty outputs, which is the same shape
    // the libei/uinput backends expose. The CLI must reject --output
    // with a friendly message rather than silently moving to (50, 60).
    let r = common::run(&["mousemove", "--output", "DP-1", "50", "60"]).await;
    let err = r.error.expect("expected an error");
    match err {
        WdoError::InvalidArg(msg) => {
            assert!(msg.contains("--output"), "msg: {msg}");
            assert!(msg.contains("does not enumerate outputs"), "msg: {msg}");
        }
        other => panic!("expected InvalidArg, got {other:?}"),
    }
}

#[tokio::test]
async fn mousemove_with_unknown_output_lists_available_in_error() {
    let mock = MockBackend::new();
    mock.set_outputs(vec![OutputInfo {
        name: "DP-1".into(),
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        scale: 1,
    }]);
    let r = common::run_with(
        &["mousemove", "--output", "DP-99", "0", "0"],
        mock,
        Environment::default(),
    )
    .await;
    let err = r.error.expect("expected an error");
    match err {
        WdoError::InvalidArg(msg) => {
            assert!(msg.contains("DP-99"), "msg: {msg}");
            assert!(msg.contains("DP-1"), "msg: {msg}");
        }
        other => panic!("expected InvalidArg, got {other:?}"),
    }
}

#[tokio::test]
async fn click_1_is_left_button_press_release() {
    let r = common::run(&["click", "1"]).await;
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
async fn click_2_is_middle_button() {
    let r = common::run(&["click", "2"]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![BackendCall::MouseButton {
            btn: MouseButton::Middle,
            dir: KeyDirection::PressRelease,
        }]
    );
}

#[tokio::test]
async fn click_3_is_right_button() {
    let r = common::run(&["click", "3"]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![BackendCall::MouseButton {
            btn: MouseButton::Right,
            dir: KeyDirection::PressRelease,
        }]
    );
}

#[tokio::test]
async fn click_8_and_9_map_to_back_and_forward() {
    let r = common::run(&["click", "8"]).await;
    assert_eq!(
        r.calls,
        vec![BackendCall::MouseButton {
            btn: MouseButton::Back,
            dir: KeyDirection::PressRelease,
        }]
    );

    let r = common::run(&["click", "9"]).await;
    assert_eq!(
        r.calls,
        vec![BackendCall::MouseButton {
            btn: MouseButton::Forward,
            dir: KeyDirection::PressRelease,
        }]
    );
}

#[tokio::test]
async fn click_unknown_index_passes_through_as_other() {
    // 5 is xdotool's scroll-up index. The CLI doesn't translate it
    // (scroll has its own subcommand), so it surfaces as Other(5).
    let r = common::run(&["click", "5"]).await;
    assert_eq!(
        r.calls,
        vec![BackendCall::MouseButton {
            btn: MouseButton::Other(5),
            dir: KeyDirection::PressRelease,
        }]
    );
}

#[tokio::test]
async fn mousedown_is_press_only() {
    let r = common::run(&["mousedown", "1"]).await;
    assert_eq!(
        r.calls,
        vec![BackendCall::MouseButton {
            btn: MouseButton::Left,
            dir: KeyDirection::Press,
        }]
    );
}

#[tokio::test]
async fn mouseup_is_release_only() {
    let r = common::run(&["mouseup", "1"]).await;
    assert_eq!(
        r.calls,
        vec![BackendCall::MouseButton {
            btn: MouseButton::Left,
            dir: KeyDirection::Release,
        }]
    );
}

#[tokio::test]
async fn scroll_passes_axes_through() {
    // Sign convention: positive dy scrolls down, positive dx scrolls
    // right. The CLI is a thin pass-through; the actual axis sign is
    // documented on the subcommand and validated by integration with
    // a real compositor (Layer 4).
    let r = common::run(&["scroll", "0", "3"]).await;
    assert!(r.exit.is_success());
    assert_eq!(r.calls, vec![BackendCall::Scroll { dx: 0.0, dy: 3.0 }]);
}

#[tokio::test]
async fn scroll_supports_fractional_and_negative_values() {
    let r = common::run(&["scroll", "-1.5", "2.25"]).await;
    assert!(r.exit.is_success());
    assert_eq!(r.calls, vec![BackendCall::Scroll { dx: -1.5, dy: 2.25 }]);
}
