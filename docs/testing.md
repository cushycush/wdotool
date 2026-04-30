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

## Layer 3: headless-compositor harness (not yet built)

A tiny Wayland client built on `smithay-client-toolkit` that opens a surface, listens for input events on its own seat, and writes received events to stdout in a stable format. The test runner spawns Sway in `WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1` mode, runs the harness inside it, runs `wdotool` against it, and asserts that the input the harness reports matches the expected sequence.

This is the layer that catches the bugs Layer 2 can't reach: transient-keymap-injection bugs on wlroots, modifier-state divergence between what wdotool thinks is pressed and what the compositor actually delivers, scroll axis sign and discrete-vs-smooth handling, the focus model around `windowactivate` followed by `key`. About a day's work to write the harness, then maybe an hour per command to add a round-trip test.

This is also the layer that makes `wdotool record` round-trippable: a CI job can record events, replay them, and assert the recorded stream matches the replayed stream within tolerance. That closes the recorder loop without needing a human.

The headless harness is the unlock for everything else. Until it exists, the questions of "does my actual key event reach the compositor" and "does it reach the right window" are answered by humans on real desktops. After it exists, both questions are CI-asserted on every PR.

If you're picking up this layer, start with the harness alone, get it printing what it observes, and only then start writing round-trip tests against it. Issue tracker: not filed yet. Open one if you want to claim it.

## Layer 4: KDE and GNOME

KDE and GNOME are hard to test in CI because they need a full desktop session with portal services running. There are three approaches and the right one depends on what you're testing.

For the KDE backend specifically, the `org.kde.KWin.Scripting` D-Bus interface is the only thing the backend talks to, and it's small enough to mock. Stand up a fake D-Bus service in a test, send the same JS strings wdotool sends, and assert their content. You're not verifying that KWin actually executes the JS, but you are verifying that wdotool generates correct JS for each window operation. Same shape works for the GNOME extension's D-Bus methods. This is closer to Layer 2 than Layer 3 in cost.

For end-to-end "does the KDE backend actually work on real KDE," there are Docker images that run full KDE/GNOME desktops via VNC. Slow and brittle for CI, occasionally useful for nightly or pre-release runs.

For real coverage, the existing approach via [issue #1](https://github.com/cushycush/wdotool/issues/1) (KDE) and [issue #2](https://github.com/cushycush/wdotool/issues/2) (GNOME) is the right answer: ten people running it on their actual desktops catches more real bugs than any synthetic harness, and the matrix in `docs/verification/` gives them a structured checklist.

## Pre-release manual matrix

A handful of things really do need a human, and "does the active window actually receive the keystroke" is the canonical example. The verification docs in `docs/verification/` are the per-platform checklists. Run through them before tagging a release, paste the output into the tracking issue, file bugs for anything that breaks.

## Running everything locally

The `cargo test --workspace` command runs Layers 1 and 2 (unit + mock-integration) and finishes in under a second on a warm cache. That's the bar for any PR: the existing tests stay green, and any new behavior gets a Layer 2 test before it lands.

Layer 3 will add maybe five minutes to a workspace test once it exists. Layer 4 manual matrix is a release-time activity.

## When you add a new subcommand

Three places to touch:

The CLI definition in `wdotool/src/cli.rs` and the dispatch arm in `wdotool/src/lib.rs` are required, same as before. Add a Layer 2 test in the right `cli_*.rs` file: assert the exact backend call sequence the dispatch produces, assert the formatted output if the command writes to stdout, assert the exit code matches xdotool's behavior. Add a Layer 1 unit test for any pure-logic helper the dispatch arm calls into. The dispatch arm itself is rarely complex enough to need its own unit test once it has a Layer 2 test.

If the command has a behavior that only manifests against a real compositor (key events actually reaching the focused window, mousemove with focus follows mouse, scroll deltas accumulating), add a row to the verification matrix for the next pre-release pass. When Layer 3 lands, those rows can move from "human checks this" to "harness asserts this."
