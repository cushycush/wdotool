//! wlroots backend — input via `zwp_virtual_keyboard_v1` +
//! `zwlr_virtual_pointer_v1`, windows via `zwlr_foreign_toplevel_management_v1`.
//!
//! All wayland work happens on a single dedicated OS thread (EventQueue is
//! !Send). The main thread talks to it over a sync command channel with
//! per-command tokio oneshots for replies.

use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rustix::fs::{MemfdFlags, SealFlags};
use tokio::sync::oneshot;
use tracing::{debug, trace, warn};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_output, wl_registry, wl_seat};
use wayland_client::{
    delegate_noop, event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle,
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1 as vk_mgr, zwp_virtual_keyboard_v1 as vk,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1 as ft_handle, zwlr_foreign_toplevel_manager_v1 as ft_mgr,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1 as vp_mgr, zwlr_virtual_pointer_v1 as vp,
};
use xkbcommon::xkb;

use super::Backend;
use crate::error::{Result, WdoError};
use crate::types::{Capabilities, KeyDirection, MouseButton, WindowId, WindowInfo};

const NAME: &str = "wlroots";

// ---- Public backend handle --------------------------------------------------

pub struct WlrootsBackend {
    tx: mpsc::Sender<Command>,
    caps: Arc<Mutex<Capabilities>>,
    // Drop this to tell the worker to shut down (not strictly needed for CLI
    // lifetimes, but keeps the thread tidy in tests and long-running use).
    _shutdown: Arc<ShutdownGuard>,
}

struct ShutdownGuard {
    tx: mpsc::Sender<Command>,
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

impl WlrootsBackend {
    pub async fn try_new() -> Result<Self> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
        let (ready_tx, ready_rx) = oneshot::channel::<std::result::Result<Capabilities, String>>();

        let tx_for_thread = cmd_tx.clone();
        std::thread::Builder::new()
            .name("wdotool-wlr".into())
            .spawn(move || {
                worker_main(cmd_rx, tx_for_thread, ready_tx);
            })
            .map_err(|e| WdoError::Backend {
                backend: NAME,
                source: anyhow::Error::new(e),
            })?;

        let caps_initial = ready_rx
            .await
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("worker thread exited before reporting ready"),
            })?
            .map_err(|msg| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!(msg),
            })?;

        let caps = Arc::new(Mutex::new(caps_initial));
        Ok(Self {
            tx: cmd_tx.clone(),
            caps,
            _shutdown: Arc::new(ShutdownGuard { tx: cmd_tx }),
        })
    }

    async fn send<T: Send + 'static>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T>>) -> Command,
    ) -> Result<T> {
        let (tx, rx) = oneshot::channel::<Result<T>>();
        self.tx.send(make(tx)).map_err(|_| WdoError::Backend {
            backend: NAME,
            source: anyhow::anyhow!("worker thread is gone"),
        })?;
        rx.await.map_err(|_| WdoError::Backend {
            backend: NAME,
            source: anyhow::anyhow!("worker dropped reply channel"),
        })?
    }
}

// ---- Commands & replies -----------------------------------------------------

enum Command {
    Key {
        keysym: String,
        dir: KeyDirection,
        reply: oneshot::Sender<Result<()>>,
    },
    TypeText {
        text: String,
        delay: Duration,
        reply: oneshot::Sender<Result<()>>,
    },
    MouseMove {
        x: i32,
        y: i32,
        absolute: bool,
        reply: oneshot::Sender<Result<()>>,
    },
    MouseButton {
        btn: MouseButton,
        dir: KeyDirection,
        reply: oneshot::Sender<Result<()>>,
    },
    Scroll {
        dx: f64,
        dy: f64,
        reply: oneshot::Sender<Result<()>>,
    },
    ListWindows {
        reply: oneshot::Sender<Result<Vec<WindowInfo>>>,
    },
    ActiveWindow {
        reply: oneshot::Sender<Result<Option<WindowInfo>>>,
    },
    ActivateWindow {
        id: String,
        reply: oneshot::Sender<Result<()>>,
    },
    CloseWindow {
        id: String,
        reply: oneshot::Sender<Result<()>>,
    },
    Shutdown,
}

