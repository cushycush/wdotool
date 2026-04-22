//! GNOME backend — libei input plus window management via a companion
//! GNOME Shell extension exposing a D-Bus service on the session bus.
//!
//! GNOME Shell does not expose a generic external window API over D-Bus or
//! Wayland protocols (unlike KDE's KWin scripting interface or the wlroots
//! foreign-toplevel protocol), so a small Shell extension is required for
//! `search` / `getactivewindow` / `windowactivate` / `windowclose`. Ship
//! target: `packaging/gnome-extension/wdotool@wdotool.github.io/`.
//!
//! Input still goes through libei over the GNOME RemoteDesktop portal —
//! this backend is a strict superset of bare libei on GNOME sessions.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tracing::debug;
use zbus::{proxy, Connection};

use super::libei::LibeiBackend;
use super::Backend;
use crate::error::{Result, WdoError};
use crate::types::{Capabilities, KeyDirection, MouseButton, WindowId, WindowInfo};

const NAME: &str = "gnome-ext";

#[proxy(
    interface = "org.wdotool.GnomeShellBridge",
    default_service = "org.wdotool.GnomeShellBridge",
    default_path = "/org/wdotool/GnomeShellBridge"
)]
trait Bridge {
    #[zbus(name = "ListWindows")]
    fn list_windows(&self) -> zbus::Result<String>;
    #[zbus(name = "GetActiveWindow")]
    fn get_active_window(&self) -> zbus::Result<String>;
    #[zbus(name = "ActivateWindow")]
    fn activate_window(&self, id: &str) -> zbus::Result<bool>;
    #[zbus(name = "CloseWindow")]
    fn close_window(&self, id: &str) -> zbus::Result<bool>;
}

pub struct GnomeExtBackend {
    libei: LibeiBackend,
    proxy: BridgeProxy<'static>,
    input_caps: Capabilities,
}

#[derive(Deserialize)]
struct ExtWindow {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
}

impl From<ExtWindow> for WindowInfo {
    fn from(w: ExtWindow) -> Self {
        WindowInfo {
            id: WindowId(w.id),
            title: w.title,
            app_id: w.app_id,
            pid: w.pid,
        }
    }
}

impl GnomeExtBackend {
    pub async fn try_new() -> Result<Self> {
        let libei = LibeiBackend::try_new()
            .await
            .map_err(|err| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("libei input init failed: {err}"),
            })?;
        let input_caps = libei.capabilities();

        let conn = Connection::session().await.map_err(dbus_err)?;
        let proxy = BridgeProxy::new(&conn)
            .await
            .map_err(|e| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!(
                    "bridge proxy init failed: {e}. \
                     Install the wdotool GNOME Shell extension from \
                     packaging/gnome-extension/wdotool@wdotool.github.io/ and \
                     enable it with `gnome-extensions enable wdotool@wdotool.github.io`."
                ),
            })?;

        // Ping the service so we fail fast if the extension isn't enabled
        // (zbus proxies connect lazily on the first real call). Using the
        // cheapest method that returns a value so any RPC framing issue
        // surfaces here rather than inside list_windows / active_window.
        proxy
            .get_active_window()
            .await
            .map_err(|e| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!(
                    "wdotool GNOME Shell extension is not responding ({e}). \
                     Run `gnome-extensions enable wdotool@wdotool.github.io` \
                     or install from packaging/gnome-extension/."
                ),
            })?;

        debug!("gnome bridge proxy ready");
        Ok(Self {
            libei,
            proxy,
            input_caps,
        })
    }
}

fn dbus_err(e: zbus::Error) -> WdoError {
    WdoError::Backend {
        backend: NAME,
        source: anyhow::Error::new(e),
    }
}

#[async_trait]
impl Backend for GnomeExtBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn capabilities(&self) -> Capabilities {
        let mut caps = self.input_caps.clone();
        caps.list_windows = true;
        caps.active_window = true;
        caps.activate_window = true;
        caps.close_window = true;
        caps
    }

    async fn key(&self, keysym: &str, dir: KeyDirection) -> Result<()> {
        self.libei.key(keysym, dir).await
    }

    async fn type_text(&self, text: &str, delay: Duration) -> Result<()> {
        self.libei.type_text(text, delay).await
    }

    async fn mouse_move(&self, x: i32, y: i32, absolute: bool) -> Result<()> {
        self.libei.mouse_move(x, y, absolute).await
    }

    async fn mouse_button(&self, btn: MouseButton, dir: KeyDirection) -> Result<()> {
        self.libei.mouse_button(btn, dir).await
    }

    async fn scroll(&self, dx: f64, dy: f64) -> Result<()> {
        self.libei.scroll(dx, dy).await
    }

    async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        let json = self.proxy.list_windows().await.map_err(dbus_err)?;
        let parsed: Vec<ExtWindow> =
            serde_json::from_str(&json).map_err(|e| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("invalid windows payload: {e}"),
            })?;
        Ok(parsed.into_iter().map(Into::into).collect())
    }

    async fn active_window(&self) -> Result<Option<WindowInfo>> {
        let json = self.proxy.get_active_window().await.map_err(dbus_err)?;
        if json.is_empty() || json == "null" {
            return Ok(None);
        }
        let w: ExtWindow = serde_json::from_str(&json).map_err(|e| WdoError::Backend {
            backend: NAME,
            source: anyhow::anyhow!("invalid active-window payload: {e}"),
        })?;
        Ok(Some(w.into()))
    }

    async fn activate_window(&self, id: &WindowId) -> Result<()> {
        let ok = self.proxy.activate_window(&id.0).await.map_err(dbus_err)?;
        if !ok {
            return Err(WdoError::WindowNotFound(id.0.clone()));
        }
        Ok(())
    }

    async fn close_window(&self, id: &WindowId) -> Result<()> {
        let ok = self.proxy.close_window(&id.0).await.map_err(dbus_err)?;
        if !ok {
            return Err(WdoError::WindowNotFound(id.0.clone()));
        }
        Ok(())
    }
}
