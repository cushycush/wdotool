# wdotool

Input automation for Wayland: a CLI (`wdotool`) and a Rust library (`wdotool-core`) other tools embed. The substrate that lets workflow tools (wflow), distros (Drift), and ad-hoc scripts simulate keystrokes / pointer events / window ops on Wayland sessions without going through ydotool's `/dev/uinput` end-run around the compositor.

## Stack

- **Rust 1.82+**, workspace with three crates (`wdotool-core` library, `wdotool` binary, `wdotool-test-harness` for headless-sway integration tests).
- **Five backends**, each on a dedicated OS thread (the libei / wayland event streams aren't `Send`):
  - `wlr-protocols` for Sway / Hyprland / river / Wayfire (zwp_virtual_keyboard, zwlr_virtual_pointer, zwlr_foreign_toplevel_management).
  - `libei` for the XDG RemoteDesktop portal (preferred on GNOME and KDE for input).
  - `kde` composes libei input with KWin scripting over D-Bus for window ops.
  - `gnome` composes libei input with a companion GNOME Shell extension for window ops.
  - `uinput` is the compositor-agnostic fallback.
- **`async_trait` Backend** is the single dispatch site every backend implements. Detector picks one based on `XDG_CURRENT_DESKTOP` + compositor hints + portal availability.

## Project layout

```
wdotool-core/                    library: the engine other Rust code consumes
  src/
    lib.rs                        public re-exports (Backend, DynBackend, detector, types, keysym, etc.)
    backend/
      mod.rs                       Backend trait (async_trait)
      detector.rs                  runtime backend selection
      libei.rs                     XDG RemoteDesktop portal + reis
      wlr_protocols.rs             virtual-keyboard/pointer + foreign-toplevel + wl_output
      kde.rs                       libei input + KWin scripting via D-Bus
      gnome.rs                     libei input + Shell extension D-Bus bridge
      uinput.rs                    /dev/uinput fallback
      mock.rs                      in-process test backend (testing Cargo feature)
    capabilities.rs               JSON schema for `wdotool capabilities`
    types.rs                      Capabilities, KeyDirection, MouseButton, WindowInfo, ...
    error.rs                      WdoError, Result
    keysym.rs                     ctrl+shift+a parser
    portal_token.rs               libei restore_token cache (no consent spam)
    recorder/                     capture: portal / evdev / simulated

wdotool/                         binary: the CLI
  src/
    main.rs                       tokio entrypoint
    lib.rs                        dispatch() under DispatchCtx { backend, env, stdout, stderr }
    cli.rs                        clap subcommand definitions
    diag.rs                       `wdotool diag` (env + backend availability)
    record.rs / replay.rs         the recorder CLI

wdotool-test-harness/            headless-sway integration runner + wdotool-observer

packaging/                       aur, flatpak, container, gnome-extension, npm
docs/                            verification matrices, xdotool-compat table, testing strategy
```

## Architecture invariants

- **Every backend implements `Backend`** in `wdotool-core/src/backend/mod.rs`. New backends slot in via `detector::build`; the CLI stays unchanged.
- **Per-backend types stay private.** The public API is `Backend`, `DynBackend`, `detector::{build, Environment, BackendKind}`, `keysym::parse_chain`, and the value types (`Capabilities`, `WindowInfo`, ...). wflow consumes this surface; treat it as semver-stable.
- **Every backend runs on its own OS thread.** Input ops dispatch via channels. Don't try to make `reis` or `wayland_client` event queues `Send` to skip this.
- **`dispatch()` is the CLI's single entry point** and returns a structured `ExitCode`. Tests drive it directly against the mock backend.
- **Capabilities reporting is a contract.** `wdotool capabilities --json` matches `docs/capabilities-schema.json`; the `@wdotool/capabilities` npm package's TypeScript types are generated from it. CI fails on drift.

## Building

```sh
cargo build              # dev (workspace root builds all three crates)
cargo build --release    # release binary at target/release/wdotool
cargo test               # unit + mock-backend integration tests (~135 tests)
```

System build deps: `libxkbcommon-dev` and `libwayland-dev` (Debian/Ubuntu names).

## Commit attribution

Plain commit messages, no `Co-Authored-By: Claude` trailers (global pref).

## Jira is the live planning layer

Project tracking lives in Jira (WDOTOOL project at `cushycush.atlassian.net`), alongside DRIFT and WFLOW on the same site. `CHANGELOG.md` is the release-history layer; `ACTIVE.md` is the session-handoff doc; Jira holds the live status. Three tiers: Epic > Task (umbrella) > Subtask.

The agent loop:

1. Run `python3 scripts/jira/seed.py lookup <path>` before starting work on a file. First matching glob in `scripts/jira/area-map.json` is the canonical ticket. If multiple match, prefer the most specific. If nothing matches, ask before continuing; the area-map needs an entry.
2. Transition to In Progress when work starts: `seed.py start WDOTOOL-XX --comment "what you're about to do"`. Skip the comment only if the ticket title already says exactly what you're doing.
3. Comment as you go: `seed.py comment WDOTOOL-XX "..."` when scope changes, when something ships, when a follow-up surfaces. Future-you reads these to remember why.
4. Transition to Done when the leaf is finished: `seed.py done WDOTOOL-XX --comment "shipped in <commit>"`. Don't move umbrella Tasks to Done until every Subtask under them is closed.
5. Carry the key on each commit as a `Refs: WDOTOOL-XX` or `Closes: WDOTOOL-XX` trailer. The post-commit hook posts the commit subject as a comment and transitions `Closes:` tickets to Done. Cross-project refs (`Refs: WFLOW-46`, `Refs: DRIFT-27`) also land on the right ticket.

Full reference at `scripts/jira/README.md`. Credentials live at `~/.config/jira/env` (chmod 600), shared with drift and wflow.

## Active session state

`ACTIVE.md` at the repo root is the session-handoff doc, updated before every session-end signal (logout, reboot, switching repos). Fresh sessions should read it before doing anything that might step on in-flight work.

## Verification matrices

Backends are smoke-tested per release on Hyprland (the maintainer's daily driver) and via the headless-sway integration harness. KDE and GNOME both have written verification checklists at `docs/verification/`; running them on real hardware is what closes issues #1 (KDE) and #4 (GNOME). The maintainer doesn't run either desktop, so these matrices double as community-contribution surfaces.

## How wdotool fits into the wider stack

wdotool is the input substrate for two consumer projects:

- **wflow** (`~/projects/wflow/`) is the macOS-Shortcuts-style workflow editor. It links `wdotool-core` from crates.io for the recorder module, and shells out to `wdotool` the binary for live workflow runs. Side-by-side compatible across versions.
- **Drift** (`~/projects/drift/`) is the Linux distro. wdotool is bundled in the ISO, the `drift` backend (planned) gives Drift's compositor a first-class slot alongside `kde` and `gnome`, and `wdotool diag` is the data source for a Drift Settings "Automation" panel.

A change to the public `wdotool-core` API is a coordinated bump with wflow; check that consumer before pushing a release.