// ---- Worker state -----------------------------------------------------------

struct ToplevelInfo {
    title: String,
    app_id: Option<String>,
    activated: bool,
    closed: bool,
    handle: ft_handle::ZwlrForeignToplevelHandleV1,
}

#[derive(Default)]
struct GlobalsScratch {
    seat: Option<wl_seat::WlSeat>,
    vk_mgr: Option<vk_mgr::ZwpVirtualKeyboardManagerV1>,
    vp_mgr: Option<vp_mgr::ZwlrVirtualPointerManagerV1>,
    ft_mgr: Option<ft_mgr::ZwlrForeignToplevelManagerV1>,
}

struct State {
    scratch: GlobalsScratch,
    toplevels: HashMap<ObjectId, ToplevelInfo>,
    // buffer while we're still collecting info for a handle that hasn't
    // emitted `done` yet; keyed by the handle's object id
    pending: HashMap<ObjectId, PendingToplevel>,
    // Outputs discovered on the registry. `motion_absolute` needs a pixel
    // extent to make coords meaningful; we use the first output with a
    // known current mode. Multi-output setups pick whichever output came
    // back first — documented as a known limitation.
    outputs: HashMap<ObjectId, OutputInfo>,
}

#[derive(Default)]
struct PendingToplevel {
    title: Option<String>,
    app_id: Option<String>,
    activated: bool,
}

#[derive(Default, Clone, Copy)]
struct OutputInfo {
    width: u32,
    height: u32,
}

impl State {
    /// Returns (x_extent, y_extent) for `motion_absolute` based on the first
    /// output that has a known mode, or None if no output has reported yet.
    fn primary_extent(&self) -> Option<(u32, u32)> {
        self.outputs
            .values()
            .find(|o| o.width > 0 && o.height > 0)
            .map(|o| (o.width, o.height))
    }
}

// ---- Worker entry point -----------------------------------------------------

