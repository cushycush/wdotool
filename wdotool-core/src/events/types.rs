//! Public types for the events module.
//!
//! `Event` is the typed projection of Drift's IPC socket2 stream. Only
//! the events a workflow-trigger consumer would actually build on are
//! included in v1. The enum is `#[non_exhaustive]` so adding variants
//! later isn't a breaking change.
//!
//! Events that arrive in both legacy and v2 form on the wire
//! (`activewindow` / `activewindowv2`, `workspace` / `workspacev2`,
//! `focusedmon` / `focusedmonv2`, `windowtitle` / `windowtitlev2`) are
//! coalesced. The parser prefers the v2 payload (which carries the
//! window address / workspace id) and discards the duplicate legacy
//! line, so consumers see one event per real compositor transition,
//! not two.

use serde::{Deserialize, Serialize};

/// A compositor lifecycle event.
///
/// Address strings are the same hexadecimal pointer Drift uses
/// elsewhere in its IPC (matches `wdotool getactivewindow`'s window
/// ID format on the Drift backend, when that lands).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Event {
    /// A new window was mapped. Carries everything the compositor
    /// emits at open time. `app_id` is the X11 / Wayland class string.
    WindowOpened {
        address: String,
        workspace: String,
        app_id: String,
        title: String,
    },

    /// A window was unmapped. Only the address is reported; the
    /// consumer should look up cached metadata if they need more.
    WindowClosed { address: String },

    /// Keyboard focus moved to a different window. `None` when focus
    /// left every window (e.g. all windows closed on the active
    /// workspace). The legacy `activewindow` line (class + title) is
    /// merged into the v2 line (address) so the consumer gets one
    /// fully-populated event per focus change.
    ActiveWindowChanged(Option<ActiveWindow>),

    /// A window's title changed. Coalesces `windowtitle` (address
    /// only) and `windowtitlev2` (address + new title).
    WindowTitleChanged { address: String, title: String },

    /// A window's fullscreen state toggled. `true` means the window
    /// is now fullscreen, `false` means it isn't. Drift emits this on
    /// the focused window; no address is included on the wire.
    FullscreenChanged { fullscreen: bool },

    /// A window requested user attention (the urgency hint).
    WindowUrgent { address: String },

    /// A wlr-foreign-toplevel client minimized or un-minimized the
    /// window. `minimized = true` is the minimize, `false` is the
    /// restore.
    WindowMinimized { address: String, minimized: bool },

    /// The active workspace changed. Coalesces `workspace` (name only)
    /// and `workspacev2` (id + name).
    WorkspaceChanged { id: Option<i64>, name: String },

    /// A new workspace was created (the compositor allocated it,
    /// usually because a window was sent there).
    WorkspaceCreated { id: Option<i64>, name: String },

    /// A workspace was destroyed (became empty).
    WorkspaceDestroyed { id: Option<i64>, name: String },

    /// Keyboard focus moved to a different monitor. `workspace` is
    /// the workspace that's active on the newly-focused monitor. The
    /// v1 / v2 lines are merged: name from v1, id from v2.
    MonitorFocusChanged {
        monitor: String,
        workspace: MonitorFocusWorkspace,
    },

    /// A monitor was hot-plugged. v2's id + short description are
    /// carried through when the compositor emits them.
    MonitorAdded {
        id: Option<i64>,
        name: String,
        description: Option<String>,
    },

    /// A monitor was unplugged.
    MonitorRemoved {
        id: Option<i64>,
        name: String,
        description: Option<String>,
    },
}

/// The window the compositor focused. `app_id` and `title` are
/// populated when the legacy `activewindow` line carried them;
/// otherwise the v2 line stands alone with just the address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveWindow {
    pub address: String,
    pub app_id: Option<String>,
    pub title: Option<String>,
}

/// The workspace on a newly-focused monitor. Drift emits the name in
/// `focusedmon` and the id in `focusedmonv2`; we keep both, plus the
/// raw legacy string for special workspaces that don't have an id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorFocusWorkspace {
    pub id: Option<i64>,
    pub name: String,
}
