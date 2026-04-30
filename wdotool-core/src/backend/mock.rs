//! In-memory [`Backend`] that records every call. The whole point of
//! this is to make CLI integration tests possible: a test parses a
//! [`Command`](crate::keysym), drives `dispatch`, and then asserts on
//! `mock.calls()` to check the exact backend trait calls the dispatch
//! produced (modifier ordering, repeat counts, --clearmodifiers
//! semantics, type-string-to-keysym translation, and so on).
//!
//! Read-side methods (`active_window`, `list_windows`, `list_outputs`,
//! `pointer_position`, `window_geometry`) return canned data the test
//! sets up via `MockBackend::set_*`. Write-side methods (`key`,
//! `type_text`, `mouse_*`, `scroll`, `activate_window`, `close_window`)
//! are pure call recorders.
//!
//! Failure injection is closure-based (`fail_with`) because [`WdoError`]
//! isn't `Clone` — every call to a failing method constructs a fresh
//! error from the factory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use super::Backend;
use crate::error::Result;
use crate::types::{
    Capabilities, KeyDirection, MouseButton, OutputInfo, WindowGeometry, WindowId, WindowInfo,
};
use crate::WdoError;

/// One recorded call. Variant per [`Backend`] trait method, holding the
/// arguments the dispatch passed in. Tests typically build an expected
/// `Vec<BackendCall>` and `assert_eq!` against `mock.calls()`.
#[derive(Clone, Debug, PartialEq)]
pub enum BackendCall {
    Key {
        keysym: String,
        dir: KeyDirection,
    },
    TypeText {
        text: String,
        delay: Duration,
    },
    MouseMove {
        x: i32,
        y: i32,
        absolute: bool,
    },
    MouseButton {
        btn: MouseButton,
        dir: KeyDirection,
    },
    Scroll {
        dx: f64,
        dy: f64,
    },
    ListWindows,
    ActiveWindow,
    ActivateWindow(WindowId),
    CloseWindow(WindowId),
    PointerPosition,
    ListOutputs,
    WindowGeometry(WindowId),
}

/// Factory that produces a [`WdoError`] on demand. Used for failure
/// injection because `WdoError` isn't `Clone` — every call to a failing
/// method invokes this to get a fresh error.
type ErrorFactory = Arc<dyn Fn() -> WdoError + Send + Sync>;

struct Inner {
    calls: Vec<BackendCall>,
    windows: Vec<WindowInfo>,
    active: Option<WindowInfo>,
    pointer: Option<(i32, i32)>,
    outputs: Vec<OutputInfo>,
    geometries: HashMap<String, WindowGeometry>,
    fail: Option<ErrorFactory>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            windows: Vec::new(),
            active: None,
            pointer: None,
            outputs: Vec::new(),
            geometries: HashMap::new(),
            fail: None,
        }
    }
}

