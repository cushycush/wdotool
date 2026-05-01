//! Sanity check that the test scaffolding works end-to-end: parse,
//! dispatch, capture, assert. The real per-command suites live in
//! sibling files (`cli_keyboard.rs`, `cli_pointer.rs`, ...).

mod common;

use wdotool_core::backend::mock::BackendCall;
use wdotool_core::types::KeyDirection;

#[tokio::test]
async fn key_press_release_records_press_then_release() {
    let r = common::run(&["key", "a"]).await;
    assert!(r.error.is_none(), "dispatch errored: {:?}", r.error);
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![BackendCall::Key {
            keysym: "a".into(),
            dir: KeyDirection::PressRelease,
        }]
    );
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "");
}
