//! Introspection subcommands: `info`, `capabilities`, `outputs`,
//! `getmouselocation`. These are the read-side commands wflows.com and
//! other tools parse, so the assertions are stricter on output format
//! than the action-side suites.

mod common;

use wdotool_core::backend::mock::{BackendCall, MockBackend};
use wdotool_core::detector::Environment;
use wdotool_core::types::OutputInfo;

#[tokio::test]
async fn capabilities_emits_valid_json_with_expected_top_level_keys() {
    let r = common::run(&["capabilities"]).await;
    assert!(r.exit.is_success(), "stderr: {}", r.stderr);

    let value: serde_json::Value =
        serde_json::from_str(&r.stdout).expect("capabilities stdout must be valid JSON");
    let obj = value.as_object().expect("top-level must be a JSON object");

    // The schema is documented at docs/capabilities-schema.json. Pin
    // the seven top-level fields wflows.com depends on.
    for key in [
        "schema_version",
        "wdotool_version",
        "backend",
        "platform",
        "input",
        "window",
        "extras",
    ] {
        assert!(obj.contains_key(key), "missing top-level key {key}");
    }

    // schema_version is the contract version; locking it to 1 catches
    // accidental bumps. A real bump should update this test.
    assert_eq!(obj["schema_version"], serde_json::json!(1));
}

#[tokio::test]
async fn capabilities_reflects_mock_backend_name() {
    let r = common::run(&["capabilities"]).await;
    let value: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(value["backend"]["selected"], serde_json::json!("mock"));
}

#[tokio::test]
async fn info_prints_human_readable_capabilities() {
    let r = common::run(&["info"]).await;
    assert!(r.exit.is_success());
    // Spot-check the labels — exact layout is documented as
    // "human-readable, not consumer-parsed", but the labels need to
    // stay stable for users who grep this output.
    assert!(r.stdout.contains("backend:"), "stdout: {}", r.stdout);
    assert!(r.stdout.contains("capabilities:"), "stdout: {}", r.stdout);
    assert!(r.stdout.contains("key_input:"), "stdout: {}", r.stdout);
    assert!(r.stdout.contains("scroll:"), "stdout: {}", r.stdout);
}

#[tokio::test]
async fn info_uses_environment_fields() {
    let env = Environment {
        desktop: Some("Hyprland".into()),
        session_type: Some("wayland".into()),
        wayland_display: Some("wayland-1".into()),
        compositor_hints: vec!["hyprland"],
    };
    let r = common::run_with(&["info"], MockBackend::new(), env).await;
    assert!(r.exit.is_success());
    assert!(r.stdout.contains("Hyprland"), "stdout: {}", r.stdout);
    assert!(r.stdout.contains("wayland-1"), "stdout: {}", r.stdout);
    assert!(r.stdout.contains("wayland:  true"), "stdout: {}", r.stdout);
}

#[tokio::test]
async fn outputs_default_format_is_tab_separated_with_header() {
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
            scale: 2,
        },
    ]);
    let r = common::run_with(&["outputs"], mock, Environment::default()).await;
    assert!(r.exit.is_success());
    assert_eq!(
        r.stdout,
        "name\tx\ty\twidth\theight\tscale\n\
         DP-1\t0\t0\t1920\t1080\t1\n\
         DP-2\t1920\t0\t2560\t1440\t2\n"
    );
    assert_eq!(r.calls, vec![BackendCall::ListOutputs]);
}

#[tokio::test]
async fn outputs_with_no_outputs_prints_just_header_and_exits_0() {
    // The wlroots backend returns Vec::new() when no monitors are
    // enumerated; libei/uinput always return Vec::new(). Either way
    // we exit 0 with a header-only table so scripts can `awk` over it.
    let r = common::run(&["outputs"]).await;
    assert!(r.exit.is_success());
    assert_eq!(r.stdout, "name\tx\ty\twidth\theight\tscale\n");
}

#[tokio::test]
async fn outputs_json_emits_valid_array() {
    let mock = MockBackend::new();
    mock.set_outputs(vec![OutputInfo {
        name: "DP-1".into(),
        x: 100,
        y: 200,
        width: 800,
        height: 600,
        scale: 1,
    }]);
    let r = common::run_with(&["outputs", "--json"], mock, Environment::default()).await;
    assert!(r.exit.is_success());
    let value: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    let arr = value.as_array().expect("outputs --json must emit an array");
    assert_eq!(arr.len(), 1);
    let row = &arr[0];
    assert_eq!(row["name"], serde_json::json!("DP-1"));
    assert_eq!(row["x"], serde_json::json!(100));
    assert_eq!(row["y"], serde_json::json!(200));
    assert_eq!(row["width"], serde_json::json!(800));
    assert_eq!(row["height"], serde_json::json!(600));
    assert_eq!(row["scale"], serde_json::json!(1));
}

#[tokio::test]
async fn getmouselocation_prints_xdotool_format_when_supported() {
    let mock = MockBackend::new();
    mock.set_pointer(Some((512, 384)));
    let r = common::run_with(&["getmouselocation"], mock, Environment::default()).await;
    assert!(r.exit.is_success());
    // xdotool's exact default format. wflows scripts depend on this
    // shape because xdotool consumers parse it directly.
    assert_eq!(r.stdout, "x:512 y:384\n");
    assert_eq!(r.calls, vec![BackendCall::PointerPosition]);
}

#[tokio::test]
async fn getmouselocation_exits_1_with_hint_when_backend_returns_none() {
    // libei/wlroots/uinput all return Ok(None) here. The CLI prints
    // a stderr hint pointing the user at kde/gnome backends or their
    // compositor's IPC, then exits 1.
    let r = common::run(&["getmouselocation"]).await;
    assert_eq!(r.exit.0, 1);
    assert!(r.error.is_none());
    assert_eq!(r.stdout, "");
    assert!(
        r.stderr.contains("pointer position is unreadable"),
        "stderr: {}",
        r.stderr
    );
}

#[tokio::test]
async fn getmouselocation_handles_negative_coordinates() {
    // Multi-monitor setups can put outputs at negative origins.
    // Sanity check the format prints correctly with a leading minus.
    let mock = MockBackend::new();
    mock.set_pointer(Some((-100, -50)));
    let r = common::run_with(&["getmouselocation"], mock, Environment::default()).await;
    assert!(r.exit.is_success());
    assert_eq!(r.stdout, "x:-100 y:-50\n");
}
