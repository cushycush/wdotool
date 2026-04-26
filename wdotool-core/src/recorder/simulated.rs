//! Deterministic event source. For tests, CI, and local UI iteration
//! when a real portal session isn't reachable.
//!
//! Plays a hardcoded "open browser, type a query, click around" script
//! at fixed timings. The script is short on purpose — a recorder unit
//! test wants known-good events arriving in known order, not a full
//! workflow.

use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::types::RecEvent;

/// Spawn the simulated pump. Returns the JoinHandle so the parent
/// can abort on stop.
pub(super) fn spawn(
    started_at: Instant,
    tx: mpsc::Sender<RecEvent>,
    mut stop_rx: oneshot::Receiver<()>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let script: Vec<(u64, RecEvent)> = vec![
            (
                60,
                RecEvent::Key {
                    t_ms: 60,
                    chord: "super+space".into(),
                },
            ),
            (
                1280,
                RecEvent::Key {
                    t_ms: 1280,
                    chord: "Return".into(),
                },
            ),
            (
                2700,
                RecEvent::Key {
                    t_ms: 2700,
                    chord: "ctrl+l".into(),
                },
            ),
            (
                4100,
                RecEvent::Key {
                    t_ms: 4100,
                    chord: "Return".into(),
                },
            ),
            (
                5900,
                RecEvent::MoveAbs {
                    t_ms: 5900,
                    x: 720,
                    y: 480,
                },
            ),
            (
                6050,
                RecEvent::Click {
                    t_ms: 6050,
                    button: 1,
                },
            ),
            (
                6900,
                RecEvent::Scroll {
                    t_ms: 6900,
                    dx: 0,
                    dy: 3,
                },
            ),
        ];

        for (at, ev) in script {
            let elapsed = started_at.elapsed().as_millis() as u64;
            let gap = at.saturating_sub(elapsed);
            // Cap individual sleeps so stop() returns quickly even
            // if the script's gap to the next event is large.
            let sleep = Duration::from_millis(gap.min(1800));
            tokio::select! {
                _ = tokio::time::sleep(sleep) => {}
                _ = &mut stop_rx => return,
            }
            if tx.send(ev).await.is_err() {
                // Consumer dropped the receiver; pump is no longer
                // wanted.
                return;
            }
        }
        // Pump finished naturally (script exhausted). Drop tx so the
        // public stream ends cleanly.
    })
}
