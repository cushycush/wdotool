# wdotool

An xdotool-compatible input automation CLI for Wayland, built on the protocols that were actually designed for this.

## Why

- **xdotool** is X11-only and does not work on Wayland.
- **ydotool** writes to `/dev/uinput`, which means root (or careful udev rules), no focus awareness, and no window management. It bypasses the compositor entirely, which breaks in sandboxed sessions and loses any security boundary.
- **wdotool** uses the protocols Wayland already provides for this: libei (via the XDG RemoteDesktop portal), wlroots' virtual-keyboard/pointer, and foreign-toplevel-management. It respects compositor focus and permissions, and only falls back to uinput when nothing better is available.

## Status

Early but usable. Tested on Hyprland. Current surface:

| Feature                         | libei | wlroots | uinput   |
| ------------------------------- | ----- | ------- | -------- |
| `key` / `keydown` / `keyup`     | ✅    | ✅      | planned  |
| `type` (Unicode via keymap)     | partial¹ | ✅      | planned² |
| `mousemove` (relative)          | ✅    | ✅      | planned  |
| `mousemove` (absolute)          | ✅    | ✅³     | planned  |
| `click` / `mousedown` / `mouseup` | ✅    | ✅      | planned  |
| `scroll`                        | ✅    | ✅      | planned  |
| `search` / `getactivewindow`    | —     | ✅      | —        |
| `windowactivate` / `windowclose` | —     | ✅      | —        |

¹ libei is a sender context; the EIS server owns the keymap. Characters not in the active layout are skipped with a warning.
² uinput has the same limitation as libei — the kernel doesn't know about keymaps.
³ wlroots absolute pointer needs output geometry to be meaningful; currently uses a 10,000×10,000 logical extent as a placeholder.

## Install

```sh
# from source
cargo install --path .

# or build locally
cargo build --release
./target/release/wdotool --help
```

## Usage

Drop-in xdotool replacement for the common commands:

```sh
wdotool info                              # show detected backend + capabilities

wdotool key ctrl+c                        # send Ctrl+C
wdotool keydown shift                     # press and hold Shift
wdotool keyup shift                       # release it

wdotool type "hello 世界 €"               # Unicode works on wlroots
wdotool type --delay 30 "slow typing"     # 30ms between chars

wdotool mousemove 500 400                 # absolute
wdotool mousemove --relative 10 -5        # relative
wdotool click 1                           # 1=left, 2=middle, 3=right, 8=back, 9=forward
wdotool scroll 0 3                        # scroll 3 units down

wdotool search --name "Firefox"           # list matching windows
wdotool getactivewindow                   # print focused window's id
wdotool windowactivate <id>               # focus a window
wdotool windowclose <id>                  # close a window
```

Global flags:

```sh
--backend <libei|wlroots|kde|gnome|uinput>   # force a specific backend
-v, --verbose                                # show detection + bind logs
```

## Backends

### libei (preferred on GNOME / KDE)

Opens an `org.freedesktop.portal.RemoteDesktop` session via the XDG portal, negotiates a libei socket, handshakes as a sender context, and emits input through the compositor's own libei implementation. Full focus awareness and permission prompts.

**Requirements:**
- `xdg-desktop-portal` + a backend that exports `org.freedesktop.portal.RemoteDesktop`. GNOME (46+) and KDE Plasma 6 both ship this.
- On Hyprland specifically, `xdg-desktop-portal-hyprland` 1.3.11 does **not** yet expose RemoteDesktop. Use the wlroots backend instead.

### wlroots (preferred on Sway, Hyprland, river, Wayfire)

Binds `zwp_virtual_keyboard_v1`, `zwlr_virtual_pointer_v1`, and `zwlr_foreign_toplevel_management_v1` directly. Uploads a transient xkb keymap at `type` time (one keycode per unique character as a Level-0 Unicode keysym) — this is how arbitrary Unicode works without a server-side keymap change.

**Requirements:**
- A wlroots-based compositor that exposes the three protocols above. Sway, Hyprland, river, and Wayfire all do by default.

### uinput (last resort — planned)

Creates a virtual input device via `/dev/uinput`. Compositor-agnostic but has no focus awareness and needs permission to open the device (either `input`-group membership or a udev rule).

## Supported compositors

| Compositor   | Input backend | Window backend |
| ------------ | ------------- | -------------- |
| Hyprland     | wlroots       | wlroots        |
| Sway         | wlroots       | wlroots        |
| river        | wlroots       | wlroots        |
| Wayfire      | wlroots       | wlroots        |
| GNOME (46+)  | libei         | planned (extension) |
| KDE Plasma 6 | libei         | planned (D-Bus / KWin) |
| Anything else | uinput (planned) | — |

Backend selection is automatic based on `XDG_CURRENT_DESKTOP` and compositor hints (`SWAYSOCK`, `HYPRLAND_INSTANCE_SIGNATURE`, etc.). The detector tries the preferred backend first and falls through to alternatives if it fails to bootstrap — use `--backend` to force a specific one.

## Building

Requires Rust 1.75+ (for async-in-traits via `async_trait`). Builds cleanly on stable.

```sh
cargo build             # dev
cargo build --release   # ~4 MB stripped binary
cargo test              # unit tests
```

System libraries: `libxkbcommon` (runtime), `libwayland-client`. Both are universally present on Wayland systems.

## Known limitations

- Unicode `type` works only on wlroots today; libei falls back to the server's keymap and skips characters it can't find.
- Absolute mouse coordinates on wlroots use a 10,000×10,000 logical grid because the backend doesn't yet track `wl_output` geometry.
- The only window backend is wlroots' `foreign-toplevel`. KDE and GNOME window management are planned.
- No pointer region / multi-seat handling yet — the first seat gets everything.

## Project layout

```
src/
  main.rs            # CLI entry (tokio)
  cli.rs             # clap subcommand definitions
  error.rs           # WdoError
  types.rs           # Capabilities, KeyDirection, MouseButton, WindowInfo
  keysym.rs          # ctrl+shift+a parser
  backend/
    mod.rs           # Backend trait (async_trait)
    detector.rs      # runtime backend selection
    libei.rs         # RemoteDesktop portal + reis
    wlroots.rs       # virtual-keyboard/pointer + foreign-toplevel
    stub.rs          # PendingBackend placeholder
```

Every real backend runs on a dedicated OS thread because the underlying event streams (`reis::EiConvertEventStream`, `wayland_client::EventQueue`) aren't `Send`. Input ops are dispatched to that thread via channels.

## License

MIT OR Apache-2.0
