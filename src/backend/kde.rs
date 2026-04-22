//! KDE backend — libei input plus KWin-scripting window management over D-Bus.
//!
//! The window half uses the same trick as `kdotool`: generate a JavaScript
//! snippet, hand it to `org.kde.KWin.Scripting.loadScriptFromText`, run it,
//! and let the script call back into a zbus service we registered at
//! startup. This avoids the `print`-signal-scraping approach (fragile and
//! needs journal access) and works on Plasma 5 and 6 identically.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::{oneshot, Mutex};
use tracing::debug;
use zbus::{interface, proxy, Connection};

use super::libei::LibeiBackend;
use super::Backend;
use crate::error::{Result, WdoError};
use crate::types::{Capabilities, KeyDirection, MouseButton, WindowId, WindowInfo};

const NAME: &str = "kde-dbus";

const BRIDGE_SERVICE: &str = "com.wdotool.KdeBridge";
const BRIDGE_PATH: &str = "/com/wdotool/KdeBridge";
const BRIDGE_IFACE: &str = "com.wdotool.KdeBridge";

pub struct KdeBackend {
    libei: LibeiBackend,
    conn: Connection,
    pending: Arc<Mutex<PendingState>>,
    next_id: AtomicU64,
    input_caps: Capabilities,
}

#[derive(Default)]
struct PendingState {
    list_waiters: HashMap<u64, oneshot::Sender<String>>,
    active_waiters: HashMap<u64, oneshot::Sender<String>>,
    action_waiters: HashMap<u64, oneshot::Sender<bool>>,
}

/// D-Bus interface our KWin scripts call back into. Names are PascalCase on
/// the wire so `callDBus(..., "ReportWindows", ...)` matches zbus's default
/// camelCase → PascalCase mapping.
struct Bridge {
    pending: Arc<Mutex<PendingState>>,
}

#[interface(name = "com.wdotool.KdeBridge")]
impl Bridge {
    async fn report_windows(&self, req_id: u64, json: String) {
        let sender = self.pending.lock().await.list_waiters.remove(&req_id);
        if let Some(tx) = sender {
            let _ = tx.send(json);
        }
    }

    async fn report_active(&self, req_id: u64, json: String) {
        let sender = self.pending.lock().await.active_waiters.remove(&req_id);
        if let Some(tx) = sender {
            let _ = tx.send(json);
        }
    }

    async fn report_action(&self, req_id: u64, ok: bool) {
        let sender = self.pending.lock().await.action_waiters.remove(&req_id);
        if let Some(tx) = sender {
            let _ = tx.send(ok);
        }
    }
}

#[proxy(
    interface = "org.kde.kwin.Scripting",
    default_service = "org.kde.KWin",
    default_path = "/Scripting"
)]
trait KwinScripting {
    #[zbus(name = "loadScriptFromText")]
    fn load_script_from_text(&self, source: &str, name: &str) -> zbus::Result<i32>;
    fn start(&self) -> zbus::Result<()>;
}

#[proxy(interface = "org.kde.kwin.Script", default_service = "org.kde.KWin")]
trait KwinScript {
    fn run(&self) -> zbus::Result<()>;
}

#[derive(Deserialize)]
struct ScriptWindow {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
}

impl From<ScriptWindow> for WindowInfo {
    fn from(s: ScriptWindow) -> Self {
        WindowInfo {
            id: WindowId(s.id),
            title: s.title,
            app_id: s.app_id,
            pid: s.pid,
        }
    }
}

impl KdeBackend {
    pub async fn try_new() -> Result<Self> {
        // Input path: libei over the KDE portal. KDE Plasma ships a
        // RemoteDesktop portal implementation, so this bootstraps cleanly
        // on actual KDE sessions.
        let libei = LibeiBackend::try_new()
            .await
            .map_err(|err| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("libei input init failed: {err}"),
            })?;
        let input_caps = libei.capabilities();

        let conn = Connection::session().await.map_err(dbus_err)?;

        let pending: Arc<Mutex<PendingState>> = Arc::new(Mutex::new(PendingState::default()));
        conn.object_server()
            .at(
                BRIDGE_PATH,
                Bridge {
                    pending: pending.clone(),
                },
            )
            .await
            .map_err(dbus_err)?;
        conn.request_name(BRIDGE_SERVICE).await.map_err(dbus_err)?;

