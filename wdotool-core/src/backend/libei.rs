//! libei input backend via the XDG RemoteDesktop portal.
//!
//! The libei event stream isn't `Send` (reis stores `dyn FnOnce` callbacks in
//! its high-level converter), so a dedicated OS thread runs a single-threaded
//! tokio runtime to drive the stream. Emit methods run on the caller's thread
//! — they're synchronous through the `Connection` proxy, which is `Send + Sync`.

use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ashpd::desktop::remote_desktop::{
    ConnectToEISOptions, DeviceType, RemoteDesktop, SelectDevicesOptions, StartOptions,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode};
use async_trait::async_trait;
use enumflags2::BitFlags;
use futures_util::StreamExt;
use reis::ei;
use reis::event::{self as rev, DeviceCapability, EiEvent};
use tokio::sync::oneshot;
use tracing::{debug, info, trace, warn};
use xkbcommon::xkb;

use super::Backend;
use crate::error::{Result, WdoError};
use crate::portal_token;
use crate::types::{Capabilities, KeyDirection, MouseButton, WindowId, WindowInfo};

const NAME: &str = "libei";

pub struct LibeiBackend {
    state: Arc<Mutex<State>>,
    start: Instant,
}

// xkb_keymap is documented thread-safe for read operations; we gate access
// behind the outer State mutex anyway.
struct SafeKeymap(xkb::Keymap);
unsafe impl Send for SafeKeymap {}
unsafe impl Sync for SafeKeymap {}

struct State {
    connection: rev::Connection,
    keyboard: Option<rev::Device>,
    pointer: Option<rev::Device>,
    pointer_abs: Option<rev::Device>,
    keymap: Option<SafeKeymap>,
    sequence: u32,
}

impl LibeiBackend {
    pub async fn try_new() -> Result<Self> {
        // Portal session is async — this has to happen on the caller's tokio
        // runtime because ashpd uses zbus on the current runtime.
        let context = open_context().await?;

        // Now cross into a dedicated OS thread that runs its own current-thread
        // tokio runtime. That thread owns the (!Send) event stream. A oneshot
        // signals when the first usable device has Resumed.
        let (connection_tx, connection_rx) = std::sync::mpsc::sync_channel::<InitResult>(1);
        let (ready_tx, ready_rx) = oneshot::channel::<()>();

        std::thread::Builder::new()
            .name("wdotool-libei".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(err) => {
                        let _ = connection_tx
                            .send(Err(format!("failed to build libei runtime: {err}")));
                        return;
                    }
                };
                rt.block_on(dispatcher(context, connection_tx, ready_tx));
            })
            .map_err(|e| WdoError::Backend {
                backend: NAME,
                source: Box::new(e),
            })?;

        // Block for the initial connection (fast — no user interaction here).
        let connection = connection_rx
            .recv()
            .map_err(|_| WdoError::Backend {
                backend: NAME,
                source: "dispatcher thread exited before sending state".into(),
            })?
            .map_err(|msg: String| WdoError::Backend {
                backend: NAME,
                source: msg.into(),
            })?;

        let state = connection.state.clone();

        // Wait up to 5s for the first device to resume. No sleep loop — the
        // dispatcher fires the oneshot exactly when DeviceResumed arrives.
        match tokio::time::timeout(Duration::from_secs(5), ready_rx).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                return Err(WdoError::Backend {
                    backend: NAME,
                    source: "dispatcher ended before any device resumed".into(),
                });
            }
            Err(_) => {
                return Err(WdoError::Backend {
                    backend: NAME,
                    source: "timed out waiting for libei device.\n\
                             Common cause: the RemoteDesktop portal dialog was \
                             dismissed or denied. Try again and accept, or check \
                             your desktop's privacy / remote-desktop settings."
                        .into(),
                });
            }
        }

        Ok(Self {
            state,
            start: Instant::now(),
        })
    }

    fn timestamp_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }

    fn emit_frame<F>(&self, pick: DeviceChoice, body: F) -> Result<()>
    where
        F: FnOnce(&rev::Device, &State),
    {
        let mut st = self.state.lock().unwrap();
        let serial = st.connection.serial();
        st.sequence = st.sequence.wrapping_add(1);
        let seq = st.sequence;

        let device = match pick {
            DeviceChoice::Keyboard => st.keyboard.clone(),
            DeviceChoice::Pointer => st.pointer.clone(),
            DeviceChoice::PointerAbsolute => st.pointer_abs.clone(),
        };

        let Some(device) = device else {
            return Err(WdoError::NotSupported {
                backend: NAME,
                what: pick.missing_error(),
            });
        };

        device.device().start_emulating(serial, seq);
        body(&device, &st);
        device.device().frame(serial, self.timestamp_us());
        device.device().stop_emulating(serial);
        st.connection.flush().map_err(|e| WdoError::Backend {
            backend: NAME,
            source: format!("ei flush failed: {e}").into(),
        })?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum DeviceChoice {
    Keyboard,
    Pointer,
    PointerAbsolute,
}

impl DeviceChoice {
    fn missing_error(self) -> &'static str {
        match self {
            Self::Keyboard => "no keyboard device from EIS",
            Self::Pointer => "no relative-pointer device from EIS",
            Self::PointerAbsolute => "no absolute-pointer device from EIS",
        }
    }
}

