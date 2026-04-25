# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `wdotool diag` and `wdotool diag --copy`. Environment + backend availability report meant for bug triage. Probes pre-conditions only (XDG env vars, portal availability via `busctl`, GNOME extension presence, `/dev/uinput` writability, portal token cache state), so the diag run never opens a portal session and never pops a consent dialog. Markdown by default, `--json` for machine-readable output, `--copy` pipes the markdown through `wl-copy` (falling back to `xclip`).
- libei portal token cache. The first run prompts the user for consent. The portal-issued `restore_token` is cached at `$XDG_STATE_HOME/wdotool/portal.token` (mode 0600 set at create time via `OpenOptions::mode(0o600)`). Subsequent runs present the token and skip the dialog. The recovery flow detects token rejection and re-runs the consent flow without clobbering a still-valid cache on transient failures. Delete the cache file to force a fresh consent.
- Per-backend Cargo features (`libei`, `wlroots`, `kde`, `gnome`, `uinput`) on the new `wdotool-core` library crate. Default-on enables all five. Downstream Rust consumers can opt out: `default-features = false, features = ["libei", "wlroots", "kde", "gnome"]` drops uinput's `input-linux` and `libc` deps.

### Changed
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
