//! Wayland input observer for wdotool integration tests.
//!
//! Connects to `$WAYLAND_DISPLAY`, creates a 1x1 toplevel surface so
//! the compositor will give it focus, takes the keyboard and pointer
//! on its seat, and writes one line per received input event to stdout.
//!
//! Test code spawns this binary inside a headless compositor (sway with
//! `WLR_BACKENDS=headless`), drives wdotool against the same display,
//! and reads the observer's stdout to assert on what events the
//! compositor actually delivered. That's the unlock that lets us pin
//! "wdotool key ctrl+shift+a sends Ctrl+Shift+a to a Wayland client"
//! in CI without a human looking at a screen.
//!
//! Output format is one line per event, space-separated, designed for
//! grep-friendly test asserts:
//!
//! ```text
//! ready
//! keymap_changed
//! key 38 a press
//! key 38 a release
//! pointer_enter 0.5 0.5
//! pointer_motion 10.0 20.0
//! pointer_button 272 press
//! pointer_button 272 release
//! pointer_axis vertical 1.0
//! pointer_axis horizontal -0.5
//! pointer_leave
//! ```
//!
//! The first `ready` line means the surface is mapped and ready to
//! receive focus; tests should wait for it before starting to send
//! input. Lines are flushed after each event so the test doesn't
//! deadlock on a buffered pipe.

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("wdotool-observer only runs on Linux");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(e) = linux_main::run() {
        eprintln!("wdotool-observer: {e}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
mod linux_main {
    use std::fs::File;
    use std::io::{Seek, SeekFrom, Write};
    use std::os::fd::{AsFd, OwnedFd};

    use rustix::shm;
    use wayland_client::{
        protocol::{
            wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm,
            wl_shm_pool, wl_surface,
        },
        Connection, Dispatch, EventQueue, QueueHandle, WEnum,
    };
    use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
    use xkbcommon::xkb;

    /// All wl-registry globals we care about, plus the surface and the
    /// running event-emitter state. One per process.
    pub struct State {
        // Globals (bound during the first roundtrip).
        compositor: Option<wl_compositor::WlCompositor>,
        shm: Option<wl_shm::WlShm>,
        xdg_wm_base: Option<xdg_wm_base::XdgWmBase>,
        seat: Option<wl_seat::WlSeat>,

        // Surface + lifecycle.
        surface: Option<wl_surface::WlSurface>,
        xdg_surface: Option<xdg_surface::XdgSurface>,
        xdg_toplevel: Option<xdg_toplevel::XdgToplevel>,
        configured: bool,
        ready_emitted: bool,

        // Input devices, bound lazily when the seat advertises the
        // matching capability. In a headless sway with no real input
        // devices, the seat starts with capabilities=0 and only
        // gains keyboard/pointer caps when another client (e.g.
        // wdotool's wlroots backend) creates a virtual_keyboard /
        // virtual_pointer instance. Eagerly calling get_keyboard /
        // get_pointer at startup is a protocol error.
        keyboard: Option<wl_keyboard::WlKeyboard>,
        pointer: Option<wl_pointer::WlPointer>,

        // xkb state for keysym name resolution.
        xkb_context: xkb::Context,
        xkb_keymap: Option<xkb::Keymap>,
        xkb_state: Option<xkb::State>,
    }

    impl State {
        fn new() -> Self {
            Self {
                compositor: None,
                shm: None,
                xdg_wm_base: None,
                seat: None,
                surface: None,
                xdg_surface: None,
                xdg_toplevel: None,
                configured: false,
                ready_emitted: false,
                keyboard: None,
                pointer: None,
                xkb_context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
                xkb_keymap: None,
                xkb_state: None,
            }
        }
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let conn = Connection::connect_to_env()?;
        let display = conn.display();

        let mut event_queue: EventQueue<State> = conn.new_event_queue();
        let qh = event_queue.handle();

        let _registry = display.get_registry(&qh, ());
        let mut state = State::new();

        // First roundtrip: bind globals.
        event_queue.roundtrip(&mut state)?;

        let compositor = state.compositor.clone().ok_or("missing wl_compositor")?;
        let shm = state.shm.clone().ok_or("missing wl_shm")?;
        let xdg_wm_base = state
            .xdg_wm_base
            .clone()
            .ok_or("missing xdg_wm_base (compositor lacks xdg-shell?)")?;
        // Hold onto the seat so its dispatch handler can fire when
        // the compositor advertises capabilities later. Don't bind
        // keyboard/pointer here; that happens lazily.
        let _seat = state.seat.clone().ok_or("missing wl_seat")?;

        // Build the surface and turn it into a toplevel. Tiny 1x1
        // buffer is enough for the compositor to consider us mappable;
        // we never actually paint anything visible.
        let surface = compositor.create_surface(&qh, ());
        let xdg_surf = xdg_wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surf.get_toplevel(&qh, ());
        toplevel.set_app_id("wdotool-observer".into());
        toplevel.set_title("wdotool observer".into());
        surface.commit();

        state.surface = Some(surface.clone());
        state.xdg_surface = Some(xdg_surf);
        state.xdg_toplevel = Some(toplevel);

        // We don't bind keyboard/pointer here; they're bound lazily
        // from the seat's Capabilities event handler when the
        // compositor advertises them. Headless sway starts with
        // capabilities=0 and only gains them when wdotool's wlroots
        // backend creates virtual input devices.

        // Now wait for configure and for keyboard/pointer to come up.
        // After that the run loop just dispatches forever, printing
        // events as they arrive.
        loop {
            event_queue.blocking_dispatch(&mut state)?;

            if !state.ready_emitted && state.configured {
                // Attach a 1x1 ARGB buffer so the compositor accepts
                // our surface as mappable. Without an attached buffer
                // sway leaves us in a "configured but not visible"
                // limbo where focus never lands.
                let buffer = make_one_pixel_buffer(&shm, &qh)?;
                state.surface.as_ref().unwrap().attach(Some(&buffer), 0, 0);
                state.surface.as_ref().unwrap().damage(0, 0, 1, 1);
                state.surface.as_ref().unwrap().commit();

                println!("ready");
                std::io::stdout().flush().ok();
                state.ready_emitted = true;
            }
        }
    }

    /// Build a 1x1 ARGB8888 wl_buffer. The compositor only needs us to
    /// have *something* attached for the surface to be mappable; the
    /// pixel content is irrelevant since we're headless.
    fn make_one_pixel_buffer(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<State>,
    ) -> Result<wl_buffer::WlBuffer, Box<dyn std::error::Error>> {
        let size: i32 = 4; // 1 px * 4 bytes per ARGB8888

        let fd = create_anonymous_shm(size as usize)?;
        let pool = shm.create_pool(fd.as_fd(), size, qh, ());
        let buffer = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, qh, ());
        pool.destroy();
        Ok(buffer)
    }

    /// memfd_create a small anonymous file the compositor can mmap as
    /// shared memory. Falls back to a tempfile if memfd isn't available
    /// (which it always is on modern Linux, but the ergonomics of the
    /// fallback are fine for portability).
    fn create_anonymous_shm(size: usize) -> Result<OwnedFd, Box<dyn std::error::Error>> {
        let fd = shm::open(
            "/wdotool-observer-shm",
            shm::OFlags::CREATE | shm::OFlags::EXCL | shm::OFlags::RDWR,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )?;
        // Unlink immediately; the open fd keeps the segment alive.
        let _ = shm::unlink("/wdotool-observer-shm");

        let mut file = File::from(fd);
        file.set_len(size as u64)?;
        file.seek(SeekFrom::Start(0))?;
        // ARGB(0,0,0,0) is fine. Don't bother painting.
        file.write_all(&[0, 0, 0, 0])?;
        Ok(file.into())
    }

    // ============================================================
    // Dispatch implementations: one impl per object we receive
    // events from. Each handler prints one stdout line per event we
    // care about, then flushes so tests reading the pipe make progress.
    // ============================================================

    impl Dispatch<wl_registry::WlRegistry, ()> for State {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _conn: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global {
                name,
                interface,
                version,
            } = event
            {
                match interface.as_str() {
                    "wl_compositor" => {
                        let v = version.min(4);
                        state.compositor = Some(
                            registry.bind::<wl_compositor::WlCompositor, _, _>(name, v, qh, ()),
                        );
                    }
                    "wl_shm" => {
                        state.shm = Some(registry.bind::<wl_shm::WlShm, _, _>(
                            name,
                            version.min(1),
                            qh,
                            (),
                        ));
                    }
                    "xdg_wm_base" => {
                        state.xdg_wm_base = Some(registry.bind::<xdg_wm_base::XdgWmBase, _, _>(
                            name,
                            version.min(2),
                            qh,
                            (),
                        ));
                    }
                    "wl_seat" => {
                        state.seat = Some(registry.bind::<wl_seat::WlSeat, _, _>(
                            name,
                            version.min(7),
                            qh,
                            (),
                        ));
                    }
                    _ => {}
                }
            }
        }
    }

    impl Dispatch<wl_compositor::WlCompositor, ()> for State {
        fn event(
            _: &mut Self,
            _: &wl_compositor::WlCompositor,
            _: wl_compositor::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<wl_shm::WlShm, ()> for State {
        fn event(
            _: &mut Self,
            _: &wl_shm::WlShm,
            _: wl_shm::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
        fn event(
            _: &mut Self,
            _: &wl_shm_pool::WlShmPool,
            _: wl_shm_pool::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<wl_buffer::WlBuffer, ()> for State {
        fn event(
            _: &mut Self,
            _: &wl_buffer::WlBuffer,
            _: wl_buffer::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<wl_surface::WlSurface, ()> for State {
        fn event(
            _: &mut Self,
            _: &wl_surface::WlSurface,
            _: wl_surface::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
        fn event(
            _: &mut Self,
            wm_base: &xdg_wm_base::XdgWmBase,
            event: xdg_wm_base::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_wm_base::Event::Ping { serial } = event {
                wm_base.pong(serial);
            }
        }
    }

    impl Dispatch<xdg_surface::XdgSurface, ()> for State {
        fn event(
            state: &mut Self,
            xdg_surf: &xdg_surface::XdgSurface,
            event: xdg_surface::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_surface::Event::Configure { serial } = event {
                xdg_surf.ack_configure(serial);
                state.configured = true;
            }
        }
    }

    impl Dispatch<xdg_toplevel::XdgToplevel, ()> for State {
        fn event(
            _: &mut Self,
            _: &xdg_toplevel::XdgToplevel,
            event: xdg_toplevel::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_toplevel::Event::Close = event {
                println!("close");
                std::io::stdout().flush().ok();
                std::process::exit(0);
            }
        }
    }

    impl Dispatch<wl_seat::WlSeat, ()> for State {
        fn event(
            state: &mut Self,
            seat: &wl_seat::WlSeat,
            event: wl_seat::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let wl_seat::Event::Capabilities {
                capabilities: WEnum::Value(caps),
            } = event
            {
                // Logging the cap mask helps debug "where did my
                // event go" failures.
                println!("seat_caps {:#x}", caps.bits());
                std::io::stdout().flush().ok();

                // Per the wl_seat spec, when a capability is removed
                // the client should release the matching wl_pointer /
                // wl_keyboard, and rebind on the next "added" event.
                // Without this, after the first wdotool run finishes
                // and removes its virtual device, the observer's
                // pointer/keyboard becomes inert and a second
                // wdotool invocation delivers events to a dead object.
                if !caps.contains(wl_seat::Capability::Keyboard) {
                    if let Some(k) = state.keyboard.take() {
                        k.release();
                    }
                }
                if !caps.contains(wl_seat::Capability::Pointer) {
                    if let Some(p) = state.pointer.take() {
                        p.release();
                    }
                }

                if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                    state.keyboard = Some(seat.get_keyboard(qh, ()));
                }
                if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                    state.pointer = Some(seat.get_pointer(qh, ()));
                }
            }
        }
    }

    impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
        fn event(
            state: &mut Self,
            _: &wl_keyboard::WlKeyboard,
            event: wl_keyboard::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                wl_keyboard::Event::Keymap { format, fd, size } => {
                    if matches!(format, WEnum::Value(wl_keyboard::KeymapFormat::XkbV1)) {
                        // SAFETY: the compositor sent us a freshly-mmappable fd
                        // describing an xkb v1 keymap. xkbcommon's
                        // new_from_fd takes ownership and mmaps it.
                        if let Ok(Some(keymap)) = unsafe {
                            xkb::Keymap::new_from_fd(
                                &state.xkb_context,
                                fd,
                                size as usize,
                                xkb::KEYMAP_FORMAT_TEXT_V1,
                                xkb::KEYMAP_COMPILE_NO_FLAGS,
                            )
                        } {
                            let xkb_state = xkb::State::new(&keymap);
                            state.xkb_keymap = Some(keymap);
                            state.xkb_state = Some(xkb_state);
                            println!("keymap_changed");
                            std::io::stdout().flush().ok();
                        }
                    }
                }
                wl_keyboard::Event::Key {
                    key,
                    state: key_state,
                    ..
                } => {
                    let action = match key_state {
                        WEnum::Value(wl_keyboard::KeyState::Pressed) => "press",
                        WEnum::Value(wl_keyboard::KeyState::Released) => "release",
                        _ => "?",
                    };
                    // wl_keyboard.key reports linux evdev keycodes; xkb
                    // expects evdev+8.
                    let xkb_keycode = key + 8;
                    let name = state
                        .xkb_state
                        .as_ref()
                        .map(|s| s.key_get_one_sym(xkb_keycode.into()))
                        .filter(|s| s.raw() != 0)
                        .map(xkb::keysym_get_name)
                        .unwrap_or_else(|| "?".into());
                    println!("key {key} {name} {action}");
                    std::io::stdout().flush().ok();
                }
                wl_keyboard::Event::Modifiers {
                    mods_depressed,
                    mods_latched,
                    mods_locked,
                    group,
                    ..
                } => {
                    if let Some(s) = state.xkb_state.as_mut() {
                        s.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                    }
                    println!("modifiers {mods_depressed:#x} {mods_latched:#x} {mods_locked:#x}");
                    std::io::stdout().flush().ok();
                }
                wl_keyboard::Event::Enter { .. } => {
                    println!("keyboard_enter");
                    std::io::stdout().flush().ok();
                }
                wl_keyboard::Event::Leave { .. } => {
                    println!("keyboard_leave");
                    std::io::stdout().flush().ok();
                }
                _ => {}
            }
        }
    }

    impl Dispatch<wl_pointer::WlPointer, ()> for State {
        fn event(
            _: &mut Self,
            _: &wl_pointer::WlPointer,
            event: wl_pointer::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                wl_pointer::Event::Enter {
                    surface_x,
                    surface_y,
                    ..
                } => {
                    println!("pointer_enter {surface_x:.1} {surface_y:.1}");
                }
                wl_pointer::Event::Leave { .. } => {
                    println!("pointer_leave");
                }
                wl_pointer::Event::Motion {
                    surface_x,
                    surface_y,
                    ..
                } => {
                    println!("pointer_motion {surface_x:.1} {surface_y:.1}");
                }
                wl_pointer::Event::Button {
                    button,
                    state: btn_state,
                    ..
                } => {
                    let action = match btn_state {
                        WEnum::Value(wl_pointer::ButtonState::Pressed) => "press",
                        WEnum::Value(wl_pointer::ButtonState::Released) => "release",
                        _ => "?",
                    };
                    println!("pointer_button {button} {action}");
                }
                wl_pointer::Event::Axis { axis, value, .. } => {
                    let label = match axis {
                        WEnum::Value(wl_pointer::Axis::VerticalScroll) => "vertical",
                        WEnum::Value(wl_pointer::Axis::HorizontalScroll) => "horizontal",
                        _ => "?",
                    };
                    println!("pointer_axis {label} {value:.2}");
                }
                _ => {}
            }
            std::io::stdout().flush().ok();
        }
    }
}