struct InitOk {
    state: Arc<Mutex<State>>,
}

type InitResult = std::result::Result<InitOk, String>;

/// Runs on the dedicated OS thread with a current-thread tokio runtime. Does
/// the handshake, shares state with the main thread via `Arc<Mutex>`, and
/// keeps draining events so pings get answered.
async fn dispatcher(
    context: ei::Context,
    init_tx: std::sync::mpsc::SyncSender<InitResult>,
    ready_tx: oneshot::Sender<()>,
) {
    let (connection, mut stream) = match context
        .handshake_tokio("wdotool", ei::handshake::ContextType::Sender)
        .await
    {
        Ok(pair) => pair,
        Err(err) => {
            let _ = init_tx.send(Err(format!("ei handshake failed: {err}")));
            return;
        }
    };
    let _ = connection.flush();

    let state = Arc::new(Mutex::new(State {
        connection,
        keyboard: None,
        pointer: None,
        pointer_abs: None,
        keymap: None,
        sequence: 0,
    }));

    if init_tx
        .send(Ok(InitOk {
            state: state.clone(),
        }))
        .is_err()
    {
        return;
    }
    drop(init_tx);

    let mut ready_tx = Some(ready_tx);
    while let Some(result) = stream.next().await {
        let event = match result {
            Ok(ev) => ev,
            Err(err) => {
                warn!(?err, "libei stream error, ending dispatch");
                break;
            }
        };
        let mut st = state.lock().unwrap();
        if handle_event(&mut st, event) {
            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(());
            }
        }
    }
    debug!("libei event stream ended");
}

