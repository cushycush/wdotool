# KDE Plasma 6 verification matrix

This is the smoke-test checklist that closes [issue #1](https://github.com/cushycush/wdotool/issues/1). The KDE backend has shipped since v0.1.x but I've never run it on a real Plasma session, because I'm on Hyprland and won't dual-boot just to verify it. Until someone walks this matrix on real KDE hardware, "I think it works" is the strongest claim I can make about the backend, which is a bad claim to ship under.

If you have a Plasma 6 box (laptop, desktop, dual-boot, or a borrowed loaner) the whole walk-through is roughly 30 to 45 minutes if everything passes. Longer if you find bugs and write good repro steps for them. That's the favor I'm asking for, and the PR you'd open at the end is what closes the issue.

## Setup

Plasma 6 on a Wayland session. Either install one of the release artifacts (DEB / RPM from the latest release, AUR if you're on an Arch-flavored distro), or build from source.

```sh
# build from source (workspace root)
cargo build --release
./target/release/wdotool diag --copy
```

Paste the output of `wdotool diag --copy` into a new comment on issue #1. That's your starting state. The matrix below is what you fill in next.

You'll want a scratch text editor open (Kate, Konsole running `cat > /tmp/scratch`, or any GUI editor) for the input ops, and a couple of windows arranged so you can see them all at once.

## Conditions

Six conditions to test against. They cover the bug classes most likely to misbehave on KDE specifically.

1. **Default.** Plasma 6's out-of-the-box settings, single monitor, 100% scale, no input method.
2. **Fractional 125%.** System Settings → Display → set scale to 125%. KWin's fractional-scale-v1 path. Most common scaling setting and the one most apps render correctly at.
3. **Fractional 175%.** Same setting, scale to 175%. Where rounding errors stack up worst, which is a known bug class on KDE.
4. **Mixed-scale dual-monitor.** Two displays, one at 100% and one at 200%. The known KDE coordinate bug class. If you only have a single monitor, mark this row N/A and note it.
5. **Wayland session restart.** Log out, log back in (Wayland session, same user). Tests that the cached portal token survives, or that the recovery flow re-prompts cleanly.
6. **Fcitx5 active.** Install fcitx5 plus a non-trivial input method (anything that intercepts key events: pinyin, mozc, hangul). Activate it before running the input ops. Real users have IMEs and they intercept events in ways that have bitten Wayland tools before.

You don't have to test in this exact order. If you swap conditions, just note which one you're under in each cell.

## How to fill in the matrix

Each cell takes one of:

- ✅ **PASS** if the operation did the expected thing
- ❌ **FAIL** if it didn't, with a one-line repro under the master table
- N/A if you couldn't test the combination (no second monitor, etc.)

Edit this file in a fork or a branch, fill in the matrix, open a PR. That PR is what closes issue #1.

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

Should print `backend: kde`. If KWin scripting fails to connect, it'll fall back to `backend: libei`, which means input still works but the window-management rows will all be N/A. Capability lines should all read `true` except the window-mgmt ones in the fallback case. If anything errors before the capabilities table, paste the full output.

### `key` (ASCII chord)

Open a text editor, focus its text area, then:

```sh
sleep 2 && wdotool key ctrl+a
```

(The sleep gives you time to refocus the editor after running the command in another terminal.)

All text in the editor should select. If it doesn't, note whether the key event registered at all (try `wdotool key a` and see if `a` types) or whether modifier handling specifically broke.

### `type` (ASCII text)

```sh
sleep 2 && wdotool type "hello kde"
```

"hello kde" should appear in the focused editor. If some characters arrive but not others, or all arrive but in the wrong layout, write down whatever shows up.

### `type` (Unicode)

```sh
sleep 2 && wdotool type "中文 · ❤️"
```

Either all three glyphs appear, or the ASCII subset appears with a warning. The KDE backend uses libei for input, which doesn't own the keymap; the documented behavior is "characters not in the active layout are skipped with a warning" (`type_unicode: ascii_only` in the capabilities schema). Anything else (crash, output that doesn't match what `wdotool type` printed) is a fail.

### `mousemove` (absolute)

```sh
wdotool mousemove 500 400
```

Cursor should jump to (500, 400). On the mixed-scale row, note which monitor it landed on. Mixed-scale coordinates with KDE are exactly the bug class I'm hunting.

### `mousemove` (relative)

```sh
wdotool mousemove --relative 100 50
```

Cursor should move 100 right and 50 down from wherever it currently is. Direction inverted, distance off, or no movement are all fails.

### `click`

```sh
sleep 2 && wdotool click 1
```

Whatever the cursor is over should get clicked (focus changes, button activates, etc.). If nothing happens, try `click 3` and see if the right-click menu appears, which tells me whether it's a button-mapping issue or a click-not-firing issue.

### `mousedown` / `mouseup`

```sh
sleep 2 && wdotool mousedown 1
# move the cursor manually with the trackpad/mouse
sleep 2 && wdotool mouseup 1
```

You should be able to drag-select text or move a window title bar. The two failure modes I care about: the button gets stuck (held forever after the second command), or never registers as held in the first place.

### `scroll`

```sh
sleep 2 && wdotool scroll 0 3
```

Whatever the cursor is over should scroll down by ~3 ticks. Wrong direction (positive dy should be DOWN per the README), wrong amount, or no scroll at all are fails.

### `search`

```sh
wdotool search --name "konsole"   # adjust to whatever's actually open
```

Should print one line per matching window: `<id>\t<title>`. Empty output even though a matching window is visible, or an error, are fails.

### `getactivewindow`

Click on a known window first.

```sh
wdotool getactivewindow
```

Should print the focused window's id. Empty output or the wrong window's id are fails.

### `windowactivate`

```sh
id=$(wdotool search --name "konsole" | head -1 | cut -f1)
wdotool windowactivate "$id"
```

The named window should come to the front and get focus. Window didn't change, or wdotool errored, are fails.

### `windowclose`

```sh
id=$(wdotool search --name "scratch-test-window" | head -1 | cut -f1)
wdotool windowclose "$id"
```

(Pick an expendable window before running, obviously.)

The window should close. If it stayed open, or KWin showed a "Force Quit?" dialog instead of a clean close, that's a fail.

## Special tests

### Token revoke + recovery

This exercises the portal token cache from PR #6. Run it under the Default condition.

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

Pass means steps 1 and 3 prompt; steps 2, 4-after-allow, and 5 are silent. The cache file at `~/.local/state/wdotool/portal.token` (mode 0600) should update between steps 1 and 4.

The two most likely failure modes: step 4 doesn't re-prompt and just errors out (the recovery flow is broken), or step 4 prompts but then step 5 keeps prompting (the new token isn't getting saved). Note which step misbehaved.

### wflow 5-step workflow

Once wflow itself migrates to `wdotool-core` as a library, this test becomes important: a real workflow should fire one consent dialog and then run silently. Until that wflow PR lands, this row is N/A.

If you're running wflow off a branch that's already migrated:

```sh
# pseudo-workflow: open Konsole, type a command, click, focus a window, close it
wflow run path/to/test-workflow.kdl
```

Pass means exactly one consent dialog at the start, all five steps succeed, no further prompts.

## After you finish

Edit this file with your filled-in matrix (PASS / FAIL / N/A in each cell, details under the table for any failure), and open a PR referencing issue #1. The PR closes when the matrix is filled in and any failures have either been fixed or filed as their own issues.

If everything passes, that's strong enough signal to mark the KDE backend "verified" in the README and remove the issue-#1 disclaimer. If there's a mix of pass and fail, I'll file the failures as separate issues and ship the next release with the verified parts called out and the failed ones marked.
