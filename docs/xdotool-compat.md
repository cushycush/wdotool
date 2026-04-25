# xdotool compatibility

wdotool is **not** a drop-in xdotool replacement.

The CLI surface aims to be argv-compatible for the operations most scripts use, so a `wdotool key ctrl+c` works the same as the xdotool equivalent. Beyond that, the parity gap widens fast: a lot of xdotool's surface depends on X11 concepts that do not exist on Wayland (interactive window pickers, global virtual-desktop manipulation, X11 properties), and a lot of the rest is window-state mutation that the Wayland security model intentionally does not expose to clients.

This page is the honest table. If you are porting a script and the command you want is `🚫 not planned`, you will need to find a non-xdotool way to do that thing.

## Status legend

- ✅ shipped: works today on at least one backend
- 🧪 partial: implemented but with caveats (limited match types, single-monitor only, etc.)
- ❌ deferred: would work on Wayland but isn't built yet; will land on demand
- 🚫 not planned: blocked by Wayland's design or by xdotool semantics that don't translate

## Input

| xdotool command | wdotool | notes |
| --- | --- | --- |
| `key`, `keydown`, `keyup` | ✅ | All five backends |
| `type` | ✅ | Full Unicode on wlroots via transient keymap; ASCII-only fallback elsewhere (the EIS server or the kernel owns the keymap) |
| `mousemove` | ✅ | Both absolute and relative |
| `mousemove_relative` | ✅ | `wdotool mousemove --relative` |
| `click` | ✅ | xdotool indices: 1=left, 2=middle, 3=right, 8=back, 9=forward |
| `mousedown`, `mouseup` | ✅ |  |
| `mousewheeldown`, `mousewheelup` | ✅ | Use `wdotool scroll dx dy` |
| `getmouselocation` | ❌ | Pure send-side surface today; no read of pointer position. Open an issue if you need it |

## Window actions

| xdotool command | wdotool | notes |
| --- | --- | --- |
| `windowactivate` | ✅ | wlroots / kde / gnome backends |
| `windowclose` | ✅ |  |
| `windowkill` | ✅ | Same as `windowclose` on Wayland |
| `windowfocus` | ❌ | xdotool distinguishes focus from activate; wdotool currently only activates |
| `windowmove`, `windowsize` | 🚫 | Wayland clients can't reposition or resize other windows; that's the compositor's job. Talk to your compositor's IPC (sway-msg, hyprctl, kwriteconfig) |
| `windowmap`, `windowunmap`, `windowminimize`, `windowraise` | 🚫 | Same reason |
| `windowstate` | 🚫 | Same reason |
| `windowreparent` | 🚫 | X11 reparenting concept does not exist on Wayland |

## Window queries

| xdotool command | wdotool | notes |
| --- | --- | --- |
| `search` | ✅ | `--name` (title), `--class` (Wayland app_id), `--pid`. Substring matching by default; pass `--regex` for full regex semantics, `--ignore-case` for case-insensitive. Exits 1 if no matches (matches xdotool's behavior). xdotool's `--role`, `--classname`, `--screen`, `--desktop`, `--all`, `--any` aren't implemented |
| `getactivewindow` | ✅ | Returns the focused window's id |
| `getwindowfocus` | ✅ | Same as `getactivewindow` on Wayland (Wayland does not expose pointer-focus separately from keyboard-focus to clients) |
| `getwindowname` | ✅ | Prints the title of a window by id. Pair with `wdotool search` to get an id |
| `getwindowpid` | ✅ | Prints the PID of a window by id. Exits 1 if the backend can't resolve a PID for that window (some compositors don't expose it) |
| `getwindowclassname` | ✅ | Prints the Wayland app_id of a window by id (the closest equivalent to X11's WM_CLASS classname). Exits 1 if no app_id is set |
| `getwindowgeometry` | ❌ | Compositor-dependent; wlroots can do it through foreign-toplevel, KWin via scripting. Not built yet |

## Selection / interactive UI

| xdotool command | wdotool | notes |
| --- | --- | --- |
| `selectwindow` | 🚫 | xdotool's interactive picker is an X11 grab. Wayland's security model doesn't allow grabbing arbitrary windows. Closest equivalent: `slurp` for region picking, or a compositor-specific picker |
| `behave`, `behave_screen_edge`, `behave_screen_corner` | 🚫 | xdotool's event-callback hooks are X11-specific; the Wayland equivalents are compositor-specific (Hyprland binds, KWin scripts, GNOME extensions) |

## Workspace / desktop

xdotool's desktop ops assume a global virtual-desktop concept (NETWM `_NET_CURRENT_DESKTOP`). Wayland has no equivalent at the protocol level; each compositor decides whether to expose workspaces and how. None of these are planned.

| xdotool command | wdotool | notes |
| --- | --- | --- |
| `set_desktop`, `get_desktop` | 🚫 | Use compositor IPC (`sway-msg workspace`, `hyprctl dispatch workspace`, KWin scripts) |
| `get_num_desktops`, `set_num_desktops` | 🚫 | Same |
| `set_desktop_for_window`, `get_desktop_for_window` | 🚫 | Same |
| `set_desktop_viewport`, `get_desktop_viewport` | 🚫 | NETWM concept; Wayland compositors don't expose this |

## X11-only properties

| xdotool command | wdotool | notes |
| --- | --- | --- |
| `set_window`, `getwindowname`-via-properties | 🚫 | wdotool can't set X11 properties on Wayland windows because the windows aren't X11 windows. XWayland clients have an X11 window backing them, but reaching it requires a different tool entirely (`xprop`, `xdotool` itself) |

## Convenience commands

xdotool ships a few shell-helper commands that don't map cleanly to a Wayland tool. wdotool deliberately doesn't reimplement them; just use the shell.

| xdotool command | wdotool | use instead |
| --- | --- | --- |
| `sleep` | 🚫 | Shell `sleep` |
| `exec` | 🚫 | Shell |
| Chained commands (`xdotool cmd ; cmd`) | 🚫 | Shell pipes / chains |

## What about features wdotool has that xdotool doesn't

- `wdotool diag` and `wdotool diag --copy` for environment introspection and bug-report capture.
- `wdotool capabilities` for structured (JSON) introspection of what this build supports. Schema at [`capabilities-schema.json`](capabilities-schema.json).
- A library API (`wdotool-core` on crates.io) for embedding the engine in other Rust tools.
- Per-backend Cargo features so library consumers can drop the backends they don't need (uinput especially, for sandboxed builds).
- Portal `restore_token` caching so libei users don't see a consent dialog on every command.

## Filing missing-command requests

If you find a `❌ deferred` entry that you actually need to port a script, [open an issue](https://github.com/cushycush/wdotool/issues) with:

1. The xdotool command you're trying to replace.
2. A short example of how you use it.
3. What Wayland session you're on (run `wdotool diag --copy` and paste).

The deferred items are deferred because nobody has hit them yet, not because they're hard.