async fn open_context() -> Result<ei::Context> {
    if let Ok(Some(context)) = ei::Context::connect_to_env() {
        debug!("connected to libei via LIBEI_SOCKET");
        return Ok(context);
    }

    debug!("opening RemoteDesktop portal session");
    let remote = RemoteDesktop::new().await.map_err(portal_err)?;
    let session = remote
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(portal_err)?;

    // Recovery flow for the portal restore_token. Without this cache,
    // every wdotool invocation pops a consent dialog. With it: the
    // first run prompts once, every subsequent run is silent until the
    // user revokes.
    //
    // Algorithm:
    //   1. Read the cached token (best-effort — any read failure
    //      degrades to "no token, run first-run consent flow").
    //   2. Try select_devices with the cached token attached.
    //   3. On success → keep the (possibly new) token from the
    //      response. On any error AND we had a cached token → retry
    //      once without the token (forces the consent dialog).
    //   4. On the retry, save whatever new token comes back. If retry
    //      ALSO fails (e.g., user denies consent, compositor crashed
    //      between the two attempts), keep the OLD cached file
    //      untouched so the next session can still try it — the
    //      compositor might have been hiccupping. Propagate the error.
    //
    // Error-class refinement is deferred: ashpd 0.13 doesn't expose a
    // clean "token-invalid" variant, so v0.2.0 retries on ANY error
    // from the with-token path. Real-world testing on KDE + GNOME
    // (issue #1) will tell us which ashpd error variants represent
    // transient vs. token-invalid failures, and a follow-up can
    // narrow the retry condition.
    let cached = match portal_token::load() {
        Ok(c) => c,
        Err(e) => {
            warn!(error = ?e, "couldn't read portal token cache, treating as missing");
            None
        }
    };

    let used_cached = cached.is_some();
    let cached_token: Option<String> = cached.as_ref().map(|c| c.token.clone());

    // run_session_flow runs select_devices + start. Start's response
    // is what carries the freshly-issued restore_token, so both calls
    // need to live inside the retry boundary.
    let selected = match run_session_flow(&remote, &session, cached_token.as_deref()).await {
        Ok(devices) => devices,
        Err(first_err) if used_cached => {
            info!(
                error = %first_err,
                "cached portal token did not satisfy session start, retrying with consent dialog"
            );
            run_session_flow(&remote, &session, None)
                .await
                .map_err(portal_err)?
        }
        Err(err) => return Err(portal_err(err)),
    };

    // Only persist on Some. The portal can return None on a successful
    // start (e.g., user opted out of "remember this choice"); in that
    // case leaving the previous cache untouched is correct — a stale
    // token will trigger the retry path next time and get refreshed
    // there.
    if let Some(new_token) = selected.restore_token() {
        let backend = detect_portal_backend();
        if let Err(e) = portal_token::save(new_token, backend) {
            warn!(error = ?e, "couldn't persist portal token, session still works");
        }
    }

    let fd = remote
        .connect_to_eis(&session, ConnectToEISOptions::default())
        .await
        .map_err(portal_err)?;
    let stream = UnixStream::from(fd);
    ei::Context::new(stream).map_err(|e| WdoError::Backend {
        backend: NAME,
        source: Box::new(e),
    })
}

async fn run_session_flow(
    remote: &RemoteDesktop,
    session: &ashpd::desktop::Session<RemoteDesktop>,
    restore_token: Option<&str>,
) -> ashpd::Result<ashpd::desktop::remote_desktop::SelectedDevices> {
    let select_opts = SelectDevicesOptions::default()
        .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
        .set_persist_mode(PersistMode::ExplicitlyRevoked)
        .set_restore_token(restore_token);
    remote
        .select_devices(session, select_opts)
        .await?
        .response()?;
    remote
        .start(session, None, StartOptions::default())
        .await?
        .response()
}

/// Best-effort identifier of which portal backend issued the token.
/// Diagnostic only — the retry-without-token recovery flow handles
/// backend switches even if the cached `portal_backend` is wrong.
fn detect_portal_backend() -> &'static str {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let lower = desktop.to_lowercase();
    if lower.contains("gnome") {
        "gnome"
    } else if lower.contains("kde") {
        "kde"
    } else {
        "other"
    }
}

fn portal_err(e: ashpd::Error) -> WdoError {
    let msg = e.to_string();
    // ashpd's wording for a missing portal backend is stable enough to match on.
    // Add actionable next-steps for the common compositors.
    let hint = if msg.contains("RemoteDesktop") || msg.contains("portal") {
        Some(
            "\n\nThe compositor isn't exposing the RemoteDesktop portal. Fix:\n  \
             GNOME: install xdg-desktop-portal-gnome (most distros ship it)\n  \
             KDE:   install xdg-desktop-portal-kde\n  \
             Hyprland/Sway/wlroots: RemoteDesktop isn't available on these yet —\n         \
             pass --backend wlroots to use virtual-keyboard/pointer directly",
        )
    } else {
        None
    };
    WdoError::Backend {
        backend: NAME,
        source: match hint {
            Some(h) => format!("{msg}{h}").into(),
            None => Box::new(e),
        },
    }
}