fn worker_main(
    rx: mpsc::Receiver<Command>,
    _self_tx: mpsc::Sender<Command>,
    ready_tx: oneshot::Sender<std::result::Result<Capabilities, String>>,
) {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(err) => {
            let _ = ready_tx.send(Err(format!("wayland connect: {err}")));
            return;
        }
    };
    let display = conn.display();
    let mut queue: EventQueue<State> = conn.new_event_queue();
    let qh = queue.handle();

    let mut state = State {
        scratch: GlobalsScratch::default(),
        toplevels: HashMap::new(),
        pending: HashMap::new(),
        outputs: HashMap::new(),
    };
    let _ = display.get_registry(&qh, ());

    // First roundtrip: populate globals list.
    if let Err(err) = queue.roundtrip(&mut state) {
        let _ = ready_tx.send(Err(format!("initial registry roundtrip: {err}")));
        return;
    }

    // Per-seat objects: create virtual keyboard + pointer if managers exist.
    let seat = state.scratch.seat.clone();
    let vk_obj = match (&state.scratch.vk_mgr, &seat) {
        (Some(mgr), Some(seat)) => Some(mgr.create_virtual_keyboard(seat, &qh, ())),
        _ => None,
    };

    // Load & install a keymap from the environment's xkb defaults so the
    // compositor can translate our keycodes. This is a best-effort default;
    // users who need a different layout get it via XKB_DEFAULT_* env vars.
    let keymap = match compile_keymap() {
        Ok(k) => Some(k),
        Err(err) => {
            warn!(
                ?err,
                "failed to compile default xkb keymap; key input disabled"
            );
            None
        }
    };
    if let (Some(vk), Some(keymap)) = (&vk_obj, keymap.as_ref()) {
        if let Err(err) = install_keymap(vk, keymap) {
            warn!(?err, "failed to upload keymap to virtual keyboard");
        }
    }

    let vp_obj = match (&state.scratch.vp_mgr, &seat) {
        (Some(mgr), Some(seat)) => Some(mgr.create_virtual_pointer(Some(seat), &qh, ())),
        _ => None,
    };

    // Foreign-toplevel manager drives its own callback stream; just hold it.
    let _ft = state.scratch.ft_mgr.clone();

    // Second roundtrip: flush creation requests + pull initial toplevel list.
    if let Err(err) = queue.roundtrip(&mut state) {
        let _ = ready_tx.send(Err(format!("post-create roundtrip: {err}")));
        return;
    }

    let caps = Capabilities {
        key_input: vk_obj.is_some() && keymap.is_some(),
        // text_input works via transient keymap injection as long as the
        // virtual keyboard itself is usable; no dependency on the default
        // keymap because we swap our own in at type time.
        text_input: vk_obj.is_some(),
        pointer_move_absolute: vp_obj.is_some(),
        pointer_move_relative: vp_obj.is_some(),
        pointer_button: vp_obj.is_some(),
        scroll: vp_obj.is_some(),
        list_windows: state.scratch.ft_mgr.is_some(),
        active_window: state.scratch.ft_mgr.is_some(),
        activate_window: state.scratch.ft_mgr.is_some() && seat.is_some(),
        close_window: state.scratch.ft_mgr.is_some(),
    };
    if ready_tx.send(Ok(caps)).is_err() {
        return;
    }

    // Command loop. Each command is followed by a short dispatch to drain
    // events that arrived in response (e.g., foreign-toplevel updates).
    loop {
        let cmd = match rx.recv() {
            Ok(c) => c,
            Err(_) => break,
        };
        match cmd {
            Command::Shutdown => break,
            Command::Key { keysym, dir, reply } => {
                let res = do_key(&conn, &vk_obj, keymap.as_ref(), &keysym, dir);
                let _ = reply.send(res);
            }
            Command::TypeText { text, delay, reply } => {
                let res = do_type_text(&conn, &vk_obj, keymap.as_ref(), &text, delay);
                let _ = reply.send(res);
            }
            Command::MouseMove {
                x,
                y,
                absolute,
                reply,
            } => {
                // Ensure output modes have arrived before we read them. A
                // dispatch_pending is cheap — no server roundtrip — and
                // covers the common case where Mode events arrived during
                // the initial handshake but more may be queued.
                let _ = queue.dispatch_pending(&mut state);
                let extent = state.primary_extent();
                let res = do_mouse_move(&conn, &vp_obj, x, y, absolute, extent);
                let _ = reply.send(res);
            }
            Command::MouseButton { btn, dir, reply } => {
                let res = do_mouse_button(&conn, &vp_obj, btn, dir);
                let _ = reply.send(res);
            }
            Command::Scroll { dx, dy, reply } => {
                let res = do_scroll(&conn, &vp_obj, dx, dy);
                let _ = reply.send(res);
            }
            Command::ListWindows { reply } => {
                // Toplevel info arrives asynchronously; a roundtrip ensures we
                // have the latest state before snapshotting.
                let _ = queue.roundtrip(&mut state);
                let _ = reply.send(Ok(state
                    .toplevels
                    .values()
                    .filter(|t| !t.closed)
                    .map(to_window_info)
                    .collect()));
            }
            Command::ActiveWindow { reply } => {
                let _ = queue.roundtrip(&mut state);
                let active = state
                    .toplevels
                    .values()
                    .find(|t| t.activated && !t.closed)
                    .map(to_window_info);
                let _ = reply.send(Ok(active));
            }
            Command::ActivateWindow { id, reply } => {
                let _ = queue.roundtrip(&mut state);
                let res = match (find_handle_by_id(&state, &id), &seat) {
                    (Some(handle), Some(seat)) => {
                        handle.activate(seat);
                        let _ = conn.flush();
                        Ok(())
                    }
                    (None, _) => Err(WdoError::WindowNotFound(id)),
                    (_, None) => Err(WdoError::NotSupported {
                        backend: NAME,
                        what: "no wl_seat bound; cannot activate window",
                    }),
                };
                let _ = reply.send(res);
            }
            Command::CloseWindow { id, reply } => {
                let _ = queue.roundtrip(&mut state);
                let res = match find_handle_by_id(&state, &id) {
                    Some(handle) => {
                        handle.close();
                        let _ = conn.flush();
                        Ok(())
                    }
                    None => Err(WdoError::WindowNotFound(id)),
                };
                let _ = reply.send(res);
            }
        }
    }
    debug!("wlroots worker exiting");
}