        debug!("kde bridge registered at {BRIDGE_PATH}");
        Ok(Self {
            libei,
            conn,
            pending,
            next_id: AtomicU64::new(1),
            input_caps,
        })
    }

    fn next_request_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn run_kwin_script(&self, script: &str) -> Result<()> {
        let scripting = KwinScriptingProxy::new(&self.conn)
            .await
            .map_err(dbus_err)?;
        let script_id = scripting
            .load_script_from_text(script, "wdotool")
            .await
            .map_err(dbus_err)?;
        if script_id < 0 {
            return Err(WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("loadScriptFromText returned {script_id}"),
            });
        }
        // Plasma 6 runs newly-loaded scripts when `start()` is called on
        // the Scripting object; Plasma 5 auto-starts on load. Calling
        // start() is idempotent on both.
        let _ = scripting.start().await;
        Ok(())
    }

    async fn list_windows_impl(&self) -> Result<Vec<WindowInfo>> {
        let req_id = self.next_request_id();
        let (tx, rx) = oneshot::channel::<String>();
        self.pending.lock().await.list_waiters.insert(req_id, tx);

        let script = list_windows_script(req_id);
        self.run_kwin_script(&script).await?;

        let json = tokio::time::timeout(Duration::from_secs(3), rx)
            .await
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("timed out waiting for KWin script callback"),
            })?
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("KWin script aborted before callback"),
            })?;

        let parsed: Vec<ScriptWindow> =
            serde_json::from_str(&json).map_err(|e| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("invalid windows payload: {e}"),
            })?;
        Ok(parsed.into_iter().map(Into::into).collect())
    }

    async fn active_window_impl(&self) -> Result<Option<WindowInfo>> {
        let req_id = self.next_request_id();
        let (tx, rx) = oneshot::channel::<String>();
        self.pending.lock().await.active_waiters.insert(req_id, tx);

        let script = active_window_script(req_id);
        self.run_kwin_script(&script).await?;

        let json = tokio::time::timeout(Duration::from_secs(3), rx)
            .await
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("timed out waiting for KWin script callback"),
            })?
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("KWin script aborted before callback"),
            })?;

        if json.is_empty() || json == "null" {
            return Ok(None);
        }
        let parsed: ScriptWindow = serde_json::from_str(&json).map_err(|e| WdoError::Backend {
            backend: NAME,
            source: anyhow::anyhow!("invalid active-window payload: {e}"),
        })?;
        Ok(Some(parsed.into()))
    }

    async fn action_impl(&self, script: String, what: &'static str) -> Result<()> {
        let req_id = self.next_request_id();
        let (tx, rx) = oneshot::channel::<bool>();
        self.pending.lock().await.action_waiters.insert(req_id, tx);

        self.run_kwin_script(&script).await?;

        let ok = tokio::time::timeout(Duration::from_secs(3), rx)
            .await
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("timed out waiting for KWin {what} callback"),
            })?
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("KWin {what} script aborted before callback"),
            })?;
        if !ok {
            return Err(WdoError::WindowNotFound(format!("{what} target")));
        }
        Ok(())
    }
}

fn dbus_err(e: zbus::Error) -> WdoError {
    WdoError::Backend {
        backend: NAME,
        source: anyhow::Error::new(e),
    }
}

// ---- JS script generators --------------------------------------------------

fn list_windows_script(req_id: u64) -> String {
    format!(
        r#"
(function() {{
  var out = [];
  var list = (typeof workspace.windowList === "function")
    ? workspace.windowList()
    : workspace.clientList();
  for (var i = 0; i < list.length; i++) {{
    var w = list[i];
    out.push({{
      id: (w.internalId || w.windowId || i).toString(),
      title: String(w.caption || ""),
      app_id: String(w.resourceClass || w.resourceName || ""),
      pid: (w.pid | 0)
    }});
  }}
  callDBus(
    "{service}", "{path}", "{iface}", "ReportWindows",
    {id}, JSON.stringify(out)
  );
}})();
"#,
        service = BRIDGE_SERVICE,
        path = BRIDGE_PATH,
        iface = BRIDGE_IFACE,
        id = req_id
    )
}

