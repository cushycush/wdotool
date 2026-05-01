# Testing wdotool

Input automation is hard to test because the thing under test is "did the OS receive and process the synthetic input correctly," and the answer to that question depends on a real compositor with real focus. The way wdotool handles this is to layer the tests so most of them can run in CI on any machine, and only the last layer needs a real Wayland session.

This document walks through what each layer covers and where to look when you add a new command.

## Layer 1: unit tests for parsing and translation

Every part of wdotool that's pure deterministic Rust gets unit tests in the same file as the code. The keysym chain parser, the search filter compiler, the recorder mapping logic, the capabilities JSON schema generator, the portal token cache. About 60 tests at time of writing, all running under `cargo test` with no compositor.

This layer catches bugs in input validation, modifier ordering at the parsing stage, regex compilation, JSON serialization. It runs in CI on Linux, macOS, and Windows, so any pure-Rust change touching one of these modules has a tight feedback loop.

If you add a new pure-logic helper, put its tests in a `#[cfg(test)] mod tests` block at the bottom of the same file. Look at `wdotool-core/src/keysym.rs` for the established pattern.

## Layer 2: mock-backend integration tests

Every CLI subcommand has integration tests in `wdotool/tests/cli_*.rs` that drive `dispatch()` against a `MockBackend` and assert on the exact sequence of `Backend` trait calls the dispatch produced. About 55 tests, four files split by command family (`cli_keyboard.rs`, `cli_pointer.rs`, `cli_window.rs`, `cli_introspection.rs`) plus `cli_smoke.rs` that just sanity-checks the harness.

This is where modifier ordering bugs surface. A test for `wdotool key ctrl+shift+a` asserts that the mock sees `Press(Control_L), Press(Shift_L), PressRelease(a), Release(Shift_L), Release(Control_L)` in that exact order. A test for `wdotool type "hello 世界"` asserts that the multi-byte string reaches the backend untransformed. A test for `wdotool mousemove --output DP-2 50 60` asserts the CLI consults `list_outputs()` and then calls `mouse_move(1970, 60, absolute=true)` if `DP-2` has its origin at `(1920, 0)`.

It's also where the "exit code 1 on no match" contract gets pinned. `dispatch()` returns a structured `ExitCode` instead of calling `process::exit`, so a `wdotool search` that finds no windows just returns `ExitCode::FAILURE`. Tests assert on the value and keep running.

The `MockBackend` lives in `wdotool-core/src/backend/mock.rs` behind the `testing` feature so wflows.com and other downstream consumers of the `Backend` trait can reuse it without re-deriving the harness. It records every call as a `BackendCall` enum variant and returns canned data for the read-side methods (`active_window`, `list_windows`, `list_outputs`, `pointer_position`, `window_geometry`) that tests configure via `set_*` setters.

If you add a new subcommand, write its tests in the matching `cli_*.rs` file using the `common::run(argv)` helper. The helper parses `argv` through clap, drives `dispatch()` over a fresh `MockBackend`, and returns the recorded calls plus captured stdout/stderr plus the exit code. Look at `cli_keyboard.rs` for the established pattern.

This layer doesn't catch bugs in the actual backends. Whether wlroots correctly translates `Press(Control_L)` into a real keyboard event the compositor delivers to the focused window is a question for the next layer.

## Layer 3: headless-compositor harness

A small Wayland observer client (`wdotool-observer`, in the `wdotool-test-harness` crate) opens a surface, takes the seat, and writes one line per received input event to stdout. The runner (`HeadlessSway` in the same crate) spawns sway with `WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1` in a private `XDG_RUNTIME_DIR`, runs the observer inside it, and shells out to `wdotool` against the same display. Tests assert on the observer's captured event stream.

This layer catches the bugs Layer 2 can't reach: transient-keymap-injection bugs on wlroots, modifier-state divergence between what wdotool thinks is pressed and what the compositor actually delivers, scroll axis sign and discrete-vs-smooth handling, the focus model around `windowactivate` followed by `key`. It's also what makes `wdotool record` round-trippable: capture events, replay them, assert the replay matches.

