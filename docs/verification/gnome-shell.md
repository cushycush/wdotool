# GNOME Shell verification matrix

This is the smoke-test checklist that closes [issue #4](https://github.com/cushycush/wdotool/issues/4). The GNOME backend has shipped since v0.1.6 (the libei input half) and v0.3.x (the Shell extension for window ops and pointer position) but has never been smoke-tested on a real GNOME Shell session, because the project is maintained from Hyprland. Until someone runs through this matrix on real GNOME hardware, "we think it works" is the strongest claim we can make about it.

Anyone with a GNOME 45+ Wayland session (Fedora Workstation, Ubuntu, Debian, openSUSE GNOME spin, anything that ships Mutter as the compositor) can fill this in. The whole walk-through is roughly 30 to 45 minutes if everything passes; longer if you find bugs and write good repro steps for them.

## Setup

GNOME Shell 45, 46, 47, or 48 on a Wayland session. Either install one of the release artifacts (DEB / RPM from the latest release, AUR if you're on an Arch-flavored distro) or build from source.

```sh
# build from source (workspace root)
cargo build --release
./target/release/wdotool diag --copy
```

Paste the output of `wdotool diag --copy` into a new comment on issue #4. That's your starting state. Then install the companion Shell extension (window ops and pointer position need it; without it the backend silently falls back to bare libei and the window-management half of the matrix is N/A):

```sh
mkdir -p ~/.local/share/gnome-shell/extensions
cp -r packaging/gnome-extension/wdotool@wdotool.github.io \
      ~/.local/share/gnome-shell/extensions/

# Wayland sessions can't reload extensions live. Log out, log back in.
gnome-extensions enable wdotool@wdotool.github.io
gnome-extensions info wdotool@wdotool.github.io   # should print STATE: ENABLED

# Confirm the bridge is on the session bus:
busctl --user list | grep wdotool
# expected: org.wdotool.GnomeShellBridge ...
```

You'll want a scratch text editor open (gedit, GNOME Text Editor, Files' rename dialog, or any GUI editor) for the input ops, and a couple of windows arranged so you can see them all at once.

## Conditions

Six conditions to test against. They cover the bug classes most likely to misbehave on GNOME specifically.

1. **Default** — GNOME's out-of-the-box settings, single monitor, 100% scale, no input method.
2. **Fractional 125%** — Settings → Displays → set scale to 125%. Mutter's fractional-scale-v1 path. Most common scaling setting and the one most apps render correctly at. (If your distro doesn't expose fractional scaling by default, `gsettings set org.gnome.mutter experimental-features "['scale-monitor-framebuffer']"` turns it on.)
3. **Fractional 175%** — Same setting, scale to 175%. Where rounding errors stack up worst.
4. **Mixed-scale dual-monitor** — Two displays, one at 100% and one at 200%. If you only have a single monitor available, mark this row N/A and note it.
5. **Wayland session restart** — Log out, log back in (Wayland session, same user). Tests that the cached portal token survives or that the recovery flow re-prompts cleanly, and that the Shell extension is still on the bus.
6. **IBus active** — Activate IBus with a non-trivial input method (anything that intercepts key events: pinyin, mozc, hangul). IBus is GNOME's default IME stack and intercepts events differently from fcitx5; real users have IMEs and they have bitten Wayland tools before.

You don't have to test in this exact order. If you swap conditions, just note which condition you're under in each cell.

## How to fill in the matrix

Each cell takes one of:

- ✅ **PASS** — operation did the expected thing
- ❌ **FAIL** — didn't work; write a one-line repro under the master table
- N/A — couldn't test this combination (e.g. no second monitor, or extension not installed)

Edit this file in a fork or a branch and submit a PR with the filled-in matrix and any repro details. That PR is what closes issue #4.

## Master matrix

| Operation | Default | Frac 125% | Frac 175% | Mixed-scale | Session restart | IBus |
| --- | --- | --- | --- | --- | --- | --- |
| `info` | | | | | | |
| `key` (ASCII chord) | | | | | | |
| `type` (ASCII text) | | | | | | |
| `type` (Unicode 中文 / emoji) | | | | | | |
| `mousemove` (absolute) | | | | | | |
| `mousemove` (relative) | | | | | | |
| `click` | | | | | | |
| `mousedown` / `mouseup` | | | | | | |
| `scroll` | | | | | | |
| `getmouselocation` | | | | | | |
| `search` | | | | | | |
| `getactivewindow` | | | | | | |
| `windowactivate` | | | | | | |
| `windowclose` | | | | | | |

