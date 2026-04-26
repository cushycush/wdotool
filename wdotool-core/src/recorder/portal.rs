//! Portal-based capture backend.
//!
//! Opens an `org.freedesktop.portal.RemoteDesktop` session via ashpd,
//! grabs the EIS file descriptor, and runs the libei stream in
//! receiver mode on a dedicated OS thread (the EI stream isn't
//! `Send`). Events flow into a tokio mpsc channel that the public
//! `RecorderSession` exposes as a Stream.
//!
//! Requires the user's compositor to expose RemoteDesktop. Plasma 6
//! and GNOME 46+ both do; xdg-desktop-portal-hyprland and
//! xdg-desktop-portal-wlr currently do not.

use std::os::unix::net::UnixStream;
use std::time::Instant;

use ashpd::desktop::remote_desktop::{DeviceType, RemoteDesktop, SelectDevicesOptions};
use futures_util::StreamExt;
use reis::ei;
use reis::event::{DeviceCapability, EiEvent};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

use super::mapping::{eis_to_rec, EisState};
use super::types::RecEvent;
use crate::error::{Result, WdoError};

const NAME: &str = "recorder-portal";

/// Open the portal session, hand off to a pump thread, and return the
/// thread handle so the caller can join it on stop.
pub(super) async fn spawn(
    started_at: Instant,
    move_threshold_px: i32,
    tx: mpsc::Sender<RecEvent>,
    stop_rx: oneshot::Receiver<()>,
) -> Result<std::thread::JoinHandle<()>> {
    let rd = RemoteDesktop::new().await.map_err(|e| WdoError::Backend {
        backend: NAME,
        source: format!("open RemoteDesktop portal proxy: {e}").into(),
    })?;
    let session = rd
        .create_session(Default::default())
        .await
        .map_err(|e| WdoError::Backend {
            backend: NAME,
            source: format!("create RemoteDesktop session: {e}").into(),
        })?;
    rd.select_devices(
        &session,
        SelectDevicesOptions::default().set_devices(DeviceType::Keyboard | DeviceType::Pointer),
    )
    .await
    .map_err(|e| WdoError::Backend {
        backend: NAME,
        source: format!("select keyboard+pointer devices: {e}").into(),
    })?;
    rd.start(&session, None, Default::default())
        .await
        .map_err(|e| WdoError::Backend {
            backend: NAME,
            source: format!("start RemoteDesktop session: {e}").into(),
        })?
        .response()
        .map_err(|e| WdoError::Backend {
            backend: NAME,
            source: format!("portal dialog denied or failed: {e}").into(),
        })?;

    let fd = rd
        .connect_to_eis(&session, Default::default())
        .await
        .map_err(|e| WdoError::Backend {
            backend: NAME,
            source: format!("ConnectToEIS (needs RemoteDesktop v2): {e}").into(),
        })?;
    let stream = UnixStream::from(fd);
    let context = ei::Context::new(stream).map_err(|e| WdoError::Backend {
        backend: NAME,
        source: format!("wrap EIS fd in ei::Context: {e}").into(),
    })?;

    // The reis event stream isn't Send. Park it on a dedicated OS
    // thread that owns its own current-thread tokio runtime; the
    // parent runtime stays multi-threaded for the rest of the app.
    let thread = std::thread::Builder::new()
        .name("wdotool-rec-portal".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    warn!(error = %e, "recorder: failed to build EIS runtime");
                    return;
                }
            };
            rt.block_on(pump(
                rd,
                session,
                context,
                stop_rx,
                tx,
                started_at,
                move_threshold_px,
            ));
        })
        .map_err(|e| WdoError::Backend {
            backend: NAME,
            source: format!("spawn EIS pump thread: {e}").into(),
        })?;
    Ok(thread)
}

async fn pump(
    _rd: RemoteDesktop,
    _session: ashpd::desktop::Session<RemoteDesktop>,
    context: ei::Context,
    stop_rx: oneshot::Receiver<()>,
    tx: mpsc::Sender<RecEvent>,
    started_at: Instant,
    move_threshold_px: i32,
) {
    let handshake = context
        .handshake_tokio("wdotool-recorder", ei::handshake::ContextType::Receiver)
        .await;
    let (_connection, mut stream) = match handshake {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %format!("{e:#}"), "EIS handshake failed");
            return;
        }
    };

    let mut state = EisState::default();

    tokio::pin!(stop_rx);
    loop {
        tokio::select! {
            _ = &mut stop_rx => {
                debug!("recorder: stop requested");
                break;
            }
            maybe_event = stream.next() => {
                let Some(event) = maybe_event else { break };
                let event = match event {
                    Ok(e) => e,
                    Err(e) => {
                        warn!(error = %format!("{e:?}"), "EIS stream error");
                        break;
                    }
                };
                if let EiEvent::SeatAdded(evt) = &event {
                    evt.seat.bind_capabilities(
                        DeviceCapability::Pointer
                            | DeviceCapability::PointerAbsolute
                            | DeviceCapability::Keyboard
                            | DeviceCapability::Button
                            | DeviceCapability::Scroll,
                    );
                    let _ = context.flush();
                }
                let t_ms = started_at.elapsed().as_millis() as u64;
                if let Some(rec) = eis_to_rec(&event, t_ms, &mut state, move_threshold_px) {
                    if tx.send(rec).await.is_err() {
                        // Consumer dropped the receiver.
                        break;
                    }
                }
            }
        }
    }
}
