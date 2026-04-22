use std::time::Duration;

use async_trait::async_trait;

use crate::error::Result;
use crate::types::{Capabilities, KeyDirection, MouseButton, WindowId, WindowInfo};

pub mod detector;
pub mod libei;
pub mod stub;
pub mod uinput;
pub mod wlroots;

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
}

pub type DynBackend = Box<dyn Backend>;
