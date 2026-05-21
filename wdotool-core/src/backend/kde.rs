//! KDE backend: libei input plus KWin-scripting window management over D-Bus.
//!
//! The window half uses the same trick as `kdotool`: generate a JavaScript
//! snippet, write it to a temp file, hand the path to
//! `org.kde.kwin.Scripting.loadScript(path, pluginName)`, run it via the
//! per-script `org.kde.kwin.Script.run()` method, and let the script call
//! back into a zbus service we registered at startup. The callback shape
//! is more reliable than scraping `print`-signal output from the journal
//! and works on Plasma 5 and 6 identically.
//!
//! We used to use `loadScriptFromText`, which took the JS as a string and
//! ran it synchronously. Plasma 6 removed that method from the public
//! Scripting interface, so every window op was failing with
//! "org.freedesktop.DBus.Error.UnknownMethod" on Plasma 6 sessions.
//! The file-based `loadScript` path is what KWin master still exposes
//! and what kdotool uses.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tempfile::TempPath;
use tokio::sync::{oneshot, Mutex};
use tracing::{debug, warn};
use zbus::{interface, proxy, Connection};

use super::libei::LibeiBackend;
use super::Backend;
use crate::error::{Result, WdoError};
use crate::types::{Capabilities, KeyDirection, MouseButton, WindowGeometry, WindowId, WindowInfo};

const NAME: &str = "kde-dbus";

const BRIDGE_PATH: &str = "/com/wdotool/KdeBridge";
const BRIDGE_IFACE: &str = "com.wdotool.KdeBridge";

pub struct KdeBackend {
    libei: LibeiBackend,
    conn: Connection,
    /// Our session bus unique name (e.g. `:1.123`). The KWin scripts we
    /// load call back here via `callDBus`. We used to use a well-known
    /// name, but Plasma 6's `loadScript`-loaded scripts can't reach
    /// well-known names from inside KWin's scripting D-Bus context;
    /// the unique name is the only address that gets through.
    bridge_addr: String,
    pending: Arc<Mutex<PendingState>>,
    /// `i32` because KWin's `callDBus` serializes JS numbers as int32 on
    /// the wire, and the bridge methods have to advertise a matching
    /// signature; otherwise zbus silently drops the call.
    next_id: AtomicI32,
    input_caps: Capabilities,
}

/// A loaded KWin script that still needs unloading.
///
/// The `TempPath` keeps the script file on disk until the handle drops.
/// We hold the script file alive until after the callback arrives because
/// some KWin versions read the file lazily.
struct KwinScriptHandle {
    plugin_name: String,
    _path: TempPath,
}

#[derive(Default)]
struct PendingState {
    list_waiters: HashMap<i32, oneshot::Sender<String>>,
    active_waiters: HashMap<i32, oneshot::Sender<String>>,
    action_waiters: HashMap<i32, oneshot::Sender<bool>>,
    pointer_waiters: HashMap<i32, oneshot::Sender<Option<(i32, i32)>>>,
    /// `Option<WindowGeometry>` lets us distinguish "found, here it is"
    /// (`Some(geom)`) from "id didn't match any window" (`None`),
    /// which the caller turns into `Err(WdoError::WindowNotFound)`.
    geometry_waiters: HashMap<i32, oneshot::Sender<Option<WindowGeometry>>>,
}

/// D-Bus interface our KWin scripts call back into. Names are PascalCase on
/// the wire so `callDBus(..., "ReportWindows", ...)` matches zbus's default
/// camelCase → PascalCase mapping.
struct Bridge {
    pending: Arc<Mutex<PendingState>>,
}

#[interface(name = "com.wdotool.KdeBridge")]
impl Bridge {
    async fn report_windows(&self, req_id: i32, json: String) {
        let sender = self.pending.lock().await.list_waiters.remove(&req_id);
        if let Some(tx) = sender {
            let _ = tx.send(json);
        }
    }

    async fn report_active(&self, req_id: i32, json: String) {
        let sender = self.pending.lock().await.active_waiters.remove(&req_id);
        if let Some(tx) = sender {
            let _ = tx.send(json);
        }
    }

    async fn report_action(&self, req_id: i32, ok: bool) {
        let sender = self.pending.lock().await.action_waiters.remove(&req_id);
        if let Some(tx) = sender {
            let _ = tx.send(ok);
        }
    }