/// Returns true when this event transitions state into "ready" (first
/// DeviceResumed with at least one usable device).
fn handle_event(st: &mut State, event: EiEvent) -> bool {
    match event {
        EiEvent::SeatAdded(ev) => {
            let caps: BitFlags<DeviceCapability> = DeviceCapability::Keyboard
                | DeviceCapability::Pointer
                | DeviceCapability::PointerAbsolute
                | DeviceCapability::Button
                | DeviceCapability::Scroll;
            ev.seat.bind_capabilities(caps);
            trace!("bound seat capabilities");
        }
        EiEvent::DeviceAdded(ev) => {
            let device = ev.device.clone();
            if device.has_capability(DeviceCapability::Keyboard) {
                if let Some(keymap_info) = device.keymap() {
                    match load_keymap(keymap_info) {
                        Ok(km) => st.keymap = Some(SafeKeymap(km)),
                        Err(err) => warn!(?err, "failed to load keymap from EIS"),
                    }
                }
                st.keyboard = Some(device.clone());
            }
            if device.has_capability(DeviceCapability::Pointer) {
                st.pointer = Some(device.clone());
            }
            if device.has_capability(DeviceCapability::PointerAbsolute) {
                st.pointer_abs = Some(device.clone());
            }
        }
        EiEvent::DeviceResumed(_) => {
            return st.keyboard.is_some() || st.pointer.is_some() || st.pointer_abs.is_some();
        }
        EiEvent::DeviceRemoved(ev) => {
            if st.keyboard.as_ref() == Some(&ev.device) {
                st.keyboard = None;
            }
            if st.pointer.as_ref() == Some(&ev.device) {
                st.pointer = None;
            }
            if st.pointer_abs.as_ref() == Some(&ev.device) {
                st.pointer_abs = None;
            }
        }
        EiEvent::Disconnected(d) => {
            warn!(reason = ?d.reason, "EIS disconnected: {:?}", d.explanation);
        }
        _ => {}
    }
    false
}

