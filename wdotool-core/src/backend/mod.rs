use std::time::Duration;

use async_trait::async_trait;

use crate::error::{Result, WdoError};
use crate::types::{
    Capabilities, KeyDirection, MouseButton, OutputInfo, WindowGeometry, WindowId, WindowInfo,
};

pub mod detector;

#[cfg(any(test, feature = "testing"))]
pub mod mock;

#[cfg(feature = "gnome")]
pub(crate) mod gnome;
#[cfg(feature = "kde")]
pub(crate) mod kde;
#[cfg(feature = "libei")]
pub(crate) mod libei;
#[cfg(feature = "uinput")]
pub(crate) mod uinput;
#[cfg(feature = "wlroots")]
pub(crate) mod wlroots;

#[async_trait]
pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;

    async fn key(&self, keysym: &str, dir: KeyDirection) -> Result<()>;
    async fn type_text(&self, text: &str, delay: Duration) -> Result<()>;

    async fn mouse_move(&self, x: i32, y: i32, absolute: bool) -> Result<()>;
    async fn mouse_button(&self, btn: MouseButton, dir: KeyDirection) -> Result<()>;
    async fn scroll(&self, dx: f64, dy: f64) -> Result<()>;

    /// Move the pointer to `(x, y)` interpreted as output-local
    /// coordinates within the named output. The default implementation
    /// looks up the output's origin via [`list_outputs`], translates
    /// to global compositor coords, and falls through to
    /// [`mouse_move`] with `absolute = true`. The wlroots backend
    /// overrides this to bind a per-output `virtual_pointer` and send
    /// `motion_absolute` against that output's mode dimensions
    /// directly, since wlroots' single-pointer `motion_absolute` ties
    /// the extent to the primary output (placing the cursor on the
    /// wrong monitor for non-primary `--output` calls).
    ///
    /// [`list_outputs`]: Backend::list_outputs
    /// [`mouse_move`]: Backend::mouse_move
    async fn mouse_move_to_output(&self, output: &str, x: i32, y: i32) -> Result<()> {
        let outputs = self.list_outputs().await?;
        if outputs.is_empty() {
            return Err(WdoError::InvalidArg(format!(
                "--output not supported: the {} backend does not enumerate outputs",
                self.name()
            )));
        }
        let target = outputs.iter().find(|o| o.name == output).ok_or_else(|| {
            let available: Vec<&str> = outputs.iter().map(|o| o.name.as_str()).collect();
            WdoError::InvalidArg(format!(
                "no output named {output:?}; available: {}",
                available.join(", ")
            ))
        })?;
        self.mouse_move(target.x + x, target.y + y, true).await
    }

    async fn list_windows(&self) -> Result<Vec<WindowInfo>>;
    async fn active_window(&self) -> Result<Option<WindowInfo>>;
    async fn activate_window(&self, id: &WindowId) -> Result<()>;
    async fn close_window(&self, id: &WindowId) -> Result<()>;

    /// Read the compositor's current pointer position in screen
    /// coordinates. Returns `Ok(None)` for backends that can't expose
    /// it: libei is send-only by design, wlroots' virtual-pointer is
    /// likewise send-only with no read protocol, and uinput is at the
    /// kernel layer with no notion of "screen". KDE reads via a
    /// transient kwin script, GNOME via the companion Shell extension.
    async fn pointer_position(&self) -> Result<Option<(i32, i32)>> {
        Ok(None)
    }

    /// Enumerate the compositor's outputs (monitors). Returns an empty
    /// vector for backends that don't enumerate outputs; the wlroots
    /// backend is the first to populate it (via `wl_output`). KDE and
    /// GNOME each have their own enumeration path that hasn't been
    /// wired up yet; libei has device regions but no name mapping;
    /// uinput is at the kernel layer with no notion of monitors.
    async fn list_outputs(&self) -> Result<Vec<OutputInfo>> {
        Ok(Vec::new())
    }

    /// Read the frame position + size of a window by id. Returns
    /// `Ok(None)` for backends that can't read window geometry:
    /// wlroots' `zwlr_foreign_toplevel_management_v1` doesn't expose
    /// geometry, libei has no window concept at all, uinput is at the
    /// kernel layer. KDE reads it via a transient kwin script
    /// (`window.frameGeometry`); GNOME via the companion Shell
    /// extension (`MetaWindow.get_frame_rect()`). Backends that do
    /// support it but receive an unknown id should return
    /// `Err(WdoError::WindowNotFound)`, matching the existing
    /// `getwindowname` / `getwindowpid` behavior.
    async fn window_geometry(&self, _id: &WindowId) -> Result<Option<WindowGeometry>> {
        Ok(None)
    }
}

pub type DynBackend = Box<dyn Backend>;
