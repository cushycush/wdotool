//! Compositor event subscription.
//!
//! Behind the `events` Cargo feature. Connects to Drift's IPC socket
//! (Hyprland-compatible `.socket2.sock`), decodes the line-oriented
//! `name>>data` wire format into a typed [`Event`] enum, and exposes
//! the stream as a `futures::Stream<Item = Result<Event, EventError>>`.
//!
//! Drift is the only event source today. wlr-protocols sessions
//! (Sway / Hyprland on a non-Drift config) and uinput / portal-only
//! sessions don't have an equivalent push channel. wflow's trigger
//! cards ("when Firefox opens, run workflow X") are the primary
//! consumer; future Drift Settings panels for live status drink from
//! the same fountain.
//!
//! ## Quick start
//!
//! ```no_run
//! use futures_util::StreamExt;
//! use wdotool_core::events::{EventStream, Event};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut stream = EventStream::connect().await?;
//! while let Some(event) = stream.next().await {
//!     match event? {
//!         Event::WindowOpened { app_id, title, .. } => {
//!             println!("opened {app_id}: {title}");
//!         }
//!         _ => {}
//!     }
//! }
//! # Ok(()) }
//! ```
//!
//! ## Coalescing of v1 / v2 lines
//!
//! Drift emits many transitions twice: a legacy line and a `v2` line
//! with extra fields. The parser merges these into a single typed
//! event so consumers don't see duplicates. See [`parser`] for the
//! pairing rules.

mod parser;
mod socket;
mod types;

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

pub use types::{ActiveWindow, Event, MonitorFocusWorkspace};

use parser::EventParser;

const CHANNEL_CAPACITY: usize = 256;