/// In-memory [`Backend`] that records every call. Cheap to clone — the
/// state is shared via `Arc<Mutex>` so a clone observes the same calls
/// and config as the original. Tests typically pass `&mock` to
/// `dispatch` and then read `mock.calls()` after.
pub struct MockBackend {
    name: &'static str,
    capabilities: Capabilities,
    inner: Arc<Mutex<Inner>>,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBackend {
    /// A backend that claims every capability and starts with empty
    /// canned data. Tests that want a partial-capability backend
    /// (e.g. uinput's send-only profile) call [`with_capabilities`]
    /// after `new`.
    ///
    /// [`with_capabilities`]: Self::with_capabilities
    pub fn new() -> Self {
        Self {
            name: "mock",
            capabilities: Capabilities {
                key_input: true,
                text_input: true,
                pointer_move_absolute: true,
                pointer_move_relative: true,
                pointer_button: true,
                scroll: true,
                list_windows: true,
                active_window: true,
                activate_window: true,
                close_window: true,
                pointer_position: true,
                list_outputs: true,
                window_geometry: true,
            },
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    pub fn with_capabilities(mut self, caps: Capabilities) -> Self {
        self.capabilities = caps;
        self
    }

    pub fn with_name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Snapshot of every call made through this backend. Returns a
    /// fresh `Vec` so tests can `assert_eq!` directly without holding
    /// the lock.
    pub fn calls(&self) -> Vec<BackendCall> {
        self.inner.lock().unwrap().calls.clone()
    }

    /// Drop every recorded call. Useful when a test runs setup ops
    /// (e.g. seeding the active window via a separate call) before the
    /// behavior it actually wants to assert on.
    pub fn clear_calls(&self) {
        self.inner.lock().unwrap().calls.clear();
    }

    pub fn set_windows(&self, windows: Vec<WindowInfo>) {
        self.inner.lock().unwrap().windows = windows;
    }

    pub fn set_active_window(&self, window: Option<WindowInfo>) {
        self.inner.lock().unwrap().active = window;
    }

    pub fn set_pointer(&self, position: Option<(i32, i32)>) {
        self.inner.lock().unwrap().pointer = position;
    }

    pub fn set_outputs(&self, outputs: Vec<OutputInfo>) {
        self.inner.lock().unwrap().outputs = outputs;
    }

    pub fn set_geometry(&self, id: &str, geometry: WindowGeometry) {
        self.inner
            .lock()
            .unwrap()
            .geometries
            .insert(id.to_string(), geometry);
    }

    /// Make every subsequent call return the error this factory
    /// produces. Pass a closure so each invocation can build a fresh
    /// `WdoError` (the type isn't `Clone`).
    pub fn fail_with<F>(&self, factory: F)
    where
        F: Fn() -> WdoError + Send + Sync + 'static,
    {
        self.inner.lock().unwrap().fail = Some(Arc::new(factory));
    }

    fn record(&self, call: BackendCall) -> Option<WdoError> {
        let mut inner = self.inner.lock().unwrap();
        inner.calls.push(call);
        inner.fail.as_ref().map(|f| f())
    }
}

#[async_trait]
impl Backend for MockBackend {
    fn name(&self) -> &'static str {
        self.name
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities.clone()
    }

    async fn key(&self, keysym: &str, dir: KeyDirection) -> Result<()> {
        match self.record(BackendCall::Key {
            keysym: keysym.to_string(),
            dir,
        }) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn type_text(&self, text: &str, delay: Duration) -> Result<()> {
        match self.record(BackendCall::TypeText {
            text: text.to_string(),
            delay,
        }) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn mouse_move(&self, x: i32, y: i32, absolute: bool) -> Result<()> {
        match self.record(BackendCall::MouseMove { x, y, absolute }) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn mouse_button(&self, btn: MouseButton, dir: KeyDirection) -> Result<()> {
        match self.record(BackendCall::MouseButton { btn, dir }) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn scroll(&self, dx: f64, dy: f64) -> Result<()> {
        match self.record(BackendCall::Scroll { dx, dy }) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        if let Some(e) = self.record(BackendCall::ListWindows) {
            return Err(e);
        }
        Ok(self.inner.lock().unwrap().windows.clone())
    }

    async fn active_window(&self) -> Result<Option<WindowInfo>> {
        if let Some(e) = self.record(BackendCall::ActiveWindow) {
            return Err(e);
        }
        Ok(self.inner.lock().unwrap().active.clone())
    }

    async fn activate_window(&self, id: &WindowId) -> Result<()> {
        match self.record(BackendCall::ActivateWindow(id.clone())) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn close_window(&self, id: &WindowId) -> Result<()> {
        match self.record(BackendCall::CloseWindow(id.clone())) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn pointer_position(&self) -> Result<Option<(i32, i32)>> {
        if let Some(e) = self.record(BackendCall::PointerPosition) {
            return Err(e);
        }
        Ok(self.inner.lock().unwrap().pointer)
    }

    async fn list_outputs(&self) -> Result<Vec<OutputInfo>> {
        if let Some(e) = self.record(BackendCall::ListOutputs) {
            return Err(e);
        }
        Ok(self.inner.lock().unwrap().outputs.clone())
    }

    async fn window_geometry(&self, id: &WindowId) -> Result<Option<WindowGeometry>> {
        if let Some(e) = self.record(BackendCall::WindowGeometry(id.clone())) {
            return Err(e);
        }
        Ok(self.inner.lock().unwrap().geometries.get(&id.0).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_each_call_in_order() {
        let mock = MockBackend::new();
        mock.key("a", KeyDirection::Press).await.unwrap();
        mock.scroll(0.0, 3.0).await.unwrap();
        let calls = mock.calls();
        assert_eq!(
            calls,
            vec![
                BackendCall::Key {
                    keysym: "a".into(),
                    dir: KeyDirection::Press,
                },
                BackendCall::Scroll { dx: 0.0, dy: 3.0 },
            ]
        );
    }

    #[tokio::test]
    async fn returns_canned_active_window() {
        let mock = MockBackend::new();
        let info = WindowInfo {
            id: WindowId("win-1".into()),
            title: "Term".into(),
            app_id: Some("kitty".into()),
            pid: Some(42),
        };
        mock.set_active_window(Some(info.clone()));
        let got = mock.active_window().await.unwrap().unwrap();
        assert_eq!(got.id, info.id);
        assert_eq!(got.title, info.title);
    }

    #[tokio::test]
    async fn fail_with_makes_every_method_error() {
        let mock = MockBackend::new();
        mock.fail_with(|| WdoError::InvalidArg("boom".into()));
        let result = mock.key("a", KeyDirection::Press).await;
        assert!(matches!(result, Err(WdoError::InvalidArg(_))));
        // Calls are still recorded even when failing.
        assert_eq!(mock.calls().len(), 1);
    }
}
