//! Input recording for Wayland Linux.
//!
//! Behind the `recorder` Cargo feature. Captures user input from one
//! of three sources — the XDG RemoteDesktop portal (libei in receiver
//! mode), `/dev/input/event*` via evdev, or a deterministic script for
//! tests — and exposes them as a `Stream` of [`RecEvent`]s.
//!
//! ## Quick start
//!
//! ```no_run
//! use futures_util::StreamExt;
//! use wdotool_core::recorder::{start, RecorderConfig};
//!
//! # async fn run() -> wdotool_core::Result<()> {
//! let mut session = start(RecorderConfig::default()).await?;
//! let mut events = session.events();
//! while let Some(ev) = events.next().await {
//!     println!("{:?}", ev);
//! }
//! # Ok(()) }
//! ```
//!
//! Or use [`RecorderSession::stop`] for a one-shot capture-and-collect
//! flow that returns the captured events as a `Vec`.
//!
//! ## What's recorded
//!
//! Pure input — `Key` chords, `Click`, `Move` (absolute on portal,
//! delta on evdev), `Scroll`, and auto-inserted `Gap` events. No
//! window-focus events or lifecycle frames; consumers that need those
//! merge their own streams in. See `docs/design/recorder-migration.md`
//! for the rationale.
//!
//! ## Backend cascade
//!
//! [`BackendChoice::Auto`] tries portal first, then evdev. It
//! deliberately does NOT fall through to simulated — fake events that
//! look real are worse than a clear error.

mod evdev;
mod mapping;
mod portal;
mod simulated;
mod types;

pub use types::{BackendChoice, RecEvent, RecorderConfig};

use std::time::Instant;

use futures_util::Stream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;

use crate::error::{Result, WdoError};

/// Capacity of the event channel between the backend pump and the
/// public stream. Sized to absorb a brief consumer stall (Qt repaint,
/// JSON serializer flush) without dropping events. Backend pumps
/// `tx.send().await` so backpressure works correctly when the consumer
/// genuinely can't keep up — they wait on the channel's receiver.
const CHANNEL_CAPACITY: usize = 256;

/// Start a recording session.
///
/// Returns a [`RecorderSession`] holding the capture pump's lifecycle
/// (the stop signal + background task / thread handles). Read events
/// off it with [`RecorderSession::events`], or do a one-shot
/// capture-and-collect with [`RecorderSession::stop`].
pub async fn start(config: RecorderConfig) -> Result<RecorderSession> {
    let started_at = Instant::now();
    let (tx, rx) = mpsc::channel::<RecEvent>(CHANNEL_CAPACITY);
    let (stop_tx, stop_rx) = oneshot::channel::<()>();

    let (resolved, task, thread) = match config.backend {
        BackendChoice::Simulated => {
            let task = simulated::spawn(started_at, tx, stop_rx);
            (BackendChoice::Simulated, Some(task), None)
        }
        BackendChoice::Portal => {
            let thread = portal::spawn(started_at, config.move_threshold_px, tx, stop_rx).await?;
            (BackendChoice::Portal, None, Some(thread))
        }
        BackendChoice::Evdev => {
            let task = evdev::spawn(started_at, config.min_move_interval_ms, tx, stop_rx).await?;
            (BackendChoice::Evdev, Some(task), None)
        }
        BackendChoice::Auto => {
            // Try portal first, then evdev. Bail out with both errors
            // if neither works — fake events that look real are worse
            // than a clear "Record can't start" message.
            let (portal_tx, evdev_tx) = (tx.clone(), tx);
            // Reuse the same stop channel: portal uses stop_rx
            // directly; if it fails, build a fresh one for evdev.
            let portal_err =
                match portal::spawn(started_at, config.move_threshold_px, portal_tx, stop_rx).await
                {
                    Ok(thread) => {
                        return Ok(RecorderSession {
                            rx: Some(rx),
                            stop_tx: Some(stop_tx),
                            source: BackendChoice::Portal,
                            task: None,
                            thread: Some(thread),
                            started_at,
                        })
                    }
                    Err(e) => e,
                };
            // Portal failed; rebuild the stop channel and try evdev.
            // The original stop_rx was consumed by the failed portal
            // attempt's spawn (or rather, would have been; in practice
            // portal::spawn doesn't consume it on the error paths
            // before connect_to_eis succeeds, but we re-create it to
            // be safe across all error timings).
            let (new_stop_tx, new_stop_rx) = oneshot::channel::<()>();
            let evdev_err = match evdev::spawn(
                started_at,
                config.min_move_interval_ms,
                evdev_tx,
                new_stop_rx,
            )
            .await
            {
                Ok(task) => {
                    return Ok(RecorderSession {
                        rx: Some(rx),
                        stop_tx: Some(new_stop_tx),
                        source: BackendChoice::Evdev,
                        task: Some(task),
                        thread: None,
                        started_at,
                    })
                }
                Err(e) => e,
            };
            return Err(WdoError::Backend {
                backend: "recorder",
                source: format!(
                    "no capture source available.\n\nPortal: {portal_err}\n\nEvdev: {evdev_err}\n\n\
                     Pick one of these to fix it:\n  \
                     • On Plasma 6 or GNOME 46+, install/restart xdg-desktop-portal.\n  \
                     • On Hyprland or Sway, add yourself to the `input` group: \
                     `sudo usermod -aG input $USER`, log out and back in."
                )
                .into(),
            });
        }
    };

    Ok(RecorderSession {
        rx: Some(rx),
        stop_tx: Some(stop_tx),
        source: resolved,
        task,
        thread,
        started_at,
    })
}

