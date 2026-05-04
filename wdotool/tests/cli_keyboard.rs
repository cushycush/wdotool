//! Keyboard subcommands: `key`, `keydown`, `keyup`, `type`. Each test
//! drives `dispatch` over a `MockBackend` and asserts on the exact
//! sequence of `BackendCall`s the CLI produced. This is where regressions
//! in modifier ordering, --clearmodifiers, and type-string handling are
//! caught — without ever touching a real compositor.

mod common;

use std::io::Write;
use std::time::Duration;

use wdotool_core::backend::mock::BackendCall;
use wdotool_core::types::KeyDirection;
use wdotool_core::WdoError;

#[tokio::test]
async fn key_plain_letter_records_press_release() {
    let r = common::run(&["key", "a"]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![BackendCall::Key {
            keysym: "a".into(),
            dir: KeyDirection::PressRelease,
        }]
    );
}

#[tokio::test]
async fn key_with_modifier_chain_uses_press_then_press_release_then_release() {
    // xdotool ordering: press modifiers in declared order, then
    // press+release the key, then release modifiers in reverse.
    let r = common::run(&["key", "ctrl+shift+a"]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![
            BackendCall::Key {
                keysym: "Control_L".into(),
                dir: KeyDirection::Press,
            },
            BackendCall::Key {
                keysym: "Shift_L".into(),
                dir: KeyDirection::Press,
            },
            BackendCall::Key {
                keysym: "a".into(),
                dir: KeyDirection::PressRelease,
            },
            BackendCall::Key {
                keysym: "Shift_L".into(),
                dir: KeyDirection::Release,
            },
            BackendCall::Key {
                keysym: "Control_L".into(),
                dir: KeyDirection::Release,
            },
        ]
    );
}

#[tokio::test]
async fn keydown_only_presses() {
    // keydown does NOT release. Used by scripts that want to hold a
    // key across other operations and release it manually with keyup.
    let r = common::run(&["keydown", "ctrl+a"]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![
            BackendCall::Key {
                keysym: "Control_L".into(),
                dir: KeyDirection::Press,
            },
            BackendCall::Key {
                keysym: "a".into(),
                dir: KeyDirection::Press,
            },
        ]
    );
}

#[tokio::test]
async fn keyup_only_releases_in_reverse() {
    // keyup releases the key first, then modifiers in reverse — the
    // mirror image of keydown.
    let r = common::run(&["keyup", "ctrl+shift+a"]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![
            BackendCall::Key {
                keysym: "a".into(),
                dir: KeyDirection::Release,
            },
            BackendCall::Key {
                keysym: "Shift_L".into(),
                dir: KeyDirection::Release,
            },
            BackendCall::Key {
                keysym: "Control_L".into(),
                dir: KeyDirection::Release,
            },
        ]
    );
}

#[tokio::test]
async fn key_clearmodifiers_releases_standard_set_first() {
    // --clearmodifiers releases every standard modifier (best-effort,
    // since Wayland clients can't read modifier state) before the
    // actual key op runs.
    let r = common::run(&["key", "--clearmodifiers", "a"]).await;
    assert!(r.exit.is_success());

    // First nine calls are the standard-modifier release sequence.
    let expected_modifiers = [
        "Control_L",
        "Control_R",
        "Shift_L",
        "Shift_R",
        "Alt_L",
        "Alt_R",
        "Super_L",
        "Super_R",
        "ISO_Level3_Shift",
    ];
    for (i, sym) in expected_modifiers.iter().enumerate() {
        assert_eq!(
            r.calls[i],
            BackendCall::Key {
                keysym: (*sym).into(),
                dir: KeyDirection::Release,
            },
            "modifier {i} ({sym}) was not released first"
        );
    }

    // Then the actual key op.
    assert_eq!(
        r.calls[expected_modifiers.len()],
        BackendCall::Key {
            keysym: "a".into(),
            dir: KeyDirection::PressRelease,
        }
    );
    assert_eq!(r.calls.len(), expected_modifiers.len() + 1);
}

#[tokio::test]
async fn type_records_text_with_default_delay_of_12ms() {
    let r = common::run(&["type", "hello"]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![BackendCall::TypeText {
            text: "hello".into(),
            delay: Duration::from_millis(12),
        }]
    );
}

#[tokio::test]
async fn type_honors_explicit_delay() {
    let r = common::run(&["type", "--delay", "50", "hi"]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![BackendCall::TypeText {
            text: "hi".into(),
            delay: Duration::from_millis(50),
        }]
    );
}

#[tokio::test]
async fn type_passes_multibyte_utf8_through_unchanged() {
    // The CLI shouldn't transform the bytes — backends receive the raw
    // string and decide how to map each grapheme to keysyms (e.g. by
    // injecting a transient keymap on wlr-protocols).
    let text = "hello 世界 🌍";
    let r = common::run(&["type", text]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![BackendCall::TypeText {
            text: text.into(),
            delay: Duration::from_millis(12),
        }]
    );
}

#[tokio::test]
async fn type_clearmodifiers_releases_first_then_types() {
    let r = common::run(&["type", "--clearmodifiers", "x"]).await;
    assert!(r.exit.is_success());
    // Last call must be the type op itself.
    let last = r.calls.last().unwrap();
    assert_eq!(
        last,
        &BackendCall::TypeText {
            text: "x".into(),
            delay: Duration::from_millis(12),
        }
    );
    // Everything before it must be a Key Release.
    for (i, c) in r.calls[..r.calls.len() - 1].iter().enumerate() {
        match c {
            BackendCall::Key {
                dir: KeyDirection::Release,
                ..
            } => {}
            other => panic!("call {i} is not a Release: {other:?}"),
        }
    }
}

#[tokio::test]
async fn type_reads_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input.txt");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b"from-file\n").unwrap();
    drop(f);

    let r = common::run(&["type", "--file", path.to_str().unwrap()]).await;
    assert!(r.exit.is_success(), "stderr: {}", r.stderr);
    assert_eq!(
        r.calls,
        vec![BackendCall::TypeText {
            // Trailing newline preserved — that's xdotool's behavior
            // for `type --file`, and it lets users include intentional
            // newlines in their input.
            text: "from-file\n".into(),
            delay: Duration::from_millis(12),
        }]
    );
}

#[tokio::test]
async fn type_with_neither_arg_errors() {
    let r = common::run(&["type"]).await;
    let err = r
        .error
        .expect("expected an error from `type` with no input");
    assert!(matches!(err, WdoError::InvalidArg(_)), "got: {err:?}");
}

#[tokio::test]
async fn key_with_invalid_chain_errors_before_calling_backend() {
    // Empty chain. Should fail keysym parsing without any backend call.
    let r = common::run(&["key", ""]).await;
    assert!(r.error.is_some(), "expected an error for empty chain");
    assert!(
        r.calls.is_empty(),
        "no backend calls should fire on parse failure: got {:?}",
        r.calls
    );
}

#[tokio::test]
async fn key_with_unknown_keysym_passes_through_to_backend() {
    // The CLI keysym parser validates structure (split on `+`, no
    // empty segments), not whether each token names a real keysym.
    // The backend gets the final say. wlr-protocols / uinput / libei each
    // apply their own xkb lookup. This test pins that contract: an
    // unknown name reaches the backend untouched.
    let r = common::run(&["key", "ThisIsNotARealKeysym1234"]).await;
    assert!(r.exit.is_success(), "error: {:?}", r.error);
    assert_eq!(
        r.calls,
        vec![BackendCall::Key {
            keysym: "ThisIsNotARealKeysym1234".into(),
            dir: KeyDirection::PressRelease,
        }]
    );
}