/// Error returned by the events module.
#[derive(Debug, thiserror::Error)]
pub enum EventError {
    /// A required environment variable wasn't set.
    #[error("missing environment variable: {0}")]
    MissingEnv(&'static str),

    /// The IPC socket isn't where we expected it.
    #[error("Drift IPC socket not found at {path}: {source}")]
    SocketNotFound {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// I/O on the socket failed after the initial connect.
    #[error("Drift IPC read failed: {0}")]
    Io(#[from] std::io::Error),
}

/// A live subscription to Drift's IPC event stream.
///
/// Created with [`EventStream::connect`] (or
/// [`EventStream::connect_at`] when the caller already knows the
/// socket path). Drops the connection on `Drop`.
pub struct EventStream {
    inner: ReceiverStream<Result<Event, EventError>>,
}

impl EventStream {
    /// Find Drift's IPC socket via `$XDG_RUNTIME_DIR` +
    /// `$HYPRLAND_INSTANCE_SIGNATURE` (or `$DRIFT_INSTANCE_SIGNATURE`
    /// if Drift sets that in the future) and connect.
    pub async fn connect() -> Result<Self, EventError> {
        let path = socket::discover_socket_path()?;
        Self::connect_at(&path).await
    }

    /// Connect to a specific socket path. Useful for tests and for
    /// callers that already resolved the path through their own
    /// mechanism.
    pub async fn connect_at(path: &Path) -> Result<Self, EventError> {
        let stream =
            UnixStream::connect(path)
                .await
                .map_err(|source| EventError::SocketNotFound {
                    path: path.to_path_buf(),
                    source,
                })?;
        let (tx, rx) = mpsc::channel::<Result<Event, EventError>>(CHANNEL_CAPACITY);
        let reader = BufReader::new(stream).lines();
        tokio::spawn(pump(reader, tx));
        Ok(Self {
            inner: ReceiverStream::new(rx),
        })
    }

    /// Build an `EventStream` from a pre-existing async line source.
    /// Used by tests to drive the parser end-to-end with synthetic
    /// input. The public API stays focused on the socket path so
    /// consumers can't accidentally depend on the line-reader internals.
    #[cfg(test)]
    pub(crate) fn from_lines<I>(lines: I) -> Self
    where
        I: IntoIterator<Item = String> + Send + 'static,
        I::IntoIter: Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<Result<Event, EventError>>(CHANNEL_CAPACITY);
        tokio::spawn(async move {
            let mut parser = EventParser::new();
            for line in lines {
                for event in parser.feed(&line) {
                    if tx.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
            }
        });
        Self {
            inner: ReceiverStream::new(rx),
        }
    }
}

impl Stream for EventStream {
    type Item = Result<Event, EventError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

async fn pump(
    mut reader: Lines<BufReader<UnixStream>>,
    tx: mpsc::Sender<Result<Event, EventError>>,
) {
    let mut parser = EventParser::new();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                for event in parser.feed(&line) {
                    if tx.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
            }
            Ok(None) => return,
            Err(e) => {
                let _ = tx.send(Err(EventError::Io(e))).await;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    async fn collect(stream: EventStream) -> Vec<Event> {
        stream
            .filter_map(|r| async { r.ok() })
            .collect::<Vec<_>>()
            .await
    }

    #[tokio::test]
    async fn parses_window_opened() {
        let stream =
            EventStream::from_lines(lines("openwindow>>5589abcd1234,1,firefox,Mozilla Firefox"));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![Event::WindowOpened {
                address: "5589abcd1234".into(),
                workspace: "1".into(),
                app_id: "firefox".into(),
                title: "Mozilla Firefox".into(),
            }]
        );
    }

    #[tokio::test]
    async fn window_opened_title_with_commas_preserved() {
        let stream = EventStream::from_lines(lines(
            "openwindow>>5589abcd1234,1,firefox,Hello, world, again",
        ));
        let events = collect(stream).await;
        let Event::WindowOpened { title, .. } = &events[0] else {
            panic!("expected WindowOpened, got {:?}", events[0]);
        };
        assert_eq!(title, "Hello, world, again");
    }

    #[tokio::test]
    async fn parses_window_closed() {
        let stream = EventStream::from_lines(lines("closewindow>>5589abcd1234"));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![Event::WindowClosed {
                address: "5589abcd1234".into(),
            }]
        );
    }

    #[tokio::test]
    async fn coalesces_active_window_pair() {
        let stream = EventStream::from_lines(lines(
            "activewindow>>firefox,Mozilla Firefox\n\
             activewindowv2>>5589abcd1234",
        ));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![Event::ActiveWindowChanged(Some(ActiveWindow {
                address: "5589abcd1234".into(),
                app_id: Some("firefox".into()),
                title: Some("Mozilla Firefox".into()),
            }))]
        );
    }

    #[tokio::test]
    async fn coalesces_active_window_pair_v2_first() {
        let stream = EventStream::from_lines(lines(
            "activewindowv2>>5589abcd1234\n\
             activewindow>>firefox,Mozilla Firefox",
        ));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![Event::ActiveWindowChanged(Some(ActiveWindow {
                address: "5589abcd1234".into(),
                app_id: Some("firefox".into()),
                title: Some("Mozilla Firefox".into()),
            }))]
        );
    }

    #[tokio::test]
    async fn coalesces_active_window_empty_pair() {
        let stream = EventStream::from_lines(lines(
            "activewindow>>,\n\
             activewindowv2>>",
        ));
        let events = collect(stream).await;
        assert_eq!(events, vec![Event::ActiveWindowChanged(None)]);
    }

    #[tokio::test]
    async fn parses_workspace_change() {
        let stream = EventStream::from_lines(lines(
            "workspace>>2\n\
             workspacev2>>2,2",
        ));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![Event::WorkspaceChanged {
                id: Some(2),
                name: "2".into(),
            }]
        );
    }