/// A live recording session.
///
/// Drop or call [`RecorderSession::stop`] to end the recording.
/// Reading the stream via [`RecorderSession::events`] takes ownership
/// of the receiver; after that, [`RecorderSession::stop`] still works
/// for cleanup but returns an empty `Vec` (the events flowed out
/// through the stream the consumer was holding).
pub struct RecorderSession {
    rx: Option<mpsc::Receiver<RecEvent>>,
    stop_tx: Option<oneshot::Sender<()>>,
    source: BackendChoice,
    task: Option<JoinHandle<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    started_at: Instant,
}

impl RecorderSession {
    /// Which backend was actually used. `BackendChoice::Auto` resolves
    /// to one of `Portal`, `Evdev`, or `Simulated` once `start()`
    /// returns.
    pub fn source(&self) -> BackendChoice {
        self.source
    }

    /// Wall-clock instant the session started. Useful for stamping
    /// out-of-band events the consumer wants to merge into the same
    /// timeline (e.g., wflow's window-focus events).
    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    /// Take the live event stream. Can only be called once; subsequent
    /// calls panic. The stream ends when the backend disconnects, when
    /// [`RecorderSession::stop`] is called, or when this session is
    /// dropped.
    pub fn events(&mut self) -> impl Stream<Item = RecEvent> + Send + 'static {
        let rx = self
            .rx
            .take()
            .expect("RecorderSession::events() can only be called once");
        ReceiverStream::new(rx)
    }

    /// Stop the recording and return everything captured so far.
    ///
    /// If the consumer already called [`events`](Self::events) and
    /// drained the stream, this returns an empty `Vec` — the events
    /// went out through that stream, not through here.
    pub async fn stop(mut self) -> Result<Vec<RecEvent>> {
        // Signal the pump to stop. Errors here only happen if the
        // pump already exited, which is fine.
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        // Drain whatever's still buffered in the channel before the
        // pump finishes its teardown.
        let collected = if let Some(mut rx) = self.rx.take() {
            let mut v = Vec::new();
            while let Some(ev) = rx.recv().await {
                v.push(ev);
            }
            v
        } else {
            Vec::new()
        };
        // Best-effort cleanup. The task's drop will abort if it's
        // still running; threads we move into a side task to avoid
        // blocking the caller's runtime on join.
        if let Some(t) = self.task.take() {
            t.abort();
            // Awaiting the JoinHandle yields the abort error; we
            // intentionally swallow it.
            let _ = t.await;
        }
        if let Some(th) = self.thread.take() {
            tokio::task::spawn_blocking(move || {
                let _ = th.join();
            });
        }
        Ok(collected)
    }
}

impl Drop for RecorderSession {
    fn drop(&mut self) {
        // Best-effort: signal stop and abort the task. Threads can't
        // be joined from drop without blocking, so we just leave them
        // — the dropped Sender will close the channel, the pump will
        // see the channel closed and exit, the OS will reap the
        // thread. Worst case is a brief overlap.
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}
