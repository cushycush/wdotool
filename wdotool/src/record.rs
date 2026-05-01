//! `wdotool record` — capture user input until Ctrl-C (or
//! `--max-duration` elapses) and write the events as JSON.
//!
//! Built on `wdotool_core::recorder`. The handler picks pattern B
//! from the design doc: start the session, race Ctrl-C against an
//! optional max-duration timer, then call `stop()` to drain captured
//! events into a `Vec` for serialization. We don't drive the live
//! `events()` Stream because the user-facing surface here is "give me
//! a JSON file at the end," not a streaming API.

use std::time::Duration;

use wdotool_core::recorder::{self, BackendChoice, RecorderConfig};
use wdotool_core::{Result, WdoError};

/// Entry point for the `Record` subcommand. Lives behind the
/// `recorder` feature gate; the call site in `main.rs` is also gated
/// so non-recorder builds compile cleanly.
pub async fn run(
    output: Option<String>,
    max_duration_secs: Option<u64>,
    backend_arg: String,
) -> anyhow::Result<()> {
    let backend = parse_backend(&backend_arg)?;
    let config = RecorderConfig {
        backend,
        ..RecorderConfig::default()
    };

    let session = recorder::start(config).await?;
    eprintln!(
        "wdotool record: capturing via {:?} backend (Ctrl-C to stop{}{})",
        session.source(),
        max_duration_secs
            .map(|s| format!(" or after {s}s"))
            .unwrap_or_default(),
        match output.as_deref() {
            Some(path) if path != "-" => format!(", writing to {path}"),
            _ => " (writing to stdout)".to_string(),
        }
    );

    // Wait for whichever stop signal fires first. ctrl_c() never
    // resolves to a value once installed (it's a signal handler), so
    // we put the timer in the same select with a future-pending
    // fallback when no max_duration was passed.
    let stop_reason = tokio::select! {
        ctrl_c = tokio::signal::ctrl_c() => {
            ctrl_c.map_err(|e| WdoError::Backend {
                backend: "recorder",
                source: format!("ctrl_c handler install failed: {e}").into(),
            })?;
            "ctrl-c"
        }
        _ = async {
            match max_duration_secs {
                Some(secs) => tokio::time::sleep(Duration::from_secs(secs)).await,
                None => std::future::pending::<()>().await,
            }
        } => "max-duration",
    };

    let events = session.stop().await?;
    eprintln!(
        "wdotool record: stopped via {stop_reason}, captured {} event{}",
        events.len(),
        if events.len() == 1 { "" } else { "s" }
    );

    // Pretty-printed JSON so a human can eyeball the captured trace.
    // serde_json reading the same file back round-trips through
    // RecEvent's #[serde(tag = "kind")] discriminant.
    let json = serde_json::to_string_pretty(&events)
        .map_err(|e| WdoError::InvalidArg(format!("recorder serialization: {e}")))?;

    match output.as_deref() {
        None | Some("-") => {
            println!("{json}");
        }
        Some(path) => {
            std::fs::write(path, format!("{json}\n"))
                .map_err(|e| WdoError::InvalidArg(format!("failed to write {path}: {e}")))?;
            eprintln!("wdotool record: wrote {path}");
        }
    }

    Ok(())
}

/// Map the user's `--backend` string onto `BackendChoice`. Defaults
/// pass through to `Auto` via clap's `default_value = "auto"`.
fn parse_backend(s: &str) -> Result<BackendChoice> {
    match s {
        "auto" => Ok(BackendChoice::Auto),
        "portal" => Ok(BackendChoice::Portal),
        "evdev" => Ok(BackendChoice::Evdev),
        "simulated" => Ok(BackendChoice::Simulated),
        other => Err(WdoError::InvalidArg(format!(
            "unknown record backend {other:?} (expected: auto, portal, evdev, simulated)"
        ))),
    }
}