fn load_keymap(km: &rev::Keymap) -> Result<xkb::Keymap> {
    let fd = km.fd.try_clone().map_err(|e| WdoError::Backend {
        backend: NAME,
        source: Box::new(e),
    })?;
    let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = unsafe {
        xkb::Keymap::new_from_fd(
            &ctx,
            fd,
            km.size as usize,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
    }
    .map_err(|e| WdoError::Backend {
        backend: NAME,
        source: Box::new(e),
    })?
    .ok_or_else(|| WdoError::Backend {
        backend: NAME,
        source: "xkb_keymap_new_from_buffer returned null".into(),
    })?;
    Ok(keymap)
}

/// Find the evdev keycode that produces `target` keysym at level 0 or 1.
/// Returns (keycode, needs_shift).
fn find_keysym_in_keymap(keymap: &xkb::Keymap, target: xkb::Keysym) -> Option<(u32, bool)> {
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

/// Look up the evdev keycode that produces `name` at level 0 or 1. Returns
/// (keycode, needs_shift).
fn resolve_keycode(keymap: &xkb::Keymap, name: &str) -> Option<(u32, bool)> {
    let target = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
    if target.raw() == 0 {
        return None;
    }
    for keycode in keymap.min_keycode().raw()..=keymap.max_keycode().raw() {
        for level in 0..=1 {
            let syms = keymap.key_get_syms_by_level(xkb::Keycode::new(keycode), 0, level);
            if syms.contains(&target) {
                // Evdev keycodes are xkb keycodes minus 8.
                return Some((keycode.saturating_sub(8), level == 1));
            }
        }
    }
    None
}

fn xdotool_button_to_evdev(btn: MouseButton) -> Option<u32> {
    // linux/input-event-codes.h
    const BTN_LEFT: u32 = 0x110;
    const BTN_RIGHT: u32 = 0x111;
    const BTN_MIDDLE: u32 = 0x112;
    const BTN_SIDE: u32 = 0x113; // back
    const BTN_EXTRA: u32 = 0x114; // forward
    Some(match btn {
        MouseButton::Left => BTN_LEFT,
        MouseButton::Middle => BTN_MIDDLE,
        MouseButton::Right => BTN_RIGHT,
        MouseButton::Back => BTN_SIDE,
        MouseButton::Forward => BTN_EXTRA,
        MouseButton::Other(_) => return None,
    })
}

#[async_trait]
impl Backend for LibeiBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn capabilities(&self) -> Capabilities {
        let st = self.state.lock().unwrap();
        Capabilities {
            key_input: st.keyboard.is_some() && st.keymap.is_some(),
            text_input: false,
            pointer_move_absolute: st.pointer_abs.is_some(),
            pointer_move_relative: st.pointer.is_some(),
            pointer_button: st.pointer.is_some() || st.pointer_abs.is_some(),
            scroll: st.pointer.is_some() || st.pointer_abs.is_some(),
            list_windows: false,
            active_window: false,
            activate_window: false,
            close_window: false,
        }
    }

    async fn key(&self, keysym: &str, dir: KeyDirection) -> Result<()> {
        let (keycode, needs_shift, shift_kc) = {
            let st = self.state.lock().unwrap();
            let keymap = st.keymap.as_ref().ok_or(WdoError::NotSupported {
                backend: NAME,
                what: "no keymap received from EIS",
            })?;
            let (kc, needs_shift) =
                resolve_keycode(&keymap.0, keysym).ok_or_else(|| WdoError::Keysym {
                    input: keysym.into(),
                    reason: format!("keysym '{keysym}' not found in server keymap"),
                })?;
            let shift_kc = resolve_keycode(&keymap.0, "Shift_L").map(|(kc, _)| kc);
            (kc, needs_shift, shift_kc)
        };

        self.emit_frame(DeviceChoice::Keyboard, |device, _st| {
            let Some(kb) = device.interface::<ei::Keyboard>() else {
                return;
            };
            match dir {
                KeyDirection::Press => {
                    if needs_shift {
                        if let Some(kc) = shift_kc {
                            kb.key(kc, ei::keyboard::KeyState::Press);
                        }
                    }
                    kb.key(keycode, ei::keyboard::KeyState::Press);
                }
                KeyDirection::Release => {
                    kb.key(keycode, ei::keyboard::KeyState::Released);
                    if needs_shift {
                        if let Some(kc) = shift_kc {
                            kb.key(kc, ei::keyboard::KeyState::Released);
                        }
                    }
                }
                KeyDirection::PressRelease => {
                    if needs_shift {
                        if let Some(kc) = shift_kc {
                            kb.key(kc, ei::keyboard::KeyState::Press);
                        }
                    }
                    kb.key(keycode, ei::keyboard::KeyState::Press);
                    kb.key(keycode, ei::keyboard::KeyState::Released);
                    if needs_shift {
                        if let Some(kc) = shift_kc {
                            kb.key(kc, ei::keyboard::KeyState::Released);
                        }
                    }
                }
            }
        })
    }

    async fn type_text(&self, text: &str, delay: Duration) -> Result<()> {
        // libei is a SENDER context: the EIS server owns the keymap. We can
        // only emit keycodes that already exist in that keymap, so Unicode
        // support is strictly bounded by what the server layout covers.
        //
        // This is a best-effort fallback. For full Unicode injection, use the
        // wlroots backend (which CAN install a transient keymap).
        let resolved: Vec<(u32, bool)> = {
            let st = self.state.lock().unwrap();
            let keymap = st.keymap.as_ref().ok_or(WdoError::NotSupported {
                backend: NAME,
                what: "no keymap received from EIS",
            })?;
            let mut out = Vec::with_capacity(text.chars().count());
            let mut missing: Vec<char> = Vec::new();
            for c in text.chars() {
                let sym = xkb::Keysym::from_char(c);
                if sym.raw() == 0 {
                    missing.push(c);
                    continue;
                }
                // resolve_keycode takes a keysym NAME; convert via utf8 repr
                // when possible, otherwise look up by raw keysym on the keymap.
                match find_keysym_in_keymap(&keymap.0, sym) {
                    Some(pair) => out.push(pair),
                    None => missing.push(c),
                }
            }
            if !missing.is_empty() {
                warn!(
                    "libei type_text: {} char(s) not in server keymap: {:?}",
                    missing.len(),
                    missing.iter().take(8).collect::<Vec<_>>()
                );
            }
            out
        };

        let shift_kc = {
            let st = self.state.lock().unwrap();
            st.keymap
                .as_ref()
                .and_then(|km| resolve_keycode(&km.0, "Shift_L"))
                .map(|(kc, _)| kc)
        };

        let chars_count = text.chars().count();
        for (idx, (keycode, needs_shift)) in resolved.into_iter().enumerate() {
            self.emit_frame(DeviceChoice::Keyboard, |device, _st| {
                let Some(kb) = device.interface::<ei::Keyboard>() else {
                    return;
                };
                if needs_shift {
                    if let Some(kc) = shift_kc {
                        kb.key(kc, ei::keyboard::KeyState::Press);
                    }
                }
                kb.key(keycode, ei::keyboard::KeyState::Press);
                kb.key(keycode, ei::keyboard::KeyState::Released);
                if needs_shift {
                    if let Some(kc) = shift_kc {
                        kb.key(kc, ei::keyboard::KeyState::Released);
                    }
                }
            })?;
            if !delay.is_zero() && idx + 1 < chars_count {
                tokio::time::sleep(delay).await;
            }
        }
        Ok(())
    }

    async fn mouse_move(&self, x: i32, y: i32, absolute: bool) -> Result<()> {
        if absolute {
            self.emit_frame(DeviceChoice::PointerAbsolute, |device, _| {
                if let Some(p) = device.interface::<ei::PointerAbsolute>() {
                    p.motion_absolute(x as f32, y as f32);
                }
            })
        } else {
            self.emit_frame(DeviceChoice::Pointer, |device, _| {
                if let Some(p) = device.interface::<ei::Pointer>() {
                    p.motion_relative(x as f32, y as f32);
                }
            })
        }
    }

    async fn mouse_button(&self, btn: MouseButton, dir: KeyDirection) -> Result<()> {
        let code = xdotool_button_to_evdev(btn)
            .ok_or_else(|| WdoError::InvalidArg(format!("unsupported mouse button: {btn:?}")))?;
        self.emit_frame(DeviceChoice::Pointer, |device, _| {
            let Some(b) = device.interface::<ei::Button>() else {
                return;
            };
            match dir {
                KeyDirection::Press => b.button(code, ei::button::ButtonState::Press),
                KeyDirection::Release => b.button(code, ei::button::ButtonState::Released),
                KeyDirection::PressRelease => {
                    b.button(code, ei::button::ButtonState::Press);
                    b.button(code, ei::button::ButtonState::Released);
                }
            }
        })
    }

    async fn scroll(&self, dx: f64, dy: f64) -> Result<()> {
        self.emit_frame(DeviceChoice::Pointer, |device, _| {
            if let Some(s) = device.interface::<ei::Scroll>() {
                s.scroll(dx as f32, dy as f32);
            }
        })
    }

    async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        Err(WdoError::NotSupported {
            backend: NAME,
            what: "list_windows — libei has no window API; pair with a WindowBackend",
        })
    }

    async fn active_window(&self) -> Result<Option<WindowInfo>> {
        Err(WdoError::NotSupported {
            backend: NAME,
            what: "active_window — libei has no window API; pair with a WindowBackend",
        })
    }

    async fn activate_window(&self, _id: &WindowId) -> Result<()> {
        Err(WdoError::NotSupported {
            backend: NAME,
            what: "activate_window — libei has no window API; pair with a WindowBackend",
        })
    }

    async fn close_window(&self, _id: &WindowId) -> Result<()> {
        Err(WdoError::NotSupported {
            backend: NAME,
            what: "close_window — libei has no window API; pair with a WindowBackend",
        })
    }
}
