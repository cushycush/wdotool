# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/cushycush/wdotool/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/cushycush/wdotool/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/cushycush/wdotool/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/cushycush/wdotool/releases/tag/v0.1.0