    /// Pointer-position result. `ok=false` means the script couldn't
    /// read `workspace.cursorPos` (very old KWin? unusual session?); the
    /// x/y values are then placeholders and the caller turns it into
    /// `Ok(None)`. xdotool's getmouselocation only ever returns
    /// coordinates, so this two-stage shape exists purely for safety
    /// against the rare "cursor not on any screen" case.
    async fn report_pointer(&self, req_id: i32, ok: bool, x: i32, y: i32) {
        let sender = self.pending.lock().await.pointer_waiters.remove(&req_id);
        if let Some(tx) = sender {
            let _ = tx.send(if ok { Some((x, y)) } else { None });
        }
    }

    /// Window-geometry result. `found=false` means no window matched
    /// the requested id; the caller maps that to
    /// `WdoError::WindowNotFound`. When `found=true`, the four geometry
    /// fields carry the frame rect.
    async fn report_geometry(
        &self,
        req_id: i32,
        found: bool,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) {
        let sender = self.pending.lock().await.geometry_waiters.remove(&req_id);
        if let Some(tx) = sender {
            let payload = if found {
                Some(WindowGeometry {
                    x,
                    y,
                    width: width.max(0) as u32,
                    height: height.max(0) as u32,
                })
            } else {
                None
            };
            let _ = tx.send(payload);
        }
    }
}

#[proxy(
    interface = "org.kde.kwin.Scripting",
    default_service = "org.kde.KWin",
    default_path = "/Scripting"
)]
trait KwinScripting {
    #[zbus(name = "loadScript")]
    fn load_script(&self, file_path: &str, plugin_name: &str) -> zbus::Result<i32>;
    #[zbus(name = "unloadScript")]
    fn unload_script(&self, plugin_name: &str) -> zbus::Result<bool>;
}

/// Per-script proxy. Path is `/Scripting/Script{id}` where `id` is the
/// integer that `loadScript` returned. Calling `run()` here is what
/// actually executes the script on Plasma 6; the old `Scripting.start()`
/// trick relied on `loadScriptFromText`'s auto-start behavior, which is
/// no longer available.
#[proxy(interface = "org.kde.kwin.Script", default_service = "org.kde.KWin")]
trait KwinScript {
    // zbus's default rename is camelCase → PascalCase (`run` → `Run`),
    // but KWin's interface is lower-camelCase, so pin the names explicitly.
    #[zbus(name = "run")]
    fn run(&self) -> zbus::Result<()>;
    #[zbus(name = "stop")]
    fn stop(&self) -> zbus::Result<()>;
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
                source: format!("libei input init failed: {err}").into(),
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

        // The bridge address the script will call back on. We don't
        // `request_name` a well-known name because Plasma 6's
        // `loadScript`-loaded scripts can't reach well-known names via
        // `callDBus` (the calls dispatch but never arrive). Routing to
        // our unique session-bus name is what kdotool does too.
        let bridge_addr = conn
            .unique_name()
            .ok_or_else(|| WdoError::Backend {
                backend: NAME,
                source: "session bus did not assign a unique name".into(),
            })?
            .to_string();

