//! Evdev capture backend. Reads `/dev/input/event*` directly.
//!
//! Used as a fallback on compositors whose portal doesn't expose
//! RemoteDesktop (xdg-desktop-portal-hyprland, xdg-desktop-portal-wlr
//! today). The `input` group membership is the consent — no
//! per-session prompt.
//!
//! One tokio task per input device; each task pumps events through
//! `evdev_to_rec` and into the shared mpsc sender. A coordinator task
//! holds the JoinSet so dropping it on stop tears every device task
//! down at once.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tracing::warn;

use super::mapping::evdev_to_rec;
use super::types::RecEvent;
use crate::error::{Result, WdoError};

const NAME: &str = "recorder-evdev";

pub(super) async fn spawn(
    started_at: Instant,
    min_move_interval_ms: u64,
    tx: mpsc::Sender<RecEvent>,
    stop_rx: oneshot::Receiver<()>,
) -> Result<JoinHandle<()>> {
    // Enumerate at start (no hot-plug). Filter to devices that
    // actually emit useful event types — skip power buttons, lid
    // switches, gpio dummies.
    let mut devices: Vec<(PathBuf, evdev::Device)> = Vec::new();
    for (path, dev) in evdev::enumerate() {
        let supports_keys = dev.supported_keys().is_some();
        let supports_rel = dev.supported_relative_axes().is_some();
        if supports_keys || supports_rel {
            devices.push((path, dev));
        }
    }

    if devices.is_empty() {
        return Err(WdoError::Backend {
            backend: NAME,
            source: "no readable input devices at /dev/input/event*. \
                     Check that you're in the `input` group \
                     (`groups | grep input`); if not, run \
                     `sudo usermod -aG input $USER` and log out/back in."
                .into(),
        });
    }

    // Shared modifier bitmask across all devices, in the same xkb bit
    // positions keycode_to_chord expects.
    let mods = Arc::new(AtomicU32::new(0));
    // Global mouse-motion throttle clock. Without this, every mouse
    // tick (1000Hz on a gaming mouse) would saturate downstream
    // queues.
    let last_move_ms = Arc::new(AtomicU64::new(0));

    let mut joinset = JoinSet::new();
    for (path, dev) in devices {
        let mut stream = match dev.into_event_stream() {
            Ok(s) => s,
            Err(e) => {
                warn!(?path, ?e, "evdev: stream open failed; skipping device");
                continue;
            }
        };
        let tx_dev = tx.clone();
        let mods_dev = mods.clone();
        let last_move_dev = last_move_ms.clone();
        joinset.spawn(async move {
            let mut rel_x: i32 = 0;
            let mut rel_y: i32 = 0;
            loop {
                let ev = match stream.next_event().await {
                    Ok(e) => e,
                    Err(_) => break,
                };
                let t_ms = started_at.elapsed().as_millis() as u64;
                if let Some(rec) = evdev_to_rec(
                    &ev,
                    t_ms,
                    &mods_dev,
                    &mut rel_x,
                    &mut rel_y,
                    &last_move_dev,
                    min_move_interval_ms,
                ) {
                    if tx_dev.send(rec).await.is_err() {
                        // Consumer dropped the receiver.
                        break;
                    }
                }
            }
        });
    }

    // Coordinator: keeps the JoinSet alive and watches the stop
    // signal. Aborting this handle drops the JoinSet, which aborts
    // every device task.
    let coordinator = tokio::spawn(async move {
        tokio::pin!(stop_rx);
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                next = joinset.join_next() => {
                    if next.is_none() {
                        // All device tasks exited.
                        break;
                    }
                }
            }
        }
        // Dropping joinset here aborts any tasks still running.
    });
    Ok(coordinator)
}
