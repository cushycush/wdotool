# wdotool

[![crates.io](https://img.shields.io/crates/v/wdotool.svg?style=flat-square&label=crates.io)](https://crates.io/crates/wdotool)
[![CI](https://img.shields.io/github/actions/workflow/status/cushycush/wdotool/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/cushycush/wdotool/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg?style=flat-square)](#license)

An xdotool-compatible input automation CLI for Wayland, built on the protocols that were actually designed for this.

## Why

- **xdotool** is X11-only and does not work on Wayland.
- **ydotool** writes to `/dev/uinput`, which means root (or careful udev rules), no focus awareness, and no window management. It bypasses the compositor entirely, which breaks in sandboxed sessions and loses any security boundary.
- **wdotool** uses the protocols Wayland already provides for this: libei (via the XDG RemoteDesktop portal), wlroots' virtual-keyboard/pointer, and foreign-toplevel-management. It respects compositor focus and permissions, and only falls back to uinput when nothing better is available.

## Status

Early but usable. Actively tested on Hyprland + wlroots. The KDE and GNOME backends need help from people running those desktops to verify — see [issue #1](https://github.com/cushycush/wdotool/issues/1) (KDE) and [issue #2](https://github.com/cushycush/wdotool/issues/2) (GNOME).

| Feature                         | libei    | wlroots | kde       | gnome     | uinput   |
| ------------------------------- | -------- | ------- | --------- | --------- | -------- |
| `key` / `keydown` / `keyup`     | ✅       | ✅      | ✅        | ✅        | ✅       |
| `type` (Unicode via keymap)     | partial¹ | ✅      | partial¹  | partial¹  | partial² |
| `mousemove` (relative)          | ✅       | ✅      | ✅        | ✅        | ✅       |
| `mousemove` (absolute)          | ✅       | ✅      | ✅        | ✅        | ✅       |
| `click` / `mousedown` / `mouseup` | ✅     | ✅      | ✅        | ✅        | ✅       |
| `scroll`                        | ✅       | ✅      | ✅        | ✅        | ✅       |
| `search` / `getactivewindow`    | —        | ✅      | ✅³       | ✅⁴       | —        |
| `windowactivate` / `windowclose` | —       | ✅      | ✅³       | ✅⁴       | —        |

¹ libei (and `kde` / `gnome`, which use libei for input) is a sender context; the EIS server owns the keymap. Characters not in the active layout are skipped with a warning.
² uinput has the same limitation as libei — the kernel doesn't know about keymaps. Best-effort via the env-default xkb layout.
³ Implemented but unverified on a real Plasma session ([issue #1](https://github.com/cushycush/wdotool/issues/1)).
⁴ Requires the companion GNOME Shell extension in `packaging/gnome-extension/wdotool@wdotool.github.io/` — see [issue #2](https://github.com/cushycush/wdotool/issues/2). Without it, `gnome` falls back to bare libei (input only).

## Install

Linux only (Wayland protocols, `/dev/uinput`).

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

# crates.io (Linux, any arch; builds on install)
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
wdotool type --file script.txt            # read text from a file
echo "hello" | wdotool type --file -      # read from stdin

wdotool key --clearmodifiers ctrl+c       # release stuck modifiers first
wdotool type --clearmodifiers "hi"        # (works on key/keydown/keyup/type)

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

**Heads up**: wdotool injects keystrokes and pointer events into whichever window has focus when the command runs. Wayland's security model has no "target a specific window" API for input — focus the target first, then run. `--clearmodifiers` is approximate on Wayland for the same reason: we can't observe current modifier state, so the flag unconditionally releases Ctrl/Shift/Alt/Super/AltGr rather than doing xdotool's save-and-restore.

## Backends

### libei

Opens an `org.freedesktop.portal.RemoteDesktop` session via the XDG portal, negotiates a libei socket, handshakes as a sender context, and emits input through the compositor's own libei implementation. Full focus awareness and permission prompts. **Preferred on GNOME and KDE.**

**Requirements:**
- `xdg-desktop-portal` + a backend that exports `org.freedesktop.portal.RemoteDesktop`. `xdg-desktop-portal-gnome` (GNOME 46+) and `xdg-desktop-portal-kde` (KDE Plasma 6) both ship it.
- On Hyprland, `xdg-desktop-portal-hyprland` 1.3.11 does **not** yet expose RemoteDesktop. Use the wlroots backend instead.

### wlroots

Binds `zwp_virtual_keyboard_v1`, `zwlr_virtual_pointer_v1`, and `zwlr_foreign_toplevel_management_v1` directly. Uploads a transient xkb keymap at `type` time (one keycode per unique character as a Level-0 Unicode keysym) — this is how arbitrary Unicode works without a server-side keymap change. Tracks `wl_output` modes so `motion_absolute` coordinates are real pixels on the primary output. **Preferred on Sway, Hyprland, river, Wayfire.**

**Requirements:**
- A wlroots-based compositor that exposes the three protocols above. Sway, Hyprland, river, and Wayfire all do by default.

### kde

Composes libei (for input) with KWin-scripting window management over D-Bus. Generates small JS snippets, hands them to `org.kde.KWin.Scripting.loadScriptFromText`, runs them, and receives results on a transient `com.wdotool.KdeBridge` D-Bus service the backend registers at startup. Same trick `kdotool` uses, adapted to both the Plasma 6 (`workspace.windowList`, `workspace.activeWindow`) and Plasma 5 (`workspace.clientList`, `workspace.activeClient`) APIs. **For Plasma 5/6 where you want window management alongside input.**

**Requirements:**
- `xdg-desktop-portal-kde` for the libei input path.
- KWin 5.22+ for the Scripting D-Bus interface.

### gnome

Pairs libei (for input, via GNOME's RemoteDesktop portal) with a companion GNOME Shell extension that exposes `ListWindows` / `GetActiveWindow` / `ActivateWindow` / `CloseWindow` on the session bus. GNOME Shell has no generic external window API, so the extension is mandatory for window management — without it, the detector falls back to bare libei automatically.

**Requirements:**
- `xdg-desktop-portal-gnome` for the libei input path (GNOME 46+ ships it).
- The `wdotool@wdotool.github.io` extension, installable from `packaging/gnome-extension/`:
  ```sh
  cp -r packaging/gnome-extension/wdotool@wdotool.github.io ~/.local/share/gnome-shell/extensions/
  # log out + log back in, then enable:
  gnome-extensions enable wdotool@wdotool.github.io
  ```
  The extension targets GNOME Shell 45–48. No background activity — it only handles D-Bus method calls from the wdotool CLI.

### uinput

Creates a virtual input device via `/dev/uinput` and writes raw `input_event` structs through the kernel. Compositor-agnostic — works on Wayland, X11, or a bare framebuffer session — but has no focus awareness and no window API. **Fallback for environments without libei or wlroots.**

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
| GNOME (46+)  | libei         | companion Shell extension (D-Bus) — [#2](https://github.com/cushycush/wdotool/issues/2) |
| KDE Plasma 6 | libei         | KWin scripting (D-Bus) — [#1](https://github.com/cushycush/wdotool/issues/1) |
| Anything else | uinput       | —              |

Backend selection is automatic based on `XDG_CURRENT_DESKTOP` and compositor hints (`SWAYSOCK`, `HYPRLAND_INSTANCE_SIGNATURE`, etc.). The detector tries the preferred backend first and falls through to alternatives if it fails to bootstrap — use `--backend` to force a specific one.

## Building

Requires Rust 1.75+ (for async-in-traits via `async_trait`). Builds cleanly on stable.

```sh
cargo build             # dev
cargo build --release   # ~4 MB stripped binary
cargo test              # unit tests
```

System libraries at build time: `libxkbcommon-dev` and `libwayland-dev` (Debian/Ubuntu names; same underlying libraries elsewhere).

## Known limitations

- Unicode `type` works fully on wlroots via transient-keymap injection. On libei, kde, and uinput it's best-effort — the compositor or kernel owns the keymap, so characters outside the active layout are skipped with a warning.
- Multi-output absolute pointer on wlroots uses whichever `wl_output` replied first. Fine for single-monitor setups; targeting a specific output would need a new CLI arg.
- GNOME window management (`search` / `windowactivate` / `windowclose`) ships a companion Shell extension at `packaging/gnome-extension/wdotool@wdotool.github.io/`. Unverified on a live GNOME session — see [issue #2](https://github.com/cushycush/wdotool/issues/2).
- KDE backend is implemented but unverified on a real Plasma session. See [issue #1](https://github.com/cushycush/wdotool/issues/1).
- No multi-seat handling — the first seat gets everything.
- `--clearmodifiers` on Wayland can't do xdotool's save-and-restore dance (we can't observe current modifier state); see the Usage note above.

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
    wlroots.rs       # virtual-keyboard/pointer + foreign-toplevel + wl_output
    kde.rs           # libei input + KWin-scripting window ops via D-Bus
    uinput.rs        # /dev/uinput fallback
    stub.rs          # PendingBackend placeholder
```

Every real backend runs on a dedicated OS thread because the underlying event streams (`reis::EiConvertEventStream`, `wayland_client::EventQueue`) aren't `Send`. Input ops are dispatched to those threads via channels.

See [`CHANGELOG.md`](CHANGELOG.md) for release notes.

## Contributing

Pull requests and issue reports welcome. Two open issues specifically want outside help:

- [#1](https://github.com/cushycush/wdotool/issues/1) — `help wanted`, `kde`, `needs-testing`. The KDE backend is implemented but has never been exercised against an actual Plasma session (project is maintained from Hyprland). Running `wdotool --backend kde info` on Plasma 5 or 6 and reporting the result would meaningfully de-risk the backend.
- [#2](https://github.com/cushycush/wdotool/issues/2) — `help wanted`, `gnome`, `enhancement`. GNOME window management needs a Shell extension; the Rust side follows the KDE pattern once the extension exists. Happy to pair on the design or review PRs.

For other bugs or features, open an issue first for anything non-trivial so the shape can be agreed before code lands.

## License

MIT OR Apache-2.0