        debug!("kde bridge registered at {BRIDGE_PATH} on {bridge_addr}");
        Ok(Self {
            libei,
            conn,
            bridge_addr,
            pending,
            next_id: AtomicI32::new(1),
            input_caps,
        })
    }

    fn next_request_id(&self) -> i32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Load and run a KWin script. Returns a handle whose lifetime keeps
    /// the temp file alive; call `cleanup_script` after the callback
    /// arrives (or on timeout) to unload the script in KWin and remove
    /// the file.
    async fn run_kwin_script(&self, req_id: i32, script: &str) -> Result<KwinScriptHandle> {
        // Each loaded script needs a unique plugin name; KWin rejects
        // `loadScript` if a plugin with the same name is already loaded.
        // PID + req_id is unique within a process and across the rare
        // overlap with another wdotool instance.
        let plugin_name = format!("wdotool-{}-{}", std::process::id(), req_id);

        let mut tmp = tempfile::Builder::new()
            .prefix("wdotool-")
            .suffix(".js")
            .tempfile()
            .map_err(io_err)?;
        tmp.write_all(script.as_bytes()).map_err(io_err)?;
        tmp.flush().map_err(io_err)?;
        let path = tmp.into_temp_path();
        let path_str = path.to_str().ok_or_else(|| WdoError::Backend {
            backend: NAME,
            source: "temp script path is not valid UTF-8".into(),
        })?;

        let scripting = KwinScriptingProxy::new(&self.conn)
            .await
            .map_err(dbus_err)?;
        let script_id = scripting
            .load_script(path_str, &plugin_name)
            .await
            .map_err(dbus_err)?;
        if script_id < 0 {
            return Err(WdoError::Backend {
                backend: NAME,
                source: format!("loadScript returned {script_id}").into(),
            });
        }

        // Plasma 6 does not auto-run loaded scripts; we have to call
        // `run()` on the per-script object at /Scripting/Script{id}.
        let script_path = format!("/Scripting/Script{script_id}");
        let script_proxy = KwinScriptProxy::builder(&self.conn)
            .path(script_path.clone())
            .map_err(dbus_err)?
            .build()
            .await
            .map_err(dbus_err)?;
        script_proxy.run().await.map_err(dbus_err)?;

        Ok(KwinScriptHandle {
            plugin_name,
            _path: path,
        })
    }

    /// Best-effort cleanup: tell KWin to drop the script and let the
    /// temp file get unlinked. Errors are logged and swallowed because
    /// a stale leftover is harmless (next call uses a fresh plugin name
    /// and a fresh temp file).
    async fn cleanup_script(&self, handle: KwinScriptHandle) {
        if let Ok(scripting) = KwinScriptingProxy::new(&self.conn).await {
            if let Err(e) = scripting.unload_script(&handle.plugin_name).await {
                warn!("kde: unloadScript({}) failed: {e}", handle.plugin_name);
            }
        }
        // `handle._path` drops here, which removes the temp file.
    }

    async fn list_windows_impl(&self) -> Result<Vec<WindowInfo>> {
        let req_id = self.next_request_id();
        let (tx, rx) = oneshot::channel::<String>();
        self.pending.lock().await.list_waiters.insert(req_id, tx);

        let script = list_windows_script(req_id, &self.bridge_addr);
        let handle = self.run_kwin_script(req_id, &script).await?;

        let wait = tokio::time::timeout(Duration::from_secs(3), rx).await;
        self.cleanup_script(handle).await;

        let json = wait
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: "timed out waiting for KWin script callback".into(),
            })?
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: "KWin script aborted before callback".into(),
            })?;

        let parsed: Vec<ScriptWindow> =
            serde_json::from_str(&json).map_err(|e| WdoError::Backend {
                backend: NAME,
                source: format!("invalid windows payload: {e}").into(),
            })?;
        Ok(parsed.into_iter().map(Into::into).collect())
    }

    async fn active_window_impl(&self) -> Result<Option<WindowInfo>> {
        let req_id = self.next_request_id();
        let (tx, rx) = oneshot::channel::<String>();
        self.pending.lock().await.active_waiters.insert(req_id, tx);

        let script = active_window_script(req_id, &self.bridge_addr);
        let handle = self.run_kwin_script(req_id, &script).await?;

        let wait = tokio::time::timeout(Duration::from_secs(3), rx).await;
        self.cleanup_script(handle).await;

        let json = wait
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: "timed out waiting for KWin script callback".into(),
            })?
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: "KWin script aborted before callback".into(),
            })?;

        if json.is_empty() || json == "null" {
            return Ok(None);
        }
        let parsed: ScriptWindow = serde_json::from_str(&json).map_err(|e| WdoError::Backend {
            backend: NAME,
            source: format!("invalid active-window payload: {e}").into(),
        })?;
        Ok(Some(parsed.into()))
    }

    async fn window_geometry_impl(&self, target_id: &str) -> Result<Option<WindowGeometry>> {
        let req_id = self.next_request_id();
        let (tx, rx) = oneshot::channel::<Option<WindowGeometry>>();
        self.pending
            .lock()
            .await
            .geometry_waiters
            .insert(req_id, tx);

        let script = window_geometry_script(req_id, target_id, &self.bridge_addr);
        let handle = self.run_kwin_script(req_id, &script).await?;

        let wait = tokio::time::timeout(Duration::from_secs(3), rx).await;
        self.cleanup_script(handle).await;

        let result = wait
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: "timed out waiting for KWin geometry callback".into(),
            })?
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: "KWin geometry script aborted before callback".into(),
            })?;
        // The script returned `found=false` means no window with that
        // id exists. KDE supports geometry reads, so that's a
        // not-found error, not an unsupported-backend signal. Reserve
        // `Ok(None)` for the (extremely rare) "window exists but its
        // frameGeometry was unreadable" case the script also covers.
        match result {
            Some(geom) => Ok(Some(geom)),
            None => Err(WdoError::WindowNotFound(target_id.to_string())),
        }
    }

    async fn pointer_position_impl(&self) -> Result<Option<(i32, i32)>> {
        let req_id = self.next_request_id();
        let (tx, rx) = oneshot::channel::<Option<(i32, i32)>>();
        self.pending.lock().await.pointer_waiters.insert(req_id, tx);

        let script = pointer_position_script(req_id, &self.bridge_addr);
        let handle = self.run_kwin_script(req_id, &script).await?;

        let wait = tokio::time::timeout(Duration::from_secs(3), rx).await;
        self.cleanup_script(handle).await;

        let result = wait
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: "timed out waiting for KWin pointer callback".into(),
            })?
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: "KWin pointer script aborted before callback".into(),
            })?;
        Ok(result)
    }

    /// Register a oneshot waiter, ask the caller to build the matching script
    /// (so the request id embedded in the JS lines up with the waiter key),
    /// run it, and return the script's boolean result. Callers map `false`
    /// onto `WdoError::WindowNotFound` with their own context.
    async fn call_action(
        &self,
        what: &'static str,
        build_script: impl FnOnce(i32) -> String,
    ) -> Result<bool> {
        let req_id = self.next_request_id();
        let (tx, rx) = oneshot::channel::<bool>();
        self.pending.lock().await.action_waiters.insert(req_id, tx);

        let script = build_script(req_id);
        let handle = self.run_kwin_script(req_id, &script).await?;

        let wait = tokio::time::timeout(Duration::from_secs(3), rx).await;
        self.cleanup_script(handle).await;

        let ok = wait
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: format!("timed out waiting for KWin {what} callback").into(),
            })?
            .unwrap_or(false);
        Ok(ok)
    }
}