fn active_window_script(req_id: u64) -> String {
    format!(
        r#"
(function() {{
  var w = workspace.activeWindow || workspace.activeClient;
  var payload = "null";
  if (w) {{
    payload = JSON.stringify({{
      id: (w.internalId || w.windowId || 0).toString(),
      title: String(w.caption || ""),
      app_id: String(w.resourceClass || w.resourceName || ""),
      pid: (w.pid | 0)
    }});
  }}
  callDBus(
    "{service}", "{path}", "{iface}", "ReportActive",
    {id}, payload
  );
}})();
"#,
        service = BRIDGE_SERVICE,
        path = BRIDGE_PATH,
        iface = BRIDGE_IFACE,
        id = req_id
    )
}

fn activate_window_script(req_id: u64, target_id: &str) -> String {
    format!(
        r#"
(function() {{
  var target = {target:?};
  var list = (typeof workspace.windowList === "function")
    ? workspace.windowList()
    : workspace.clientList();
  var found = false;
  for (var i = 0; i < list.length; i++) {{
    var w = list[i];
    var id = (w.internalId || w.windowId || i).toString();
    if (id === target) {{
      workspace.activeWindow = w;
      found = true;
      break;
    }}
  }}
  callDBus(
    "{service}", "{path}", "{iface}", "ReportAction",
    {id}, found
  );
}})();
"#,
        service = BRIDGE_SERVICE,
        path = BRIDGE_PATH,
        iface = BRIDGE_IFACE,
        id = req_id,
        target = target_id
    )
}

fn close_window_script(req_id: u64, target_id: &str) -> String {
    format!(
        r#"
(function() {{
  var target = {target:?};
  var list = (typeof workspace.windowList === "function")
    ? workspace.windowList()
    : workspace.clientList();
  var found = false;
  for (var i = 0; i < list.length; i++) {{
    var w = list[i];
    var id = (w.internalId || w.windowId || i).toString();
    if (id === target) {{
      if (typeof w.closeWindow === "function") {{
        w.closeWindow();
      }} else if (typeof w.close === "function") {{
        w.close();
      }}
      found = true;
      break;
    }}
  }}
  callDBus(
    "{service}", "{path}", "{iface}", "ReportAction",
    {id}, found
  );
}})();
"#,
        service = BRIDGE_SERVICE,
        path = BRIDGE_PATH,
        iface = BRIDGE_IFACE,
        id = req_id,
        target = target_id
    )
}

// ---- Backend trait impl ----------------------------------------------------

#[async_trait]
impl Backend for KdeBackend {
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
        self.list_windows_impl().await
    }

    async fn active_window(&self) -> Result<Option<WindowInfo>> {
        self.active_window_impl().await
    }

    async fn activate_window(&self, id: &WindowId) -> Result<()> {
        let req_id = self.next_request_id();
        let script = activate_window_script(req_id, &id.0);
        // Need to register the waiter with the SAME req_id the script uses.
        // action_impl does that, but it generates its own id; reimplement
        // inline so the ids line up.
        let (tx, rx) = oneshot::channel::<bool>();
        self.pending.lock().await.action_waiters.insert(req_id, tx);
        self.run_kwin_script(&script).await?;
        let ok = tokio::time::timeout(Duration::from_secs(3), rx)
            .await
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("timed out waiting for KWin activate callback"),
            })?
            .unwrap_or(false);
        if !ok {
            return Err(WdoError::WindowNotFound(id.0.clone()));
        }
        Ok(())
    }

    async fn close_window(&self, id: &WindowId) -> Result<()> {
        let req_id = self.next_request_id();
        let script = close_window_script(req_id, &id.0);
        let (tx, rx) = oneshot::channel::<bool>();
        self.pending.lock().await.action_waiters.insert(req_id, tx);
        self.run_kwin_script(&script).await?;
        let ok = tokio::time::timeout(Duration::from_secs(3), rx)
            .await
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("timed out waiting for KWin close callback"),
            })?
            .unwrap_or(false);
        if !ok {
            return Err(WdoError::WindowNotFound(id.0.clone()));
        }
        Ok(())
    }
}

// Keep `action_impl` around for future refactoring even though activate/close
// currently duplicate its body inline — generic factoring requires returning
// the req_id so callers can register waiters, which is a TODO.
#[allow(dead_code)]
fn _keep_action_impl() {
    let _ = KdeBackend::action_impl;
}