Plus three special cells, run only once each rather than once per condition:

| Special test | Status | Notes |
| --- | --- | --- |
| Token revoke + recovery (run "Revoke" in Settings → Privacy → Screen Sharing after first run, then `wdotool key a` again, expect a new consent dialog) | | |
| Extension disable + re-enable (`gnome-extensions disable`, run a window op and confirm it errors cleanly, then `gnome-extensions enable` and confirm window ops come back without restarting Shell) | | |
| Cross-workspace activation (move a window to a different workspace, then `wdotool windowactivate <id>` from your active workspace; expect the window to be raised and the workspace switched) | | |

## What each operation should do

The exact command per row, what passing looks like, what to record if it fails.

### `info`

```sh
wdotool info
```

**Pass:** Prints `backend: gnome-ext` (or `backend: libei` if the Shell extension isn't installed or enabled, in which case window ops will be unavailable but input ops should still work). All capability lines say `true` except the window-mgmt lines if it fell back to bare libei.

**On fail:** Paste the full output. Probably an error before the capabilities table, or `backend: libei` when you expected `gnome-ext` (extension not installed or not enabled).

### `key` (ASCII chord)

Open a text editor, focus its text area.

```sh
sleep 2 && wdotool key ctrl+a
```

(The sleep gives you time to refocus the editor after running the command in another terminal.)

**Pass:** All text in the editor is selected.

**On fail:** Note whether the key event was registered at all (try `wdotool key a` and see if `a` types) or whether modifier handling specifically broke.

### `type` (ASCII text)

```sh
sleep 2 && wdotool type "hello gnome"
```

**Pass:** "hello gnome" appears in the focused editor.

**On fail:** Did some characters arrive but not others? Did all characters arrive but in the wrong layout? Note whatever shows up.

### `type` (Unicode)

```sh
sleep 2 && wdotool type "中文 · ❤️"
```

**Pass:** Either all three glyphs appear, or the ASCII subset appears with a warning. The GNOME backend uses libei for input, which doesn't own the keymap; the documented behavior is "characters not in the active layout are skipped with a warning" (`type_unicode: ascii_only` in the capabilities schema).

**On fail:** Crash, or output doesn't match what `wdotool type` printed.

### `mousemove` (absolute)

```sh
wdotool mousemove 500 400
```

**Pass:** The cursor jumps to coordinates (500, 400). On the mixed-scale row, note which monitor it landed on; mixed-scale coordinate handling is exactly the bug class we're hunting.

**On fail:** Cursor went somewhere unexpected, or didn't move at all.

### `mousemove` (relative)

```sh
wdotool mousemove --relative 100 50
```

**Pass:** Cursor moves 100 right and 50 down from its current position.

**On fail:** Direction inverted, distance off, or no movement.

### `click`

```sh
sleep 2 && wdotool click 1
```

**Pass:** Whatever the cursor is over gets clicked (focus changes, button activates, etc.).

**On fail:** No click registered, or wrong button (try `click 3` and see if a context menu appears).

### `mousedown` / `mouseup`

```sh
sleep 2 && wdotool mousedown 1
# move the cursor manually with the trackpad/mouse
sleep 2 && wdotool mouseup 1
```

**Pass:** Hold-and-drag works. You should be able to drag-select text or move a window title bar.

**On fail:** The button got stuck (held forever after the second command), or never registered as held.

### `scroll`

```sh
sleep 2 && wdotool scroll 0 3
```

**Pass:** Whatever the cursor is over scrolls down by ~3 ticks.

**On fail:** Wrong direction (positive dy should be DOWN per the README), wrong amount, or no scroll.

### `getmouselocation`

```sh
wdotool mousemove 500 400
wdotool getmouselocation
```

**Pass:** Prints `x:500 y:400` (or whatever coordinate is current). The GNOME backend reads pointer position via the Shell extension's `GetPointerPosition` method.

**On fail:** Wrong coordinates (especially under fractional scale), or an error about the bridge not being on the bus.

### `search`

```sh
wdotool search --name "gedit"   # adjust to whatever's actually open
```

**Pass:** Prints one line per matching window: `<id>\t<title>`. IDs are GNOME `MetaWindow.get_stable_sequence()` values, so they'll be small integers as strings.

**On fail:** Empty output even though a matching window is visible, or an error. If override-redirect popups (tooltips, dropdowns) leak in, that's also a fail; the extension filters them out.

### `getactivewindow`

Click on a known window first.

```sh
wdotool getactivewindow
```

**Pass:** Prints the focused window's id.

**On fail:** Empty output, or the wrong window's id.

### `windowactivate`

```sh
id=$(wdotool search --name "gedit" | head -1 | cut -f1)
wdotool windowactivate "$id"
```

**Pass:** The named window comes to the front and gets focus.

**On fail:** Window didn't change (focus stayed elsewhere), or wdotool errored.

### `windowclose`

```sh
id=$(wdotool search --name "scratch-test-window" | head -1 | cut -f1)
wdotool windowclose "$id"
```

(Pick an expendable window before running.)

**Pass:** The window closes.

**On fail:** Window stayed open, or GNOME showed an "Application is not responding" dialog instead of a clean close.

## Special tests

### Token revoke + recovery

This exercises the portal token cache from PR #6. Run it under "Default" condition.

```sh
# 1. First run prompts for consent.
wdotool key a
# (consent dialog appears, click Allow. Token is saved.)

# 2. Second run is silent (token cached).
wdotool key a
# (no dialog, "a" types into focused window.)

# 3. Open Settings → Privacy → Screen Sharing (or Remote Desktop, naming
#    varies by GNOME version), find the wdotool grant, revoke it.

# 4. Third run should re-prompt (the recovery flow detects the
#    revoked token and runs the consent dialog again).
wdotool key a
# (consent dialog appears again. Click Allow. New token cached.)

# 5. Fourth run is silent again.
wdotool key a
```

**Pass:** Steps 1, 3 prompt; steps 2, 4-after-allow, 5 are silent. The cache file at `~/.local/state/wdotool/portal.token` (mode 0600) updates between steps 1 and 4.

**On fail:** Note which step misbehaved. Most likely failure modes: step 4 doesn't re-prompt and just errors out (recovery flow is broken), or step 4 prompts but then step 5 keeps prompting (the new token isn't getting saved).

