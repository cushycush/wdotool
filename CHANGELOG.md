# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `wdotool getwindowgeometry <id>` reads a window's frame position and size and prints it in xdotool's default format (a `Window <id>` header line, a position line, a geometry line). Supported on KDE (kwin script reading `window.frameGeometry`) and GNOME (Shell extension calling `MetaWindow.get_frame_rect()`); the wlroots backend exits 1 with a clear stderr message because `zwlr_foreign_toplevel_management_v1` doesn't expose geometry, and libei / uinput exit 1 for the same window-concept reason. Capabilities schema's `extras` grew a `window_geometry` boolean reporting backend support.
- GNOME companion Shell extension: new `GetWindowGeometry` D-Bus method on `org.wdotool.GnomeShellBridge`. If you already have an older copy of the extension installed, reinstall from `packaging/gnome-extension/wdotool@wdotool.github.io/` to pick up the new method (older copies will return a D-Bus error on `getwindowgeometry`).
- `wdotool outputs` and `wdotool outputs --json` enumerate the compositor's monitors. Each row carries name (e.g. `DP-1`, `HDMI-A-1`), origin in compositor coordinates, logical size in pixels (post-scale), and the integer scale factor. Modern wlroots compositors put the real position on `xdg_output.logical_position` rather than `wl_output.geometry`, so the wlroots backend now binds `zxdg_output_manager_v1` and reads from there, falling back to `wl_output.geometry` on legacy compositors that don't expose xdg_output. Pair with the new `wdotool mousemove --output <name> X Y` to position the pointer in output-local coordinates instead of the global compositor space. Currently wlroots-only; KDE, GNOME, libei, and uinput exit 0 with empty output for `outputs` and reject `--output` with an error message naming the missing capability. Capabilities schema's `extras.outputs` boolean now reflects this per-backend (was always `false` before; flips to `true` on wlroots).
  - **Multi-output mousemove caveat:** `--output` correctly translates the user-supplied coordinates by adding the named output's logical-position offset, but the wlroots virtual-pointer protocol's `motion_absolute` request currently interprets coordinates relative to the primary output's mode dimensions. Cursor placement on non-primary outputs may land in the wrong place until the wlroots backend learns to create per-output virtual-pointer instances via `create_virtual_pointer_with_output`. Tracked at [issue #22](https://github.com/cushycush/wdotool/issues/22).
- `wdotool getmouselocation` reads the compositor's current pointer position and prints it as `x:N y:N` (xdotool's default format). First read-side input API in wdotool: every prior command was a send-side virtual-pointer or virtual-keyboard call. Supported on KDE (kwin script reading `workspace.cursorPos`) and GNOME (Shell extension calling `global.get_pointer()`); exits 1 with a clear stderr message on libei / wlroots / uinput because their Wayland protocols are send-only by design. Capabilities schema's `extras` grew a `pointer_position` boolean reporting backend support.
- GNOME companion Shell extension: new `GetPointerPosition` D-Bus method on `org.wdotool.GnomeShellBridge`. If you already have an older copy of the extension installed, reinstall from `packaging/gnome-extension/wdotool@wdotool.github.io/` to pick up the new method (older copies will return a D-Bus error on `getmouselocation`).

## [0.3.0] — 2026-04-26

