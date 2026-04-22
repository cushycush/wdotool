use std::time::Duration;

use async_trait::async_trait;

use super::Backend;
use crate::error::{Result, WdoError};
use crate::types::{Capabilities, KeyDirection, MouseButton, WindowId, WindowInfo};

/// A backend slot whose real implementation has not landed yet. It reports its
/// intended name and capabilities so `wdotool info` stays honest, and every op
/// returns `NotSupported` with a clear backend label.
pub struct PendingBackend {
    pub name: &'static str,
    pub caps: Capabilities,
}

impl PendingBackend {
    fn unsupported<T>(&self, what: &'static str) -> Result<T> {
        Err(WdoError::NotSupported {
            backend: self.name,
            what,
        })
    }
}

#[async_trait]
impl Backend for PendingBackend {
    fn name(&self) -> &'static str {
        self.name
    }

    fn capabilities(&self) -> Capabilities {
        self.caps.clone()
    }

    async fn key(&self, _keysym: &str, _dir: KeyDirection) -> Result<()> {
        self.unsupported("key")
    }

    async fn type_text(&self, _text: &str, _delay: Duration) -> Result<()> {
        self.unsupported("type_text")
    }

    async fn mouse_move(&self, _x: i32, _y: i32, _absolute: bool) -> Result<()> {
        self.unsupported("mouse_move")
    }

    async fn mouse_button(&self, _btn: MouseButton, _dir: KeyDirection) -> Result<()> {
        self.unsupported("mouse_button")
    }

    async fn scroll(&self, _dx: f64, _dy: f64) -> Result<()> {
        self.unsupported("scroll")
    }

    async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        self.unsupported("list_windows")
    }

    async fn active_window(&self) -> Result<Option<WindowInfo>> {
        self.unsupported("active_window")
    }

    async fn activate_window(&self, _id: &WindowId) -> Result<()> {
        self.unsupported("activate_window")
    }

    async fn close_window(&self, _id: &WindowId) -> Result<()> {
        self.unsupported("close_window")
    }
}
