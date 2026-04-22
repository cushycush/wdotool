# wdotool

An xdotool-compatible input automation CLI for Wayland, built on the protocols that were actually designed for this.

## Why

- **xdotool** is X11-only and does not work on Wayland.
- **ydotool** writes to `/dev/uinput`, which means root (or careful udev rules), no focus awareness, and no window management. It bypasses the compositor entirely, which breaks in sandboxed sessions and loses any security boundary.
- **wdotool** uses the protocols Wayland already provides for this: libei (via the XDG RemoteDesktop portal), wlroots' virtual-keyboard/pointer, and foreign-toplevel-management. It respects compositor focus and permissions, and only falls back to uinput when nothing better is available.

## Status

Early but usable. Tested on Hyprland. Current surface:

| Feature                         | libei    | wlroots | uinput   |
| ------------------------------- | -------- | ------- | -------- |
| `key` / `keydown` / `keyup`     | ✅       | ✅      | ✅       |
| `type` (Unicode via keymap)     | partial¹ | ✅      | partial² |
| `mousemove` (relative)          | ✅       | ✅      | ✅       |
| `mousemove` (absolute)          | ✅       | ✅³     | ✅       |
| `click` / `mousedown` / `mouseup` | ✅     | ✅      | ✅       |
| `scroll`                        | ✅       | ✅      | ✅       |
| `search` / `getactivewindow`    | —        | ✅      | —        |
| `windowactivate` / `windowclose` | —       | ✅      | —        |

¹ libei is a sender context; the EIS server owns the keymap. Characters not in the active layout are skipped with a warning.
² uinput has the same limitation as libei — the kernel doesn't know about keymaps. Best-effort via the env-default xkb layout.
³ wlroots absolute pointer needs output geometry to be meaningful; currently uses a 10,000×10,000 logical extent as a placeholder.

## Install

```sh
# Arch Linux (AUR) — three flavors, pick one:
yay -S wdotool        # source build at the latest stable tag
yay -S wdotool-bin    # prebuilt x86_64 binary (fastest install)
yay -S wdotool-git    # rolling build from main

# Prebuilt binary via shell installer (any glibc x86_64 Linux)
curl -LsSf https://github.com/cushycush/wdotool/releases/latest/download/wdotool-installer.sh | sh

# Nix (flake)
nix run github:cushycush/wdotool -- --help
# or add to a NixOS/home-manager config:
# inputs.wdotool.url = "github:cushycush/wdotool";

# From crates.io (any Linux arch; builds on install)
cargo install wdotool

# From source
git clone https://github.com/cushycush/wdotool && cd wdotool
cargo build --release
./target/release/wdotool --help
```

Runtime deps: `libxkbcommon` and `libwayland-client`. Both are universally present on Wayland systems.

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

### kde (KDE Plasma 5/6)

Composes libei (for input) with KWin-scripting window management over D-Bus. Generates small JS snippets, hands them to `org.kde.KWin.Scripting.loadScriptFromText`, runs them, and receives results on a transient `com.wdotool.KdeBridge` D-Bus service the backend registers at startup. Same trick `kdotool` uses, adapted to both the Plasma 6 (`workspace.windowList`, `workspace.activeWindow`) and Plasma 5 (`workspace.clientList`, `workspace.activeClient`) APIs.

**Requirements:**
- `xdg-desktop-portal-kde` for the libei input path.
- KWin 5.22+ for the Scripting D-Bus interface.

### uinput (last resort)

Creates a virtual input device via `/dev/uinput` and writes raw `input_event` structs through the kernel. Compositor-agnostic — works on Wayland, X11, or a bare framebuffer session — but has no focus awareness.

**Requirements:**
- Write access to `/dev/uinput`. The usual setup is `usermod -aG uinput $USER` (or `input` on some distros) plus a udev rule if the device isn't created with the group by default:
  ```
  KERNEL=="uinput", GROUP="uinput", MODE="0660"
  ```
  On Arch with `systemd-tmpfiles`, `uaccess` tags work too.
- xkb configuration present on the system (for keysym → keycode translation). Installed by default on every Wayland/X11 install.

## Supported compositors

| Compositor   | Input backend | Window backend |
| ------------ | ------------- | -------------- |
| Hyprland     | wlroots       | wlroots        |
| Sway         | wlroots       | wlroots        |
| river        | wlroots       | wlroots        |
| Wayfire      | wlroots       | wlroots        |
| GNOME (46+)  | libei         | planned (extension) |
| KDE Plasma 6 | libei         | KWin scripting (D-Bus) |
| Anything else | uinput        | — |

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
