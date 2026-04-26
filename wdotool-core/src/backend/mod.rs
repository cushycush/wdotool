use std::time::Duration;

use async_trait::async_trait;

use crate::error::Result;
use crate::types::{Capabilities, KeyDirection, MouseButton, WindowId, WindowInfo};

pub mod detector;

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
}

pub type DynBackend = Box<dyn Backend>;
