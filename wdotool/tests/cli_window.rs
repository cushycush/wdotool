//! Window subcommands: `search`, `getactivewindow`, `windowactivate`,
//! `windowclose`, `getwindowname`, `getwindowpid`, `getwindowclassname`,
//! `getwindowgeometry`. These exercise the CLI's filter logic, the
//! "exit 1 on no match / unsupported" contract, and the formatted
//! output xdotool consumers parse.

mod common;

use wdotool_core::backend::mock::{BackendCall, MockBackend};
use wdotool_core::detector::Environment;
use wdotool_core::types::{WindowGeometry, WindowId, WindowInfo};
use wdotool_core::WdoError;

fn win(id: &str, title: &str, app_id: Option<&str>, pid: Option<u32>) -> WindowInfo {
    WindowInfo {
        id: WindowId(id.into()),
        title: title.into(),
        app_id: app_id.map(str::to_string),
        pid,
    }
}

fn mock_with_windows(windows: Vec<WindowInfo>) -> MockBackend {
    let mock = MockBackend::new();
    mock.set_windows(windows);
    mock
}

#[tokio::test]
async fn search_no_filter_lists_all_windows_tab_separated() {
    let mock = mock_with_windows(vec![
        win("w1", "Firefox", Some("org.mozilla.firefox"), Some(100)),
        win("w2", "Terminal", Some("kitty"), Some(200)),
    ]);
    let r = common::run_with(&["search"], mock, Environment::default()).await;
    assert!(r.exit.is_success(), "stderr: {}", r.stderr);
    assert_eq!(r.stdout, "w1\tFirefox\nw2\tTerminal\n");
    assert_eq!(r.calls, vec![BackendCall::ListWindows]);
}

#[tokio::test]
async fn search_no_match_exits_with_code_1() {
    // xdotool exits 1 when search returns no rows so shell scripts
    // can branch on `if wdotool search ...`. The test asserts the
    // structured ExitCode rather than process::exit.
    let mock = mock_with_windows(vec![win("w1", "Firefox", None, None)]);
    let r = common::run_with(
        &["search", "--name", "Chromium"],
        mock,
        Environment::default(),
    )
    .await;
    assert!(!r.exit.is_success());
    assert_eq!(r.exit.0, 1);
    assert!(r.error.is_none(), "no match should not raise an error");
    assert_eq!(r.stdout, "");
}

#[tokio::test]
async fn search_default_substring_match_is_case_sensitive() {
    // Two runs against identical state: "Fire" matches "Firefox" but
    // lowercase "fire" doesn't. Build the mock twice (it isn't Clone).
    let r = common::run_with(
        &["search", "--name", "Fire"],
        mock_with_windows(vec![win("w1", "Firefox - Wikipedia", None, None)]),
        Environment::default(),
    )
    .await;
    assert!(r.exit.is_success());
    assert_eq!(r.stdout, "w1\tFirefox - Wikipedia\n");

    let r = common::run_with(
        &["search", "--name", "fire"],
        mock_with_windows(vec![win("w1", "Firefox - Wikipedia", None, None)]),
        Environment::default(),
    )
    .await;
    assert_eq!(r.exit.0, 1);
    assert_eq!(r.stdout, "");
}

#[tokio::test]
async fn search_ignore_case_makes_substring_match() {
    let mock = mock_with_windows(vec![win("w1", "Firefox", None, None)]);
    let r = common::run_with(
        &["search", "--ignore-case", "--name", "FIRE"],
        mock,
        Environment::default(),
    )
    .await;
    assert!(r.exit.is_success());
    assert_eq!(r.stdout, "w1\tFirefox\n");
}

#[tokio::test]
async fn search_regex_flag_treats_pattern_as_regex() {
    let mock = mock_with_windows(vec![
        win("w1", "Firefox", None, None),
        win("w2", "Fire fox", None, None),
        win("w3", "Mozilla", None, None),
    ]);
    // With --regex, `Fire.fox` is "Fire" + any single char + "fox":
    // "Fire fox" matches (space is the any-char), "Firefox" doesn't
    // (no separator between "Fire" and "fox"), "Mozilla" doesn't.
    let r = common::run_with(
        &["search", "--regex", "--name", "Fire.fox"],
        mock,
        Environment::default(),
    )
    .await;
    assert!(r.exit.is_success());
    assert_eq!(r.stdout, "w2\tFire fox\n");
}

#[tokio::test]
async fn search_any_combines_filters_with_or() {
    let mock = mock_with_windows(vec![
        win("w1", "Firefox", Some("none"), Some(999)),
        win("w2", "Slack", Some("kitty"), Some(999)),
    ]);
    // --any: at least one filter must match. PID 999 matches both;
    // class "kitty" matches only w2. Default AND would match neither,
    // since w1 doesn't have class "kitty" and w2 doesn't have name
    // "Firefox". --any makes both match.
    let r = common::run_with(
        &[
            "search", "--any", "--name", "Firefox", "--class", "kitty",
        ],
        mock,
        Environment::default(),
    )
    .await;
    assert!(r.exit.is_success());
    assert_eq!(r.stdout, "w1\tFirefox\nw2\tSlack\n");
}