fn to_window_info(t: &ToplevelInfo) -> WindowInfo {
    WindowInfo {
        id: WindowId(t.handle.id().to_string()),
        title: t.title.clone(),
        app_id: t.app_id.clone(),
        pid: None,
    }
}

fn find_handle_by_id<'a>(
    state: &'a State,
    id: &str,
) -> Option<&'a ft_handle::ZwlrForeignToplevelHandleV1> {
    state
        .toplevels
        .values()
        .find(|t| t.handle.id().to_string() == id)
        .map(|t| &t.handle)
}

// ---- Input op implementations ----------------------------------------------

fn do_key(
    conn: &Connection,
    vk_obj: &Option<vk::ZwpVirtualKeyboardV1>,
    keymap: Option<&SafeKeymap>,
    keysym: &str,
    dir: KeyDirection,
) -> Result<()> {
    let vk = vk_obj.as_ref().ok_or(WdoError::NotSupported {
        backend: NAME,
        what: "no zwp_virtual_keyboard_v1 bound",
    })?;
    let keymap = keymap.ok_or(WdoError::NotSupported {
        backend: NAME,
        what: "no keymap compiled",
    })?;
    let (keycode, needs_shift) =
        resolve_keycode(&keymap.0, keysym).ok_or_else(|| WdoError::Keysym {
            input: keysym.into(),
            reason: format!("keysym '{keysym}' not found in active keymap"),
        })?;
    let shift_kc = resolve_keycode(&keymap.0, "Shift_L").map(|(kc, _)| kc);

    let time = millis_monotonic();
    match dir {
        KeyDirection::Press => {
            if needs_shift {
                if let Some(kc) = shift_kc {
                    vk.key(time, kc, 1);
                }
            }
            vk.key(time, keycode, 1);
        }
        KeyDirection::Release => {
            vk.key(time, keycode, 0);
            if needs_shift {
                if let Some(kc) = shift_kc {
                    vk.key(time, kc, 0);
                }
            }
        }
        KeyDirection::PressRelease => {
            if needs_shift {
                if let Some(kc) = shift_kc {
                    vk.key(time, kc, 1);
                }
            }
            vk.key(time, keycode, 1);
            vk.key(time, keycode, 0);
            if needs_shift {
                if let Some(kc) = shift_kc {
                    vk.key(time, kc, 0);
                }
            }
        }
    }
    conn.flush().map_err(wayland_io_err)?;
    Ok(())
}

fn do_type_text(
    conn: &Connection,
    vk_obj: &Option<vk::ZwpVirtualKeyboardV1>,
    default_keymap: Option<&SafeKeymap>,
    text: &str,
    delay: Duration,
) -> Result<()> {
    let vk = vk_obj.as_ref().ok_or(WdoError::NotSupported {
        backend: NAME,
        what: "no zwp_virtual_keyboard_v1 bound",
    })?;

    // Collect unique chars in order of first appearance so the keycode table
    // stays compact and reproducible.
    let mut unique: Vec<char> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in text.chars() {
        if seen.insert(c) {
            unique.push(c);
        }
    }
    if unique.is_empty() {
        return Ok(());
    }

    let (keymap_text, char_to_code) = build_text_keymap(&unique);
    upload_keymap_text(vk, &keymap_text)?;
    conn.flush().map_err(wayland_io_err)?;

    // The compositor needs a moment to parse the new keymap before our
    // first key event arrives. A full roundtrip would be ideal but we
    // don't have the event queue here; a short sleep matches what wtype
    // does and works in practice on Hyprland/Sway/KDE.
    std::thread::sleep(Duration::from_millis(20));

    for (idx, c) in text.chars().enumerate() {
        let kc = *char_to_code.get(&c).expect("char mapped above");
        let t = millis_monotonic();
        vk.key(t, kc, 1);
        vk.key(t, kc, 0);
        conn.flush().map_err(wayland_io_err)?;
        // Skip the tail-sleep on the last char so total latency stays tight.
        if !delay.is_zero() && idx + 1 < text.chars().count() {
            std::thread::sleep(delay);
        }
    }

    // Restore the default keymap so a subsequent `key` command in the same
    // process still resolves against normal symbols. For single-shot CLI
    // use this is cosmetic, but it's cheap and keeps state coherent.
    if let Some(km) = default_keymap {
        let _ = install_keymap(vk, km);
        let _ = conn.flush();
    }
    Ok(())
}