fn dbus_err(e: zbus::Error) -> WdoError {
    WdoError::Backend {
        backend: NAME,
        source: Box::new(e),
    }
}

fn io_err(e: std::io::Error) -> WdoError {
    WdoError::Backend {
        backend: NAME,
        source: Box::new(e),
    }
}

// ---- JS script generators --------------------------------------------------

fn list_windows_script(req_id: i32, bridge_addr: &str) -> String {
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
        service = bridge_addr,
        path = BRIDGE_PATH,
        iface = BRIDGE_IFACE,
        id = req_id
    )
}

fn active_window_script(req_id: i32, bridge_addr: &str) -> String {
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
        service = bridge_addr,
        path = BRIDGE_PATH,
        iface = BRIDGE_IFACE,
        id = req_id
    )
}

fn window_geometry_script(req_id: i32, target_id: &str, bridge_addr: &str) -> String {
    // window.frameGeometry is a QRect with x, y, width, height. We
    // walk the same window list that list_windows uses, find the one
    // matching the requested id, and report its geometry. If no
    // window matches, found=false and the caller maps to
    // WindowNotFound.
    format!(
        r#"
(function() {{
  var target = {target};
  var list = (typeof workspace.windowList === "function")
    ? workspace.windowList()
    : workspace.clientList();
  var found = false;
  var x = 0, y = 0, w = 0, h = 0;
  for (var i = 0; i < list.length; i++) {{
    var win = list[i];
    var id = (win.internalId || win.windowId || i).toString();
    if (id === target) {{
      var g = win.frameGeometry;
      if (g) {{
        x = (g.x | 0);
        y = (g.y | 0);
        w = (g.width | 0);
        h = (g.height | 0);
        found = true;
      }}
      break;
    }}
  }}
  callDBus(
    "{service}", "{path}", "{iface}", "ReportGeometry",
    {id}, found, x, y, w, h
  );
}})();
"#,
        service = bridge_addr,
        path = BRIDGE_PATH,
        iface = BRIDGE_IFACE,
        id = req_id,
        target = js_string_literal(target_id)
    )
}

fn pointer_position_script(req_id: i32, bridge_addr: &str) -> String {
    // workspace.cursorPos returns a QPoint with .x and .y in compositor
    // coordinates. Coerce to plain numbers so `callDBus` sends them as
    // i32, matching the Bridge::report_pointer signature.
    format!(
        r#"
(function() {{
  var p = workspace.cursorPos;
  var ok = (p && typeof p.x === "number" && typeof p.y === "number");
  callDBus(
    "{service}", "{path}", "{iface}", "ReportPointer",
    {id}, ok, ok ? (p.x | 0) : 0, ok ? (p.y | 0) : 0
  );
}})();
"#,
        service = bridge_addr,
        path = BRIDGE_PATH,
        iface = BRIDGE_IFACE,
        id = req_id
    )
}

/// JSON-encode a string as a JS literal. `{:?}` would ~work for ASCII ids but
/// diverges from JS escape syntax on exotic codepoints; JSON strings are a
/// subset of JS string literals, so this is always safe to paste in.
fn js_string_literal(s: &str) -> String {
    serde_json::to_string(s).expect("serde_json cannot fail on &str")
}