#[tokio::test]
async fn getactivewindow_prints_id_when_active() {
    let mock = MockBackend::new();
    mock.set_active_window(Some(win("active-id", "Active", None, None)));
    let r = common::run_with(&["getactivewindow"], mock, Environment::default()).await;
    assert!(r.exit.is_success());
    assert_eq!(r.stdout, "active-id\n");
    assert_eq!(r.calls, vec![BackendCall::ActiveWindow]);
}

#[tokio::test]
async fn getactivewindow_with_no_active_returns_window_not_found() {
    let r = common::run(&["getactivewindow"]).await;
    let err = r.error.expect("expected an error");
    assert!(matches!(err, WdoError::WindowNotFound(_)), "got: {err:?}");
}

#[tokio::test]
async fn windowactivate_passes_id_verbatim() {
    let r = common::run(&["windowactivate", "some-id"]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![BackendCall::ActivateWindow(WindowId("some-id".into()))]
    );
}

#[tokio::test]
async fn windowclose_passes_id_verbatim() {
    let r = common::run(&["windowclose", "some-id"]).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.calls,
        vec![BackendCall::CloseWindow(WindowId("some-id".into()))]
    );
}

#[tokio::test]
async fn getwindowname_prints_title_for_known_id() {
    let mock = mock_with_windows(vec![win("w1", "Firefox - News", None, None)]);
    let r = common::run_with(
        &["getwindowname", "w1"],
        mock,
        Environment::default(),
    )
    .await;
    assert!(r.exit.is_success());
    assert_eq!(r.stdout, "Firefox - News\n");
}

#[tokio::test]
async fn getwindowname_unknown_id_errors() {
    let mock = mock_with_windows(vec![win("w1", "Firefox", None, None)]);
    let r = common::run_with(
        &["getwindowname", "missing"],
        mock,
        Environment::default(),
    )
    .await;
    let err = r.error.expect("expected an error");
    assert!(matches!(err, WdoError::WindowNotFound(_)), "got: {err:?}");
}

#[tokio::test]
async fn getwindowpid_prints_pid_when_set() {
    let mock = mock_with_windows(vec![win("w1", "Firefox", None, Some(4242))]);
    let r = common::run_with(&["getwindowpid", "w1"], mock, Environment::default()).await;
    assert!(r.exit.is_success());
    assert_eq!(r.stdout, "4242\n");
}

#[tokio::test]
async fn getwindowpid_exits_1_when_pid_absent() {
    // Some compositors don't expose pid even when they expose the
    // window. The CLI prints a stderr hint and exits 1 — not an error
    // that bubbles up through Result.
    let mock = mock_with_windows(vec![win("w1", "Firefox", None, None)]);
    let r = common::run_with(&["getwindowpid", "w1"], mock, Environment::default()).await;
    assert_eq!(r.exit.0, 1);
    assert!(r.error.is_none());
    assert_eq!(r.stdout, "");
    assert!(r.stderr.contains("pid not available"), "stderr: {}", r.stderr);
}

#[tokio::test]
async fn getwindowclassname_prints_app_id_when_set() {
    let mock = mock_with_windows(vec![win("w1", "Firefox", Some("org.mozilla.firefox"), None)]);
    let r = common::run_with(
        &["getwindowclassname", "w1"],
        mock,
        Environment::default(),
    )
    .await;
    assert!(r.exit.is_success());
    assert_eq!(r.stdout, "org.mozilla.firefox\n");
}

#[tokio::test]
async fn getwindowclassname_exits_1_when_app_id_absent() {
    let mock = mock_with_windows(vec![win("w1", "Firefox", None, None)]);
    let r = common::run_with(
        &["getwindowclassname", "w1"],
        mock,
        Environment::default(),
    )
    .await;
    assert_eq!(r.exit.0, 1);
    assert!(r.error.is_none());
    assert!(r.stderr.contains("classname"), "stderr: {}", r.stderr);
}

#[tokio::test]
async fn getwindowgeometry_prints_xdotool_format_when_supported() {
    let mock = MockBackend::new();
    mock.set_geometry(
        "w1",
        WindowGeometry {
            x: 100,
            y: 200,
            width: 800,
            height: 600,
        },
    );
    let r = common::run_with(
        &["getwindowgeometry", "w1"],
        mock,
        Environment::default(),
    )
    .await;
    assert!(r.exit.is_success(), "stderr: {}", r.stderr);
    // xdotool's exact format. The "Screen: N" line xdotool prints is
    // dropped on Wayland (no stable screen index exposed to clients).
    assert_eq!(
        r.stdout,
        "Window w1\n  Position: 100,200\n  Geometry: 800x600\n"
    );
}

#[tokio::test]
async fn getwindowgeometry_exits_1_when_backend_returns_none() {
    // No geometry set on the mock means window_geometry() returns
    // Ok(None) — the "backend doesn't support reading geometry"
    // signal. The CLI prints a friendly stderr hint and exits 1.
    let r = common::run(&["getwindowgeometry", "w1"]).await;
    assert_eq!(r.exit.0, 1);
    assert!(r.error.is_none());
    assert!(
        r.stderr.contains("window geometry is unreadable"),
        "stderr: {}",
        r.stderr
    );
}
