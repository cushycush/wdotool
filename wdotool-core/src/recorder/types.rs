//! Public types for the recorder module.
//!
//! `RecEvent` is the substrate-level input event. It's deliberately
//! pure input — no focus / window / lifecycle events live here, since
//! those are compositor-specific (Hyprland's `.socket2.sock`, KWin
//! scripts, GNOME extensions) and belong in the consumer that knows
//! which compositor it's on. wflow merges its own focus stream with
//! this one before pushing through its UI bridge; other consumers
//! that just want input (a CLI recorder, a test harness, a
//! workflow-replayer test) get exactly what they need.

use serde::{Deserialize, Serialize};

/// A single captured input event.
///
/// `t_ms` is milliseconds since the recording session started. Use it
/// to reproduce timing on replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecEvent {
    /// A chord was pressed (coalesced from modifier + key state).
    /// `chord` is in wdotool's keysym format, e.g. `ctrl+l`,
    /// `super+Return`.
    Key { t_ms: u64, chord: String },

    /// A mouse button was pressed (release is implicit on replay).
    /// `button` follows xdotool's indexing: 1=left, 2=middle,
    /// 3=right, 8=back, 9=forward.
    Click { t_ms: u64, button: u8 },

    /// Pointer motion to a known absolute screen coordinate. Emitted
    /// by the libei portal path, where the EIS server hands us
    /// screen-space positions.
    MoveAbs { t_ms: u64, x: i32, y: i32 },

    /// Pointer motion as a delta from the previous position. Emitted
    /// by the evdev path, where `REL_X`/`REL_Y` events don't carry an
    /// absolute position and there's no portable way to read the
    /// pointer's current location without a portal.
    MoveDelta { t_ms: u64, dx: i32, dy: i32 },

    /// Scroll. Positive `dy` scrolls down; positive `dx` scrolls right.
    Scroll { t_ms: u64, dx: i32, dy: i32 },

    /// Auto-inserted when nothing else happened for a while. Lets
    /// replay reproduce timing without the consumer having to track
    /// elapsed time between events.
    Gap { t_ms: u64, ms: u64 },
}

impl RecEvent {
    /// Return the timestamp this event happened at, in ms since the
    /// session started. Useful when sorting / merging streams.
    pub fn t_ms(&self) -> u64 {
        match self {
            RecEvent::Key { t_ms, .. }
            | RecEvent::Click { t_ms, .. }
            | RecEvent::MoveAbs { t_ms, .. }
            | RecEvent::MoveDelta { t_ms, .. }
            | RecEvent::Scroll { t_ms, .. }
            | RecEvent::Gap { t_ms, .. } => *t_ms,
        }
    }
}

/// Configuration for a recording session.
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Minimum interval between `Move` emissions in milliseconds.
    /// Below this, motion accumulates in an internal buffer and
    /// flushes when the interval elapses or a non-motion event
    /// arrives. Defaults to 1000ms; tune via `WDOTOOL_REC_MOVE_INTERVAL_MS`
    /// or by setting this field directly.
    pub min_move_interval_ms: u64,

    /// Pointer-motion threshold in pixels. Below this, accumulators
    /// build up and don't emit a `Move`. Default 4.
    pub move_threshold_px: i32,

    /// Backend choice. `Auto` cascades portal -> evdev (no simulated
    /// in the cascade — fake events that look real are worse than a
    /// clear error).
    pub backend: BackendChoice,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        let env_interval = std::env::var("WDOTOOL_REC_MOVE_INTERVAL_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1000);
        Self {
            min_move_interval_ms: env_interval,
            move_threshold_px: 4,
            backend: BackendChoice::Auto,
        }
    }
}

/// Which capture backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    /// Try portal (libei receiver) first, then evdev. Never falls
    /// through to simulated.
    Auto,
    /// XDG RemoteDesktop portal + libei in receiver mode. Requires
    /// the portal to expose `org.freedesktop.portal.RemoteDesktop`
    /// (Plasma 6, GNOME 46+).
    Portal,
    /// Read `/dev/input/event*` directly. Requires the user to be in
    /// the `input` group.
    Evdev,
    /// Deterministic test script. For tests and CI; no real input
    /// is captured.
    Simulated,
}