fn activate_window_script(req_id: i32, target_id: &str, bridge_addr: &str) -> String {
    format!(
        r#"
(function() {{
  var target = {target};
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
        service = bridge_addr,
        path = BRIDGE_PATH,
        iface = BRIDGE_IFACE,
        id = req_id,
        target = js_string_literal(target_id)
    )
}

fn close_window_script(req_id: i32, target_id: &str, bridge_addr: &str) -> String {
    format!(
        r#"
(function() {{
  var target = {target};
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
        service = bridge_addr,
        path = BRIDGE_PATH,
        iface = BRIDGE_IFACE,
        id = req_id,
        target = js_string_literal(target_id)
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
        caps.pointer_position = true;
        caps.window_geometry = true;
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

    async fn pointer_position(&self) -> Result<Option<(i32, i32)>> {
        self.pointer_position_impl().await
    }

    async fn window_geometry(&self, id: &WindowId) -> Result<Option<WindowGeometry>> {
        self.window_geometry_impl(&id.0).await
    }

    async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        self.list_windows_impl().await
    }

    async fn active_window(&self) -> Result<Option<WindowInfo>> {
        self.active_window_impl().await
    }

    async fn activate_window(&self, id: &WindowId) -> Result<()> {
        let ok = self
            .call_action("activate", |req_id| {
                activate_window_script(req_id, &id.0, &self.bridge_addr)
            })
            .await?;
        if !ok {
            return Err(WdoError::WindowNotFound(id.0.clone()));
        }
        Ok(())
    }

    async fn close_window(&self, id: &WindowId) -> Result<()> {
        let ok = self
            .call_action("close", |req_id| {
                close_window_script(req_id, &id.0, &self.bridge_addr)
            })
            .await?;
        if !ok {
            return Err(WdoError::WindowNotFound(id.0.clone()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_and_active_scripts_embed_request_id_and_bridge_addr() {
        let addr = ":1.42";
        let s = list_windows_script(42, addr);
        assert!(s.contains("\"ReportWindows\""));
        assert!(s.contains(addr));
        assert!(s.contains(BRIDGE_PATH));
        assert!(s.contains(BRIDGE_IFACE));
        assert!(s.contains("42"));
        // Uses windowList() / clientList() for Plasma 6 / 5 compatibility.
        assert!(s.contains("windowList"));
        assert!(s.contains("clientList"));

        let s = active_window_script(7, addr);
        assert!(s.contains("\"ReportActive\""));
        assert!(s.contains("activeWindow"));
        assert!(s.contains("activeClient"));
        assert!(s.contains(" 7,"));
    }

    #[test]
    fn action_scripts_json_encode_the_target_id() {
        // A target id with JS-meaningful chars (quotes, backslashes, a newline)
        // must come out as a valid JS string literal — not a Rust Debug repr.
        let weird = "ab\"c\\d\ne";
        let addr = ":1.99";
        let s = activate_window_script(1, weird, addr);
        // JSON encodes " → \" and \ → \\ and \n → \n, and wraps in double quotes.
        assert!(s.contains(r#"var target = "ab\"c\\d\ne";"#));
        assert!(s.contains("ReportAction"));
        assert!(s.contains("workspace.activeWindow = w"));

        let s = close_window_script(1, weird, addr);
        assert!(s.contains(r#"var target = "ab\"c\\d\ne";"#));
        // Plasma 6 uses closeWindow, Plasma 5 uses close — both dispatched.
        assert!(s.contains("closeWindow"));
        assert!(s.contains("w.close()"));
    }

    #[test]
    fn request_id_renders_as_bare_number_for_int32_wire_signature() {
        // KWin's JS `callDBus` serializes plain numeric literals as
        // int32 on the bus, and the bridge methods advertise an int32
        // signature to match. If a script generator ever wrapped the
        // id in quotes (sending it as a string), zbus would silently
        // drop the call and every window op would just time out.
        let s = list_windows_script(99, ":1.42");
        // Three trailing-comma positions where the id appears as a literal.
        assert!(s.contains("\"ReportWindows\",\n    99,"));
        // No accidental "99" anywhere.
        assert!(!s.contains("\"99\""));
    }

    #[test]
    fn js_string_literal_is_valid_json() {
        // The output is always a quoted JSON string, parseable back to the input.
        for s in ["", "plain", "with \"quotes\"", "unicode ☃ €", "\0\n\t\r"] {
            let encoded = js_string_literal(s);
            assert!(encoded.starts_with('"') && encoded.ends_with('"'));
            let round_trip: String = serde_json::from_str(&encoded).unwrap();
            assert_eq!(round_trip, s);
        }
    }
}