    #[tokio::test]
    async fn parses_window_title_change() {
        let stream = EventStream::from_lines(lines(
            "windowtitle>>5589abcd1234\n\
             windowtitlev2>>5589abcd1234,New Title",
        ));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![Event::WindowTitleChanged {
                address: "5589abcd1234".into(),
                title: "New Title".into(),
            }]
        );
    }

    #[tokio::test]
    async fn parses_fullscreen_toggle() {
        let stream = EventStream::from_lines(lines(
            "fullscreen>>1\n\
             fullscreen>>0",
        ));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![
                Event::FullscreenChanged { fullscreen: true },
                Event::FullscreenChanged { fullscreen: false },
            ]
        );
    }

    #[tokio::test]
    async fn parses_minimized() {
        let stream = EventStream::from_lines(lines(
            "minimized>>5589abcd1234,1\n\
             minimized>>5589abcd1234,0",
        ));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![
                Event::WindowMinimized {
                    address: "5589abcd1234".into(),
                    minimized: true,
                },
                Event::WindowMinimized {
                    address: "5589abcd1234".into(),
                    minimized: false,
                },
            ]
        );
    }

    #[tokio::test]
    async fn parses_urgent() {
        let stream = EventStream::from_lines(lines("urgent>>5589abcd1234"));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![Event::WindowUrgent {
                address: "5589abcd1234".into(),
            }]
        );
    }

    #[tokio::test]
    async fn parses_focused_monitor_change() {
        let stream = EventStream::from_lines(lines(
            "focusedmon>>DP-1,1\n\
             focusedmonv2>>DP-1,1",
        ));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![Event::MonitorFocusChanged {
                monitor: "DP-1".into(),
                workspace: MonitorFocusWorkspace {
                    id: Some(1),
                    name: "1".into(),
                },
            }]
        );
    }

    #[tokio::test]
    async fn parses_workspace_create_destroy() {
        let stream = EventStream::from_lines(lines(
            "createworkspace>>special:scratch\n\
             createworkspacev2>>-99,special:scratch\n\
             destroyworkspace>>special:scratch\n\
             destroyworkspacev2>>-99,special:scratch",
        ));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![
                Event::WorkspaceCreated {
                    id: Some(-99),
                    name: "special:scratch".into(),
                },
                Event::WorkspaceDestroyed {
                    id: Some(-99),
                    name: "special:scratch".into(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn parses_monitor_add_remove() {
        let stream = EventStream::from_lines(lines(
            "monitoradded>>DP-1\n\
             monitoraddedv2>>0,DP-1,Dell P2415Q\n\
             monitorremoved>>DP-1\n\
             monitorremovedv2>>0,DP-1,Dell P2415Q",
        ));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![
                Event::MonitorAdded {
                    id: Some(0),
                    name: "DP-1".into(),
                    description: Some("Dell P2415Q".into()),
                },
                Event::MonitorRemoved {
                    id: Some(0),
                    name: "DP-1".into(),
                    description: Some("Dell P2415Q".into()),
                },
            ]
        );
    }

    #[tokio::test]
    async fn unknown_events_are_skipped() {
        let stream = EventStream::from_lines(lines(
            "submap>>resize\n\
             screencast>>1,0\n\
             closewindow>>5589abcd1234",
        ));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![Event::WindowClosed {
                address: "5589abcd1234".into(),
            }]
        );
    }

    #[tokio::test]
    async fn malformed_lines_are_skipped() {
        let stream = EventStream::from_lines(lines(
            "not an event\n\
             closewindow>>5589abcd1234",
        ));
        let events = collect(stream).await;
        assert_eq!(
            events,
            vec![Event::WindowClosed {
                address: "5589abcd1234".into(),
            }]
        );
    }

    #[tokio::test]
    async fn end_to_end_via_unix_socket() {
        use std::io::Write;
        use std::os::unix::net::UnixListener as StdUnixListener;
        use std::thread;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".socket2.sock");
        let listener = StdUnixListener::bind(&path).expect("bind");
        listener.set_nonblocking(false).expect("set blocking");

        let writer = thread::spawn(move || {
            let (mut conn, _) = listener.accept().expect("accept");
            conn.write_all(b"openwindow>>5589abcd1234,1,firefox,Mozilla Firefox\n")
                .expect("write 1");
            conn.write_all(b"closewindow>>5589abcd1234\n")
                .expect("write 2");
            // Drop the connection to signal EOF.
        });

        let stream = EventStream::connect_at(&path).await.expect("connect");
        let events = collect(stream).await;
        writer.join().expect("writer join");

        assert_eq!(
            events,
            vec![
                Event::WindowOpened {
                    address: "5589abcd1234".into(),
                    workspace: "1".into(),
                    app_id: "firefox".into(),
                    title: "Mozilla Firefox".into(),
                },
                Event::WindowClosed {
                    address: "5589abcd1234".into(),
                },
            ]
        );
    }
}