### Added
- `wdotool search --any` flips filter combination from the default AND to OR. With multiple filters set (`--name`, `--class`, `--pid`), the default behavior requires every filter to match; `--any` accepts a window if at least one set filter matches. `--all` is also accepted as a no-op for xdotool argv compatibility, since it just names the existing default. The two flags are mutually exclusive (clap rejects with exit 2). With no filters set, `--any` falls through to "list everything" so that `wdotool search --any` and `wdotool search` behave the same with zero filters.
- `@wdotool/capabilities` npm package. TypeScript types and the JSON Schema document for the `wdotool capabilities` output, packaged so JS/TS consumers (wflows.com, dashboards, config UIs) can parse a capabilities report with full type safety. Exports `CapabilitiesReport` and the supporting types, an `isCapabilitiesReport` runtime guard, and the schema itself for use with ajv. Source under `packaging/npm/`. The schema is synced from `docs/capabilities-schema.json` at build time, and CI fails if the locked enums in `src/types.ts` drift from the schema. First publish is manual via the new `npm publish` workflow once the npm org is set up.
- `wdotool getwindowname <id>`, `wdotool getwindowpid <id>`, and `wdotool getwindowclassname <id>` round out the xdotool query surface. Each takes a window id (the same string `wdotool search` and `wdotool getactivewindow` print) and writes a single field to stdout: title, PID, or app_id (the Wayland equivalent of X11's WM_CLASS classname). Returns exit 1 if the id doesn't exist or if the backend can't resolve the requested field for that window. No new backend code: every backend that already populates `WindowInfo` from `list_windows` gets these for free.
- `wdotool search` grew real matchers. `--class` now works alongside the existing `--name` (filters on Wayland app_id, the closest equivalent to X11's WM_CLASS). `--pid` filters by exact process id. `--regex` switches `--name` and `--class` from substring matching to full regex; `--ignore-case` works in both modes. The capabilities schema's `window.match_by` field grew from `["title"]` to `["title", "app_id", "pid"]` accordingly. Exit code semantics now match xdotool: `wdotool search` returns 1 when nothing matches and 0 when it finds at least one window, so shell scripts can branch on `if wdotool search --name foo; then ...`.

## [0.2.0] — 2026-04-25

### Added
- `docs/verification/kde-plasma-6.md`. Smoke-test checklist for the KDE backend, designed to be filled in by anyone with a Plasma 6 machine. 13 operations × 6 conditions (default, fractional 125%, fractional 175%, mixed-scale dual-monitor, Wayland session restart, Fcitx5 active) plus two special tests for the token-revoke recovery flow and the wflow library integration. Each row has the command to run, what passes, and what to record on fail. Filling it in closes [issue #1](https://github.com/cushycush/wdotool/issues/1) and unblocks the KDE-verified claim in the README.
- `docs/xdotool-compat.md`. Honest parity table between xdotool and wdotool, grouped by category (input, window actions, window queries, workspace ops, X11-only). Each row marks the command as shipped, partial, deferred, or not planned, with a short reason. Replaces the implicit "drop-in replacement" promise the old README made.
- DEB and RPM packages for Debian / Ubuntu and Fedora / openSUSE / RHEL families. Built by a new `distros.yml` CI workflow that runs alongside cargo-dist's release on every tag push and uploads both artifacts to the same GitHub Release. Closes the install-friction gap for users on those distros, who previously had to either build from crates.io (requires Rust toolchain) or use the generic shell installer.
- Flathub manifest, .desktop file, and AppStream metainfo at `packaging/flatpak/`. App ID is `io.github.cushycush.wdotool`. The manifest builds locally; submission to flathub/flathub is a separate manual step the maintainer does. See `packaging/flatpak/README.md` for the steps.
- `wdotool diag` and `wdotool diag --copy`. Environment + backend availability report meant for bug triage. Probes pre-conditions only (XDG env vars, portal availability via `busctl`, GNOME extension presence, `/dev/uinput` writability, portal token cache state), so the diag run never opens a portal session and never pops a consent dialog. Markdown by default, `--json` for machine-readable output, `--copy` pipes the markdown through `wl-copy` (falling back to `xclip`).
- libei portal token cache. The first run prompts the user for consent. The portal-issued `restore_token` is cached at `$XDG_STATE_HOME/wdotool/portal.token` (mode 0600 set at create time via `OpenOptions::mode(0o600)`). Subsequent runs present the token and skip the dialog. The recovery flow detects token rejection and re-runs the consent flow without clobbering a still-valid cache on transient failures. Delete the cache file to force a fresh consent.
- Per-backend Cargo features (`libei`, `wlroots`, `kde`, `gnome`, `uinput`) on the new `wdotool-core` library crate. Default-on enables all five. Downstream Rust consumers can opt out: `default-features = false, features = ["libei", "wlroots", "kde", "gnome"]` drops uinput's `input-linux` and `libc` deps.

### Changed
- README opening reframed. wdotool is no longer described as an "xdotool-compatible CLI"; it's an input automation tool with both a CLI and a library API, and `wflow` is the first known library consumer. The "Why" section now points migrants at a new `docs/xdotool-compat.md` for the honest parity table instead of overpromising drop-in compatibility.
- Repo is now a Cargo workspace. The engine moved into `wdotool-core/` (a library crate); the `wdotool` binary is a thin clap wrapper that depends on `wdotool-core`. End-user behavior is unchanged. Other Rust projects can `cargo add wdotool-core` and call the engine directly instead of subprocessing the binary.
- `WdoError::Backend.source` type changed from `anyhow::Error` to `Box<dyn std::error::Error + Send + Sync>`. `wdotool-core` no longer pulls `anyhow` into its dependents.
- libei's `select_devices` switched from `PersistMode::DoNot` to `PersistMode::ExplicitlyRevoked` so the portal actually issues restore tokens.
- Workspace MSRV pin: Rust 1.82+ (the CLI uses `Option::is_none_or`).

## [0.1.6] — 2026-04-22

### Fixed
- wlroots `type`: control characters (`\n`, `\r`, `\t`, `\x08`, `\x7f`, `\x1b`) now emit via their semantic keysym (Return / Tab / BackSpace / Delete / Escape) through the transient keymap, rather than the raw Unicode control codepoint — most text widgets silently drop the latter. Fixes `wdotool type $'line1\nline2'` silently dropping the newline on Hyprland / Sway.

### Added
- wlroots `key U20AC` / `key €`: when the requested keysym isn't present in the user's active layout, fall back to transient-keymap injection (same mechanism as `type`). Applies to the press+release form; standalone `keydown` / `keyup` with Unicode still error, since swapping the keymap mid-chord would change the meaning of any held-down keycode.
- **Experimental:** `gnome` backend — libei input + window management via a companion GNOME Shell extension that exposes `ListWindows` / `GetActiveWindow` / `ActivateWindow` / `CloseWindow` on the session bus. Extension ships under `packaging/gnome-extension/wdotool@wdotool.github.io/` (targets GNOME Shell 45–48). If the extension isn't installed, `gnome` fails fast at init and the detector falls through to bare libei. Detector priority on GNOME is now GnomeExt → Libei → Wlroots, parallel to the KDE path. **Not yet dogfooded on a live GNOME session** — please try it and file issues ([#2](https://github.com/cushycush/wdotool/issues/2)).

### Internal
- `kde` backend: collapsed the duplicated `activate_window` / `close_window` scaffolding (waiter registration + timeout + error mapping) into a shared `call_action(what, build_script)` helper that generates the request id, builds the script with it, and returns the script's boolean result. Removes the `_keep_action_impl` dead-code placeholder.
- `kde` backend: target window ids are JSON-encoded when embedded in the generated KWin JavaScript, rather than via Rust's `{:?}` Debug formatter. Rust Debug emits `\u{XXXX}` for non-ASCII, which is Rust syntax, not JS. Current KWin ids are alphanumeric so this has been cosmetic in practice — the change is preventive.
- `kde` backend: first round of unit tests (3) covering script generation and the JSON-encoding helper.
- `uinput` backend: dropped the unused `start: Instant` field on `UinputBackend`, the dead `let _ = shift` placeholder in `type_text`, and the unused `char` carried through the resolutions vec. `resolve_keycode` is now a thin wrapper over `find_keysym` (the two previously duplicated the keymap walk). Unit test anchors the `MouseButton → evdev code` mapping so Back/Forward never silently swap.
- Removed `src/backend/stub.rs` and `Capabilities::none()`. Every backend kind now has a real implementation; the stub was only there to keep `detector::build_one` exhaustive while real backends landed, and its continued presence was a dead-code warning waiting to happen.

## [0.1.5] — 2026-04-22

### Added
- `wdotool type --file <path>`: read the text to type from a file. `--file -` reads from stdin. Mutually exclusive with the positional text arg.
- `--clearmodifiers` flag on `key`, `keydown`, `keyup`, and `type`. Approximates xdotool's flag — Wayland doesn't expose the compositor's current modifier state to clients, so this releases the standard set (Ctrl/Shift/Alt/Super/AltGr, L+R) unconditionally rather than doing xdotool's save-and-restore dance.

### Changed
- libei portal-missing errors now name the specific `xdg-desktop-portal-*` package to install per desktop (GNOME / KDE) and redirect Hyprland/Sway users to `--backend wlroots`.
- libei timeout errors call out the most likely cause (dismissed portal permission dialog).
- wlroots `NotSupported` errors explain which compositors expose `zwp_virtual_keyboard_v1` / `zwlr_virtual_pointer_v1` and suggest `--backend libei` for GNOME/KDE.

## [0.1.4] — 2026-04-22

### Added
- `wdotool-bin` AUR package — installs the prebuilt x86_64 binary from the GitHub Release (no compile). `provides=(wdotool)`, so conflicts cleanly with `wdotool` and `wdotool-git`.
- `wdotool-git` AUR package — rolling build from `main`, pkgver auto-derived via `git describe`.
- `flake.nix` with `packages.default`, `packages.wdotool`, and a `devShells.default` containing rustc/cargo/clippy/rustfmt/rust-analyzer. Linux only (x86_64 + aarch64).
- `crates-io.yml` workflow that runs `cargo publish` automatically on every tag push (needs `CARGO_REGISTRY_TOKEN` secret).
- `aur.yml` now runs as a matrix across `wdotool` and `wdotool-bin`, with a 15-minute retry loop on the binary tarball so the `-bin` package waits for cargo-dist's Release workflow to upload the artifact.

### Internal
- `packaging/aur/` reorganized into per-variant subdirectories (`wdotool/`, `wdotool-bin/`, `wdotool-git/`). Each has its own `PKGBUILD`; the workflow patches the relevant one per matrix leg.

## [0.1.3] — 2026-04-22

### Added
- AUR publish workflow (`.github/workflows/aur.yml`). On every published GitHub Release it computes the new source tarball's sha256, patches `packaging/aur/PKGBUILD`, pushes the update to the AUR, and syncs the patched PKGBUILD back to `main`. Removes the manual per-release PKGBUILD bump.

## [0.1.2] — 2026-04-22

### Added
- CI workflow (`.github/workflows/ci.yml`): `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` on every push to `main` and every pull request.
- `wl_output` tracking in the wlroots backend so `motion_absolute` uses real pixel dimensions instead of a hardcoded 10,000×10,000 square.

### Fixed
- `wdotool mousemove <x> <y>` on wlroots now lands where the caller asked. Previously a 500×300 request on a 1920×1080 monitor ended up near (96, 58).

### Internal
- Cleaned up nine clippy lints that had accumulated (`iter().any(|k| *k == target)` → `contains(&target)`, `map_or(true|false, …)` → `is_none_or` / `is_some_and`). Code is now clippy-clean under `-D warnings`.

## [0.1.1] — 2026-04-22

### Added
- `LICENSE-MIT` and `LICENSE-APACHE` in the repo root. Previously referenced by `Cargo.toml` but not actually shipped in the source tarball.
- `packaging/aur/PKGBUILD` for a source-based `wdotool` package on the AUR.

### Fixed
- First release that actually includes license text in the crate tarball, so downstream packagers and `cargo install` users can see the terms.

## [0.1.0] — 2026-04-22

Initial release.

### Added
- xdotool-compatible CLI (`key`, `type`, `mousemove`, `click`, `scroll`, `search`, `getactivewindow`, `windowactivate`, `windowclose`, `info`) via `clap`.
- **libei backend** using the XDG RemoteDesktop portal and `reis` for the EIS handshake. Handles keyboard + pointer + scroll + button. Runs on a dedicated OS thread because the event stream is `!Send`.
- **wlroots backend** using `zwp_virtual_keyboard_v1`, `zwlr_virtual_pointer_v1`, and `zwlr_foreign_toplevel_management_v1`. Full window management (list / activate / close / getactive). Unicode `type` via transient-keymap injection.
- **KDE backend** composing libei (input) with KWin scripting (windows) over D-Bus. Uses a transient `com.wdotool.KdeBridge` zbus service so scripts can call back with results. Works on Plasma 5 and 6.
- **uinput fallback** via `/dev/uinput` for environments without libei or wlroots protocols. No focus awareness; requires `uinput` group membership or a udev rule.
- Runtime backend detector that picks the best available backend based on `XDG_CURRENT_DESKTOP`, compositor hints, and bootstrap success; falls through alternatives when the preferred one fails.
- GitHub Actions release workflow via `cargo-dist` for a shell-installer + tarball release.

### Known limitations
- GNOME window backend is not yet implemented.
- `type_text` Unicode support is full on wlroots (transient keymap) but best-effort on libei/uinput (bounded by the compositor's active keymap).

[Unreleased]: https://github.com/cushycush/wdotool/compare/v0.1.5...HEAD
[0.1.5]: https://github.com/cushycush/wdotool/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/cushycush/wdotool/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/cushycush/wdotool/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/cushycush/wdotool/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/cushycush/wdotool/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/cushycush/wdotool/releases/tag/v0.1.0
