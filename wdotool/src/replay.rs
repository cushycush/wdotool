//! `wdotool replay <file>` — read a captured `RecEvent` trace and
//! dispatch each event through the active backend, reproducing the
//! original timing.
//!
//! Lives behind the same `recorder` Cargo feature as `wdotool record`
//! because the input format (`wdotool_core::recorder::RecEvent`) is
//! defined inside that module. Replay itself doesn't pull in any of
//! the capture machinery (portal, evdev, simulated); it only needs
//! the type to deserialize.
//!
//! Timing handling: each `RecEvent` carries an absolute `t_ms` since
//! the recording session started, and the recorder auto-inserts
//! `Gap { ms }` events when nothing happened for a while. We sleep
//! on each `Gap` for `ms / speed` milliseconds and dispatch
//! everything else immediately. This matches the design intent of
//! Gaps ("Lets replay reproduce timing without the consumer having
//! to track elapsed time between events") and avoids the complexity
//! of computing per-event deltas.

use std::time::Duration;

use wdotool_core::recorder::RecEvent;
use wdotool_core::{Backend, KeyDirection, MouseButton, Result, WdoError};

use crate::run_key;

/// Read a trace from `file` (or stdin if `file == "-"`), parse it as
/// a JSON array of `RecEvent`, then dispatch each event through the
/// backend. `speed` scales Gap durations: 1.0 = real-time, 2.0 =
/// twice as fast, 0.5 = half speed.
pub async fn run(backend: &dyn Backend, file: &str, speed: f64) -> Result<()> {
    if speed <= 0.0 {
        return Err(WdoError::InvalidArg(format!(
            "--speed must be positive, got {speed}"
        )));
    }
    let trace = read_trace(file)?;
    let events: Vec<RecEvent> = serde_json::from_str(&trace).map_err(|e| {
        WdoError::InvalidArg(format!("failed to parse trace as RecEvent JSON: {e}"))
    })?;

    for event in events {
        dispatch_event(backend, &event, speed).await?;
    }
    Ok(())
}

fn read_trace(file: &str) -> Result<String> {
    if file == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| WdoError::InvalidArg(format!("failed to read stdin: {e}")))?;
        Ok(buf)
    } else {
        std::fs::read_to_string(file)
            .map_err(|e| WdoError::InvalidArg(format!("failed to read {file}: {e}")))
    }
}

async fn dispatch_event(backend: &dyn Backend, event: &RecEvent, speed: f64) -> Result<()> {
    match event {
        RecEvent::Gap { ms, .. } => {
            // Scale the Gap by the user's --speed. Round to nearest;
            // sub-millisecond precision isn't meaningful for input.
            let scaled = (*ms as f64 / speed).round() as u64;
            if scaled > 0 {
                tokio::time::sleep(Duration::from_millis(scaled)).await;
            }
        }
        RecEvent::Key { chord, .. } => {
            // Same path the `key` subcommand uses, so modifier
            // ordering and the run_key helper's xdotool-compatible
            // press / release sequence are reused here verbatim.
            run_key(backend, chord, KeyDirection::PressRelease).await?;
        }
        RecEvent::Click { button, .. } => {
            // RecEvent::Click is a press+release pair on replay (the
            // type's docs explicitly say release is implicit).
            backend
                .mouse_button(
                    MouseButton::from_index(*button as u32),
                    KeyDirection::PressRelease,
                )
                .await?;
        }
        RecEvent::MoveAbs { x, y, .. } => {
            backend.mouse_move(*x, *y, true).await?;
        }
        RecEvent::MoveDelta { dx, dy, .. } => {
            backend.mouse_move(*dx, *dy, false).await?;
        }
        RecEvent::Scroll { dx, dy, .. } => {
            // RecEvent stores deltas as i32 (because evdev reports
            // them that way); the Backend trait scroll takes f64
            // because the wlroots/libei paths use floating-point
            // axes. The cast is lossless for any realistic scroll.
            backend.scroll(*dx as f64, *dy as f64).await?;
        }
    }
    Ok(())
}