fn do_mouse_move(
    conn: &Connection,
    vp_obj: &Option<vp::ZwlrVirtualPointerV1>,
    x: i32,
    y: i32,
    absolute: bool,
    extent: Option<(u32, u32)>,
) -> Result<()> {
    let vp = vp_obj.as_ref().ok_or(WdoError::NotSupported {
        backend: NAME,
        what: "no zwlr_virtual_pointer_v1 bound",
    })?;
    let time = millis_monotonic();
    if absolute {
        // Prefer the real primary-output pixel size. Fall back to a 10k square
        // if the compositor hasn't reported an output mode yet — in that case
        // callers get scaled coords they can sanity-check against.
        let (x_extent, y_extent) = extent.unwrap_or((10_000, 10_000));
        let x = x.clamp(0, x_extent as i32) as u32;
        let y = y.clamp(0, y_extent as i32) as u32;
        vp.motion_absolute(time, x, y, x_extent, y_extent);
    } else {
        vp.motion(time, x as f64, y as f64);
    }
    vp.frame();
    conn.flush().map_err(wayland_io_err)?;
    Ok(())
}

fn do_mouse_button(
    conn: &Connection,
    vp_obj: &Option<vp::ZwlrVirtualPointerV1>,
    btn: MouseButton,
    dir: KeyDirection,
) -> Result<()> {
    let vp = vp_obj.as_ref().ok_or(WdoError::NotSupported {
        backend: NAME,
        what: "no zwlr_virtual_pointer_v1 bound",
    })?;
    let code = match btn {
        MouseButton::Left => 0x110,
        MouseButton::Right => 0x111,
        MouseButton::Middle => 0x112,
        MouseButton::Back => 0x113,
        MouseButton::Forward => 0x114,
        MouseButton::Other(_) => {
            return Err(WdoError::InvalidArg(format!(
                "unsupported mouse button: {btn:?}"
            )));
        }
    };
    let time = millis_monotonic();
    match dir {
        KeyDirection::Press => {
            vp.button(
                time,
                code,
                wayland_client::protocol::wl_pointer::ButtonState::Pressed,
            );
        }
        KeyDirection::Release => {
            vp.button(
                time,
                code,
                wayland_client::protocol::wl_pointer::ButtonState::Released,
            );
        }
        KeyDirection::PressRelease => {
            vp.button(
                time,
                code,
                wayland_client::protocol::wl_pointer::ButtonState::Pressed,
            );
            vp.button(
                time,
                code,
                wayland_client::protocol::wl_pointer::ButtonState::Released,
            );
        }
    }
    vp.frame();
    conn.flush().map_err(wayland_io_err)?;
    Ok(())
}

fn do_scroll(
    conn: &Connection,
    vp_obj: &Option<vp::ZwlrVirtualPointerV1>,
    dx: f64,
    dy: f64,
) -> Result<()> {
    let vp = vp_obj.as_ref().ok_or(WdoError::NotSupported {
        backend: NAME,
        what: "no zwlr_virtual_pointer_v1 bound",
    })?;
    let time = millis_monotonic();
    if dx != 0.0 {
        vp.axis(
            time,
            wayland_client::protocol::wl_pointer::Axis::HorizontalScroll,
            dx,
        );
    }
    if dy != 0.0 {
        vp.axis(
            time,
            wayland_client::protocol::wl_pointer::Axis::VerticalScroll,
            dy,
        );
    }
    vp.frame();
    conn.flush().map_err(wayland_io_err)?;
    Ok(())
}

