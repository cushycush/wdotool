# KDE Plasma 6 verification matrix

This is the smoke-test checklist that closes [issue #1](https://github.com/cushycush/wdotool/issues/1). The KDE backend has shipped since v0.1.x but has never been smoke-tested on a real Plasma session, because the project is maintained from Hyprland. Until someone runs through this matrix on real KDE hardware, "we think it works" is the strongest claim we can make about it.

Anyone with a Plasma 6 machine (laptop, desktop, dual-boot, or a borrowed loaner) can fill this in. The whole walk-through is roughly 30 to 45 minutes if everything passes; longer if you find bugs and write good repro steps for them.

## Setup

Plasma 6 + Wayland session. Either install one of the release artifacts (DEB / RPM from the latest release, AUR if you're on an Arch-flavored distro), or build from source.

```sh
# build from source (workspace root)
cargo build --release
./target/release/wdotool diag --copy
```

Paste the output of `wdotool diag --copy` into a new comment on issue #1. That's your starting state. The matrix below is what you fill in next.

You'll want a scratch text editor open (Kate, Konsole running `cat > /tmp/scratch`, or any GUI editor) for the input ops, and a couple of windows arranged so you can see them all at once.

## Conditions

Six conditions to test against. They cover the bug classes most likely to misbehave on KDE specifically.

1. **Default** — Plasma 6's out-of-the-box settings, single monitor, 100% scale, no input method.
2. **Fractional 125%** — System Settings → Display → set scale to 125%. KWin's fractional-scale-v1 path. The most common scaling setting and the one most apps render correctly at.
3. **Fractional 175%** — Same setting, scale to 175%. Where rounding errors stack up worst; a known bug class on KDE.
4. **Mixed-scale dual-monitor** — Two displays, one at 100% and one at 200%. The known KDE coordinate bug class. If you only have a single monitor available, mark this row N/A and note it.
5. **Wayland session restart** — Log out, log back in (Wayland session, same user). Tests that the cached portal token survives or that the recovery flow re-prompts cleanly.
6. **Fcitx5 active** — Install fcitx5 + a non-trivial input method (anything that intercepts key events: pinyin, mozc, hangul). Activate it before running the input ops. Real users have IMEs and they intercept events in ways that have bitten Wayland tools before.

You don't have to test in this exact order. If you swap conditions, just note which condition you're under in each cell.

## How to fill in the matrix

Each cell takes one of:

- ✅ **PASS** — operation did the expected thing
- ❌ **FAIL** — didn't work; write a one-line repro under the master table
- N/A — couldn't test this combination (e.g. no second monitor)

Edit this file in a fork or a branch and submit a PR with the filled-in matrix and any repro details. That PR is what closes issue #1.

## Master matrix

| Operation | Default | Frac 125% | Frac 175% | Mixed-scale | Session restart | Fcitx5 |
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
| `search` | | | | | | |
| `getactivewindow` | | | | | | |
| `windowactivate` | | | | | | |
| `windowclose` | | | | | | |

Plus two special cells, run only once each rather than once per condition:

| Special test | Status | Notes |
| --- | --- | --- |
| Token revoke + recovery (run "Revoke" in System Settings → Privacy → Remote Desktop after first run, then `wdotool key a` again, expect a new consent dialog) | | |
| wflow 5-step workflow (open Konsole, type a command, click, focus a window, close it; one consent dialog total, all 5 steps succeed) | | |

## What each operation should do

The exact command per row, what passing looks like, what to record if it fails.

### `info`

```sh
wdotool info
```

**Pass:** Prints `backend: kde` (or `backend: libei` if KWin scripting fails to connect, in which case window ops will be unavailable but input ops should still work). All capability lines say `true` except the window-mgmt lines if it fell back to bare libei.

**On fail:** Paste the full output. Probably an error before the capabilities table.

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
sleep 2 && wdotool type "hello kde"
```

**Pass:** "hello kde" appears in the focused editor.

**On fail:** Did some characters arrive but not others? Did all characters arrive but in the wrong layout? Note whatever shows up.

### `type` (Unicode)

```sh
sleep 2 && wdotool type "中文 · ❤️"
```

**Pass:** Either all three glyphs appear, or the ASCII subset appears with a warning. The KDE backend uses libei for input, which doesn't own the keymap; the documented behavior is "characters not in the active layout are skipped with a warning" (`type_unicode: ascii_only` in the capabilities schema).

**On fail:** Crash, or output doesn't match what `wdotool type` printed.

### `mousemove` (absolute)

```sh
wdotool mousemove 500 400
```

**Pass:** The cursor jumps to coordinates (500, 400). On the mixed-scale row, note which monitor it landed on; KDE coordinates with mixed scaling are exactly the bug class we're hunting.

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

**On fail:** No click registered, or wrong button (try `click 3` and see if right-click menu appears).

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

### `search`

```sh
wdotool search --name "konsole"   # adjust to whatever's actually open
```

**Pass:** Prints one line per matching window: `<id>\t<title>`.

**On fail:** Empty output even though a matching window is visible, or an error.

### `getactivewindow`

Click on a known window first.

```sh
wdotool getactivewindow
```

**Pass:** Prints the focused window's id.

**On fail:** Empty output, or the wrong window's id.

### `windowactivate`

```sh
id=$(wdotool search --name "konsole" | head -1 | cut -f1)
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

**On fail:** Window stayed open, or KWin showed a "Force Quit?" dialog instead of a clean close.

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

# 3. Open System Settings → Privacy → Remote Desktop, find the wdotool
#    grant, click "Revoke". This invalidates the cached token.

# 4. Third run should re-prompt (the recovery flow detects the
#    revoked token and runs the consent dialog again).
wdotool key a
# (consent dialog appears again. Click Allow. New token cached.)

# 5. Fourth run is silent again.
wdotool key a
```

**Pass:** Steps 1, 3 prompt; steps 2, 4-after-allow, 5 are silent. The cache file at `~/.local/state/wdotool/portal.token` (mode 0600) updates between steps 1 and 4.

**On fail:** Note which step misbehaved. The most likely failure modes are: step 4 doesn't re-prompt and just errors out (the recovery flow is broken), or step 4 prompts but then steps 5+ keep prompting (the new token isn't getting saved).

### wflow 5-step workflow

Once wflow itself migrates to `wdotool-core` as a library, this test becomes important: a real workflow should fire one consent dialog and then run silently. Until that wflow PR lands, this row is N/A.

If you're running wflow off a branch that's already migrated:

```sh
# pseudo-workflow: open Konsole, type a command, click, focus a window, close it
wflow run path/to/test-workflow.kdl
```

**Pass:** Exactly one consent dialog at the start, all five steps succeed, no further prompts.

## After you finish

Edit this file with your filled-in matrix (PASS / FAIL / N/A in each cell, and details under the table for any failure), and open a PR. Reference issue #1 in the PR description. The PR closes when the matrix is filled in and any failures have either been fixed or filed as their own issues.

If everything passes, that's strong enough signal to mark the KDE backend "verified" in the README and remove the issue-#1 disclaimer. If there's a mix of pass and fail, we file the failures as separate issues and ship v0.2.0 with the verified parts called out and the failed ones marked.