The harness is shipped (issue [#30](https://github.com/cushycush/wdotool/issues/30)) with twelve round-trip tests in `wdotool/tests/cli_roundtrip.rs`. Four pass against real headless sway: surface-becomes-ready, `wdotool key a`, `wdotool key ctrl+shift+a` (the modifier-ordering test that already caught a real bug, see below), and `wdotool type "hello"`. The tests skip themselves with a friendly "install sway" message when sway isn't on `PATH`, so the suite stays green on machines without sway.

Output format from the observer is one line per event, space-separated, designed for grep-friendly asserts: `key 38 a press`, `pointer_motion 100.0 200.0`, `pointer_axis vertical 1.0`, and so on. The observer's xkb integration resolves linux-evdev keycodes to keysym names so test assertions stay layout-independent.

The first run of the round-trip suite caught a real wdotool bug. Without a roundtrip after each input event, the wlroots backend would return from `do_mouse_move` / `do_key` / etc. immediately after `flush()`, the wdotool process would exit, the virtual_keyboard / virtual_pointer would be destroyed, and any in-flight events the compositor hadn't processed yet would be silently dropped. The fix is in `wdotool-core/src/backend/wlroots.rs`: each pointer / keyboard command in the worker loop now does a `queue.roundtrip()` before sending its reply. Layer 2 couldn't have caught this because the mock backend isn't a real Wayland connection.

Eight tests in the suite are `#[ignore]`d for a headless-sway-specific race: in a real desktop the seat already has pointer cap (real mouse plugged in), so observer clients are already bound when wdotool injects motion. In headless sway the seat starts with no pointer cap, sway broadcasts the cap event when wdotool's virtual_pointer is created, but sway processes wdotool's `motion_absolute` before the observer's `get_pointer` request reaches the compositor. Sway then has no pointer client to deliver the motion to and discards it. None of this affects production behavior; it only blocks one approach to automated round-trip testing of pointer events. The pre-release manual matrix in `docs/verification/` covers pointer behavior on real desktops; running `cargo test -- --ignored` will re-attempt the ignored tests once a workaround lands (a long-running prime process, libei backend in CI, or weston headless).

## Layer 4: KDE and GNOME

KDE and GNOME are hard to test in CI because they need a full desktop session with portal services running. There are three approaches and the right one depends on what you're testing.

For the KDE backend specifically, the `org.kde.KWin.Scripting` D-Bus interface is the only thing the backend talks to, and it's small enough to mock. Stand up a fake D-Bus service in a test, send the same JS strings wdotool sends, and assert their content. You're not verifying that KWin actually executes the JS, but you are verifying that wdotool generates correct JS for each window operation. Same shape works for the GNOME extension's D-Bus methods. This is closer to Layer 2 than Layer 3 in cost.

For end-to-end "does the KDE backend actually work on real KDE," there are Docker images that run full KDE/GNOME desktops via VNC. Slow and brittle for CI, occasionally useful for nightly or pre-release runs.

For real coverage, the existing approach via [issue #1](https://github.com/cushycush/wdotool/issues/1) (KDE) and [issue #2](https://github.com/cushycush/wdotool/issues/2) (GNOME) is the right answer: ten people running it on their actual desktops catches more real bugs than any synthetic harness, and the matrix in `docs/verification/` gives them a structured checklist.

## Pre-release manual matrix

A handful of things really do need a human, and "does the active window actually receive the keystroke" is the canonical example. The verification docs in `docs/verification/` are the per-platform checklists. Run through them before tagging a release, paste the output into the tracking issue, file bugs for anything that breaks.

## Running everything locally

The `cargo test --workspace` command runs Layers 1 and 2 plus the Layer 3 framework (which skips when sway isn't installed) and finishes in under a second on a warm cache. That's the bar for any PR: the existing tests stay green, and any new behavior gets a Layer 2 test before it lands.

When sway is installed, the Layer 3 round-trip suite adds about a second per test. Layer 4 manual matrix is a release-time activity.

## When you add a new subcommand

Three places to touch:

The CLI definition in `wdotool/src/cli.rs` and the dispatch arm in `wdotool/src/lib.rs` are required, same as before. Add a Layer 2 test in the right `cli_*.rs` file: assert the exact backend call sequence the dispatch produces, assert the formatted output if the command writes to stdout, assert the exit code matches xdotool's behavior. Add a Layer 1 unit test for any pure-logic helper the dispatch arm calls into. The dispatch arm itself is rarely complex enough to need its own unit test once it has a Layer 2 test.

If the command has a behavior that only manifests against a real compositor (key events actually reaching the focused window, mousemove with focus follows mouse, scroll deltas accumulating), add a row to the verification matrix for the next pre-release pass. When Layer 3 lands, those rows can move from "human checks this" to "harness asserts this."