fn wayland_io_err<E: std::fmt::Display>(e: E) -> WdoError {
    WdoError::Backend {
        backend: NAME,
        source: anyhow::anyhow!("wayland I/O: {e}"),
    }
}

fn millis_monotonic() -> u32 {
    // Protocol uses a 32-bit millisecond timestamp. Use wrapping truncation —
    // compositors only care about deltas & ordering.
    use std::time::Instant;
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_millis() as u32
}

// ---- xkb helpers ------------------------------------------------------------

struct SafeKeymap(xkb::Keymap);
unsafe impl Send for SafeKeymap {}
unsafe impl Sync for SafeKeymap {}

fn compile_keymap() -> Result<SafeKeymap> {
    let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    // Empty RMLVO → falls back to XKB_DEFAULT_* env vars, then libxkbcommon
    // compiled-in defaults (pc105 + us).
    let keymap =
        xkb::Keymap::new_from_names(&ctx, "", "", "", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
            .ok_or_else(|| WdoError::Backend {
                backend: NAME,
                source: anyhow::anyhow!("xkb_keymap_new_from_names returned null"),
            })?;
    Ok(SafeKeymap(keymap))
}

fn install_keymap(vk_obj: &vk::ZwpVirtualKeyboardV1, keymap: &SafeKeymap) -> Result<()> {
    let as_string = keymap.0.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    upload_keymap_text(vk_obj, &as_string)
}

/// Generate an xkb_keymap text where each unique input character gets its own
/// keycode at level 0 (no modifiers needed). Matches the trick wtype uses.
///
/// Returns the keymap text plus a map from char to the evdev-style keycode the
/// virtual keyboard should emit to produce that character.
fn build_text_keymap(chars: &[char]) -> (String, std::collections::HashMap<char, u32>) {
    use std::fmt::Write as _;

    let mut s = String::new();
    // xkb requires minimum >= 8; keycode N in xkb terms = evdev N - 8.
    let min_xkb: u32 = 8;
    // Pad max by a few so compositors that reject tight ranges are happy.
    let max_xkb: u32 = min_xkb + chars.len() as u32 + 7;

    writeln!(s, "xkb_keymap {{").unwrap();
    writeln!(s, "  xkb_keycodes \"wdotool\" {{").unwrap();
    writeln!(s, "    minimum = {};", min_xkb).unwrap();
    writeln!(s, "    maximum = {};", max_xkb).unwrap();
    for i in 0..chars.len() {
        writeln!(s, "    <K{i}> = {};", min_xkb + i as u32).unwrap();
    }
    writeln!(s, "  }};").unwrap();
    writeln!(s, "  xkb_types \"wdotool\" {{ include \"complete\" }};").unwrap();
    writeln!(
        s,
        "  xkb_compatibility \"wdotool\" {{ include \"complete\" }};"
    )
    .unwrap();
    writeln!(s, "  xkb_symbols \"wdotool\" {{").unwrap();
    writeln!(s, "    name[Group1] = \"wdotool\";").unwrap();
    for (i, c) in chars.iter().enumerate() {
        // `U<hex>` is xkb's canonical notation for Unicode keysyms.
        writeln!(s, "    key <K{i}> {{ [ U{:04X} ] }};", *c as u32).unwrap();
    }
    writeln!(s, "  }};").unwrap();
    writeln!(s, "}};").unwrap();

    let mut map = std::collections::HashMap::with_capacity(chars.len());
    for (i, c) in chars.iter().enumerate() {
        map.insert(*c, i as u32);
    }
    (s, map)
}

/// Write a keymap string to a sealed memfd and upload it via the virtual
/// keyboard's `keymap` request.
fn upload_keymap_text(vk_obj: &vk::ZwpVirtualKeyboardV1, keymap_text: &str) -> Result<()> {
    let bytes = keymap_text.as_bytes();
    let fd = rustix::fs::memfd_create(
        "wdotool-keymap",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .map_err(|e| WdoError::Backend {
        backend: NAME,
        source: anyhow::Error::new(e),
    })?;
    let mut file = std::fs::File::from(fd);
    file.write_all(bytes).map_err(wayland_io_err)?;
    file.write_all(&[0u8]).map_err(wayland_io_err)?;
    file.flush().ok();
    let size = bytes.len() as u32 + 1;
    let _ = rustix::fs::fcntl_add_seals(
        file.as_fd(),
        SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE,
    );
    file.seek(SeekFrom::Start(0)).ok();
    vk_obj.keymap(
        wayland_client::protocol::wl_keyboard::KeymapFormat::XkbV1 as u32,
        file.as_fd(),
        size,
    );
    Ok(())
}

fn resolve_keycode(keymap: &xkb::Keymap, name: &str) -> Option<(u32, bool)> {
    let target = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
    if target.raw() == 0 {
        return None;
    }
    for keycode in keymap.min_keycode().raw()..=keymap.max_keycode().raw() {
        for level in 0..=1 {
            let syms = keymap.key_get_syms_by_level(xkb::Keycode::new(keycode), 0, level);
            if syms.contains(&target) {
                return Some((keycode.saturating_sub(8), level == 1));
            }
        }
    }
    None
}

// ---- Wayland dispatch -------------------------------------------------------

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_seat" => {
                    let seat = registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(7), qh, ());
                    state.scratch.seat = Some(seat);
                }
                "wl_output" => {
                    let output =
                        registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                    state.outputs.insert(output.id(), OutputInfo::default());
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    let m = registry.bind::<vk_mgr::ZwpVirtualKeyboardManagerV1, _, _>(
                        name,
                        version.min(1),
                        qh,
                        (),
                    );
                    state.scratch.vk_mgr = Some(m);
                }
                "zwlr_virtual_pointer_manager_v1" => {
                    let m = registry.bind::<vp_mgr::ZwlrVirtualPointerManagerV1, _, _>(
                        name,
                        version.min(2),
                        qh,
                        (),
                    );
                    state.scratch.vp_mgr = Some(m);
                }
                "zwlr_foreign_toplevel_manager_v1" => {
                    let m = registry.bind::<ft_mgr::ZwlrForeignToplevelManagerV1, _, _>(
                        name,
                        version.min(3),
                        qh,
                        (),
                    );
                    state.scratch.ft_mgr = Some(m);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // We don't care about seat capabilities or name events for automation.
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Only the current mode is meaningful — discard non-current ones so
        // motion_absolute uses the resolution the output is actually rendering.
        if let wl_output::Event::Mode {
            flags,
            width,
            height,
            ..
        } = event
        {
            let is_current = flags
                .into_result()
                .map(|f| f.contains(wl_output::Mode::Current))
                .unwrap_or(false);
            if is_current {
                state.outputs.entry(output.id()).or_default().width = width.max(0) as u32;
                state.outputs.entry(output.id()).or_default().height = height.max(0) as u32;
            }
        }
    }
}