### Extension disable + re-enable

Tests that the bridge cleanly tears down and comes back without restarting Shell, which matters for users who toggle extensions during a session.

```sh
# Window ops work.
wdotool search

# Disable.
gnome-extensions disable wdotool@wdotool.github.io

# Window op should now error cleanly with a message about the extension
# not being installed or enabled.
wdotool search

# Re-enable.
gnome-extensions enable wdotool@wdotool.github.io

# Window ops should work again, no Shell restart needed.
wdotool search
```

**Pass:** Step 1 lists windows, step 3 errors with a useful message (not a crash, not a hang), step 5 lists windows again.

**On fail:** Hang on step 3 (proxy doesn't time out), or step 5 needs a Shell restart to recover (D-Bus name not properly re-registered on enable).

### Cross-workspace activation

GNOME organizes windows into workspaces; activating a window on a different workspace should switch you to that workspace AND raise the window. This is one of the most common scriptable flows ("focus my browser, no matter where it is") and the most likely bug class for the Shell-extension half.

```sh
# Open a window, move it to workspace 2 (Super + Shift + Page Down by default).
# Switch back to workspace 1 (Super + Page Up).

id=$(wdotool search --name "gedit" | head -1 | cut -f1)
wdotool windowactivate "$id"
```

**Pass:** You're now on workspace 2, with the gedit window focused.

**On fail:** Workspace didn't change but the window says it's "activated" (lying), or the extension errored, or focus went to the wrong window.

## After you finish

Edit this file with your filled-in matrix (PASS / FAIL / N/A in each cell, and details under the table for any failure), and open a PR. Reference issue #4 in the PR description. The PR closes when the matrix is filled in and any failures have either been fixed or filed as their own issues.

If everything passes, that's strong enough signal to mark the GNOME backend "verified" in the README and remove the issue-#4 disclaimer. If there's a mix of pass and fail, we file the failures as separate issues and ship the next release with the verified parts called out and the failed ones marked.