impl Dispatch<ft_mgr::ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ft_mgr::ZwlrForeignToplevelManagerV1,
        event: ft_mgr::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ft_mgr::Event::Toplevel { toplevel } = event {
            state
                .pending
                .insert(toplevel.id(), PendingToplevel::default());
            // Stash the handle in toplevels now so it's reachable if done()
            // never arrives (closed before initial state); fields fill in
            // via handle events.
            state.toplevels.insert(
                toplevel.id(),
                ToplevelInfo {
                    title: String::new(),
                    app_id: None,
                    activated: false,
                    closed: false,
                    handle: toplevel,
                },
            );
        }
    }

    // Opcode 0 = `toplevel` — the event that creates a new handle object.
    event_created_child!(State, ft_mgr::ZwlrForeignToplevelManagerV1, [
        0 => (ft_handle::ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ft_handle::ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ft_handle::ZwlrForeignToplevelHandleV1,
        event: ft_handle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = handle.id();
        match event {
            ft_handle::Event::Title { title } => {
                state.pending.entry(id.clone()).or_default().title = Some(title);
            }
            ft_handle::Event::AppId { app_id } => {
                state.pending.entry(id.clone()).or_default().app_id = Some(app_id);
            }
            ft_handle::Event::State { state: flags } => {
                // "activated" is the first u32 of the array, per the protocol
                // (the array holds one or more state enum values, little-endian).
                let mut activated = false;
                for chunk in flags.chunks_exact(4) {
                    let v = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    // state enum: maximized=0, minimized=1, activated=2, fullscreen=3
                    if v == 2 {
                        activated = true;
                    }
                }
                state.pending.entry(id.clone()).or_default().activated = activated;
            }
            ft_handle::Event::Done => {
                if let Some(pending) = state.pending.remove(&id) {
                    if let Some(info) = state.toplevels.get_mut(&id) {
                        if let Some(title) = pending.title {
                            info.title = title;
                        }
                        if pending.app_id.is_some() {
                            info.app_id = pending.app_id;
                        }
                        info.activated = pending.activated;
                    }
                }
            }
            ft_handle::Event::Closed => {
                if let Some(info) = state.toplevels.get_mut(&id) {
                    info.closed = true;
                }
                // Send destroy eagerly — server has told us nothing more is coming.
                handle.destroy();
                state.toplevels.remove(&id);
                state.pending.remove(&id);
            }
            _ => {
                trace!("ignoring foreign-toplevel event");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_keymap_maps_each_char_to_own_keycode() {
        let (text, map) = build_text_keymap(&['a', 'B', '!', '€']);
        // Each char gets a distinct keycode starting at 0.
        assert_eq!(map[&'a'], 0);
        assert_eq!(map[&'B'], 1);
        assert_eq!(map[&'!'], 2);
        assert_eq!(map[&'€'], 3);
        // The keymap includes a Unicode keysym for the euro sign (U+20AC).
        assert!(text.contains("U20AC"));
        assert!(text.contains("U0061")); // 'a'
        assert!(text.contains("U0042")); // 'B'
        assert!(text.contains("xkb_types \"wdotool\" { include \"complete\" }"));
    }

    #[test]
    fn text_keymap_minimum_is_eight() {
        let (text, _) = build_text_keymap(&['x']);
        assert!(text.contains("minimum = 8;"));
    }
}

// Manager + per-seat object interfaces that either emit no events or emit
// events we don't care about for automation.
delegate_noop!(State: vk_mgr::ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: vk::ZwpVirtualKeyboardV1);
delegate_noop!(State: vp_mgr::ZwlrVirtualPointerManagerV1);
delegate_noop!(State: vp::ZwlrVirtualPointerV1);

// ---- Backend trait impl -----------------------------------------------------

#[async_trait]
impl Backend for WlrootsBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn capabilities(&self) -> Capabilities {
        self.caps.lock().unwrap().clone()
    }

    async fn key(&self, keysym: &str, dir: KeyDirection) -> Result<()> {
        let keysym = keysym.to_string();
        self.send(|reply| Command::Key { keysym, dir, reply }).await
    }

    async fn type_text(&self, text: &str, delay: Duration) -> Result<()> {
        let text = text.to_string();
        self.send(|reply| Command::TypeText { text, delay, reply })
            .await
    }

    async fn mouse_move(&self, x: i32, y: i32, absolute: bool) -> Result<()> {
        self.send(|reply| Command::MouseMove {
            x,
            y,
            absolute,
            reply,
        })
        .await
    }

    async fn mouse_button(&self, btn: MouseButton, dir: KeyDirection) -> Result<()> {
        self.send(|reply| Command::MouseButton { btn, dir, reply })
            .await
    }

    async fn scroll(&self, dx: f64, dy: f64) -> Result<()> {
        self.send(|reply| Command::Scroll { dx, dy, reply }).await
    }

    async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        self.send(|reply| Command::ListWindows { reply }).await
    }

    async fn active_window(&self) -> Result<Option<WindowInfo>> {
        self.send(|reply| Command::ActiveWindow { reply }).await
    }

    async fn activate_window(&self, id: &WindowId) -> Result<()> {
        let id = id.0.clone();
        self.send(|reply| Command::ActivateWindow { id, reply })
            .await
    }

    async fn close_window(&self, id: &WindowId) -> Result<()> {
        let id = id.0.clone();
        self.send(|reply| Command::CloseWindow { id, reply }).await
    }
}
