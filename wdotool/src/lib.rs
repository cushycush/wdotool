//! Library half of the `wdotool` binary. The thin `main.rs` wires
//! argv → `cli::Cli` → `dispatch`, but everything testable lives here so
//! integration tests in `tests/` can drive `dispatch` against a mock
//! backend without spawning a subprocess.
//!
//! The two public entry points worth knowing:
//!
//! - [`dispatch`] runs a parsed [`Command`] against an arbitrary
//!   [`Backend`], writing all human-readable output to the writers in
//!   [`DispatchCtx`]. It returns an [`ExitCode`] — the binary translates
//!   non-zero into `process::exit`, tests just assert on it.
//! - [`SearchFilters`] is exposed so the existing search unit tests can
//!   keep their friendly module-private feel without re-deriving the
//!   filter logic.

pub mod cli;
pub mod diag;
#[cfg(feature = "recorder")]
pub mod record;
#[cfg(feature = "recorder")]
pub mod replay;

use std::io::Write;
use std::time::Duration;

use regex::Regex;
use tracing_subscriber::EnvFilter;

use wdotool_core::detector::Environment;
use wdotool_core::keysym;
use wdotool_core::{
    Backend, KeyDirection, MouseButton, Result, WdoError, WindowGeometry, WindowId, WindowInfo,
};

pub use cli::{Cli, Command};

/// Output sinks + environment passed to [`dispatch`]. The binary fills
/// these with `io::stdout()` / `io::stderr()`; tests fill them with
/// `Vec<u8>` so they can assert on captured output.
pub struct DispatchCtx<'a> {
    pub backend: &'a dyn Backend,
    pub env: &'a Environment,
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
}

/// Wraps a process exit status. `0` is success; non-zero is reserved
/// for the xdotool-compatible "no match / unsupported on this backend"
/// signals (search with no results, getmouselocation on a send-only
/// backend, getwindow* with a missing field). `dispatch` returns this
/// instead of calling `process::exit` directly so tests can run inside
/// the same process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct ExitCode(pub i32);

impl ExitCode {
    pub const SUCCESS: ExitCode = ExitCode(0);
    pub const FAILURE: ExitCode = ExitCode(1);

    pub fn is_success(self) -> bool {
        self.0 == 0
    }
}

/// Run a parsed command against a backend. Returns a structured exit
/// code instead of terminating the process; the binary calls
/// `process::exit` on non-zero, tests assert directly.
pub async fn dispatch(ctx: &mut DispatchCtx<'_>, cmd: Command) -> Result<ExitCode> {
    match cmd {
        Command::Capabilities => {
            let value = wdotool_core::capabilities::report_json(ctx.env, ctx.backend);
            let pretty = serde_json::to_string_pretty(&value)
                .map_err(|e| WdoError::InvalidArg(format!("capabilities serialization: {e}")))?;
            writeln!(ctx.stdout, "{pretty}").map_err(io_err)?;
        }
        Command::Info => {
            let caps = ctx.backend.capabilities();
            writeln!(ctx.stdout, "backend:  {}", ctx.backend.name()).map_err(io_err)?;
            writeln!(ctx.stdout, "desktop:  {:?}", ctx.env.desktop).map_err(io_err)?;
            writeln!(ctx.stdout, "session:  {:?}", ctx.env.session_type).map_err(io_err)?;
            writeln!(ctx.stdout, "display:  {:?}", ctx.env.wayland_display).map_err(io_err)?;
            writeln!(ctx.stdout, "hints:    {:?}", ctx.env.compositor_hints).map_err(io_err)?;
            writeln!(ctx.stdout, "wayland:  {}", ctx.env.is_wayland()).map_err(io_err)?;
            writeln!(ctx.stdout, "capabilities:").map_err(io_err)?;
            writeln!(ctx.stdout, "  key_input:             {}", caps.key_input).map_err(io_err)?;
            writeln!(ctx.stdout, "  text_input:            {}", caps.text_input).map_err(io_err)?;
            writeln!(
                ctx.stdout,
                "  pointer_move_absolute: {}",
                caps.pointer_move_absolute
            )
            .map_err(io_err)?;
            writeln!(
                ctx.stdout,
                "  pointer_move_relative: {}",
                caps.pointer_move_relative
            )
            .map_err(io_err)?;
            writeln!(
                ctx.stdout,
                "  pointer_button:        {}",
                caps.pointer_button
            )
            .map_err(io_err)?;
            writeln!(ctx.stdout, "  scroll:                {}", caps.scroll).map_err(io_err)?;
            writeln!(ctx.stdout, "  list_windows:          {}", caps.list_windows)
                .map_err(io_err)?;
            writeln!(
                ctx.stdout,
                "  active_window:         {}",
                caps.active_window
            )
            .map_err(io_err)?;
            writeln!(
                ctx.stdout,
                "  activate_window:       {}",
                caps.activate_window
            )
            .map_err(io_err)?;
            writeln!(ctx.stdout, "  close_window:          {}", caps.close_window)
                .map_err(io_err)?;
            writeln!(
                ctx.stdout,
                "  pointer_position:      {}",
                caps.pointer_position
            )
            .map_err(io_err)?;
            writeln!(ctx.stdout, "  list_outputs:          {}", caps.list_outputs)
                .map_err(io_err)?;
            writeln!(
                ctx.stdout,
                "  window_geometry:       {}",
                caps.window_geometry
            )
            .map_err(io_err)?;
        }
        Command::Key {
            clearmodifiers,
            chain,
        } => {
            if clearmodifiers {
                clear_modifiers(ctx.backend).await;
            }
            run_key(ctx.backend, &chain, KeyDirection::PressRelease).await?;
        }
        Command::Keydown {
            clearmodifiers,
            chain,
        } => {
            if clearmodifiers {
                clear_modifiers(ctx.backend).await;
            }
            run_key(ctx.backend, &chain, KeyDirection::Press).await?;
        }
        Command::Keyup {
            clearmodifiers,
            chain,
        } => {
            if clearmodifiers {
                clear_modifiers(ctx.backend).await;
            }
            run_key(ctx.backend, &chain, KeyDirection::Release).await?;
        }
        Command::Type {
            delay,
            file,
            clearmodifiers,
            text,
        } => {
            let resolved = resolve_type_input(file, text)?;
            if clearmodifiers {
                clear_modifiers(ctx.backend).await;
            }
            ctx.backend
                .type_text(&resolved, Duration::from_millis(delay))
                .await?;
        }
        Command::Mousemove {
            relative,
            output,
            x,
            y,
        } => {
            // clap already rejects --output combined with --relative,
            // so the two arms here are mutually exclusive. The
            // --output path delegates to the trait's
            // mouse_move_to_output method, which has a default impl
            // that translates output-local coords to global; the
            // wlr-protocols backend overrides that default to bind a
            // per-output virtual_pointer (fixes #22).
            match output {
                Some(name) => ctx.backend.mouse_move_to_output(&name, x, y).await?,
                None => ctx.backend.mouse_move(x, y, !relative).await?,
            }
        }
        Command::Click { button } => {
            ctx.backend
                .mouse_button(MouseButton::from_index(button), KeyDirection::PressRelease)
                .await?;
        }
        Command::Mousedown { button } => {
            ctx.backend
                .mouse_button(MouseButton::from_index(button), KeyDirection::Press)
                .await?;
        }
        Command::Mouseup { button } => {
            ctx.backend
                .mouse_button(MouseButton::from_index(button), KeyDirection::Release)
                .await?;
        }
        Command::Scroll { dx, dy } => {
            ctx.backend.scroll(dx, dy).await?;
        }
        Command::Search {
            name,
            class,
            pid,
            regex,
            ignore_case,
            any,
            all: _,
        } => {
            let windows = ctx.backend.list_windows().await?;
            let filters = SearchFilters::compile(SearchFlags {
                name: name.as_deref(),
                class: class.as_deref(),
                pid,
                regex,
                ignore_case,
                any,
            })?;
            let mut matched = false;
            for w in windows.iter().filter(|w| filters.matches(w)) {
                writeln!(ctx.stdout, "{}\t{}", w.id, w.title).map_err(io_err)?;
                matched = true;
            }
            // xdotool exits 1 when nothing matched; preserve that for
            // shell scripts that branch on `if wdotool search ...`.
            if !matched {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Getactivewindow => match ctx.backend.active_window().await? {
            Some(w) => writeln!(ctx.stdout, "{}", w.id).map_err(io_err)?,
            None => return Err(WdoError::WindowNotFound("active".into())),
        },
        Command::Outputs { json } => {
            let outputs = ctx.backend.list_outputs().await?;
            if json {
                let value = serde_json::to_value(
                    outputs
                        .iter()
                        .map(|o| {
                            serde_json::json!({
                                "name": o.name,
                                "x": o.x,
                                "y": o.y,
                                "width": o.width,
                                "height": o.height,
                                "scale": o.scale,
                            })
                        })
                        .collect::<Vec<_>>(),
                )
                .map_err(|e| WdoError::InvalidArg(format!("outputs serialization: {e}")))?;
                let pretty = serde_json::to_string_pretty(&value)
                    .map_err(|e| WdoError::InvalidArg(format!("outputs serialization: {e}")))?;
                writeln!(ctx.stdout, "{pretty}").map_err(io_err)?;
            } else {
                writeln!(ctx.stdout, "name\tx\ty\twidth\theight\tscale").map_err(io_err)?;
                for o in &outputs {
                    writeln!(
                        ctx.stdout,
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        o.name, o.x, o.y, o.width, o.height, o.scale
                    )
                    .map_err(io_err)?;
                }
            }
        }
        Command::Getmouselocation => match ctx.backend.pointer_position().await? {
            Some((x, y)) => writeln!(ctx.stdout, "x:{x} y:{y}").map_err(io_err)?,
            None => {
                writeln!(
                    ctx.stderr,
                    "wdotool: pointer position is unreadable on the {} backend (Wayland \
                     virtual-pointer protocols are send-only). Use the kde or gnome backend, \
                     or your compositor's IPC (hyprctl cursorpos, swaymsg get_seats).",
                    ctx.backend.name()
                )
                .map_err(io_err)?;
                return Ok(ExitCode::FAILURE);
            }
        },
        Command::Windowactivate { id } => ctx.backend.activate_window(&WindowId(id)).await?,
        Command::Windowclose { id } => ctx.backend.close_window(&WindowId(id)).await?,
        Command::Getwindowname { id } => {
            let w = find_window(ctx.backend, &id).await?;
            writeln!(ctx.stdout, "{}", w.title).map_err(io_err)?;
        }
        Command::Getwindowpid { id } => {
            let w = find_window(ctx.backend, &id).await?;
            match w.pid {
                Some(pid) => writeln!(ctx.stdout, "{pid}").map_err(io_err)?,
                None => {
                    writeln!(ctx.stderr, "wdotool: pid not available for window {id}")
                        .map_err(io_err)?;
                    return Ok(ExitCode::FAILURE);
                }
            }
        }
        Command::Getwindowclassname { id } => {
            let w = find_window(ctx.backend, &id).await?;
            match w.app_id {
                Some(app_id) => writeln!(ctx.stdout, "{app_id}").map_err(io_err)?,
                None => {
                    writeln!(
                        ctx.stderr,
                        "wdotool: classname (app_id) not available for window {id}"
                    )
                    .map_err(io_err)?;
                    return Ok(ExitCode::FAILURE);
                }
            }
        }
        Command::Getwindowgeometry { id } => {
            // Trait contract:
            //   Ok(Some(geom)) -> backend supports it, found, here it is
            //   Err(WindowNotFound) -> backend supports it, but no
            //                          window with that id
            //   Ok(None) -> backend doesn't support reading geometry
            // The error path bubbles via `?` so we only handle
            // Ok(Some) and Ok(None) explicitly here.
            match ctx.backend.window_geometry(&WindowId(id.clone())).await? {
                Some(WindowGeometry {
                    x,
                    y,
                    width,
                    height,
                }) => {
                    // Match xdotool's default format. The "screen"
                    // line xdotool prints doesn't translate to Wayland
                    // (compositors don't expose a stable screen index
                    // that's meaningful to clients), so we drop it.
                    writeln!(ctx.stdout, "Window {id}").map_err(io_err)?;
                    writeln!(ctx.stdout, "  Position: {x},{y}").map_err(io_err)?;
                    writeln!(ctx.stdout, "  Geometry: {width}x{height}").map_err(io_err)?;
                }
                None => {
                    writeln!(
                        ctx.stderr,
                        "wdotool: window geometry is unreadable on the {} backend (no Wayland \
                         protocol exposes window geometry to other clients). Use the kde or \
                         gnome backend.",
                        ctx.backend.name()
                    )
                    .map_err(io_err)?;
                    return Ok(ExitCode::FAILURE);
                }
            }
        }
        Command::Diag { .. } => {
            // Handled in main() before dispatch is called so diag never
            // bootstraps a backend.
            unreachable!("Diag short-circuits before dispatch");
        }
        #[cfg(feature = "recorder")]
        Command::Record { .. } => {
            // Same as Diag: handled in main() before dispatch so the
            // recorder owns its own portal session.
            unreachable!("Record short-circuits before dispatch");
        }
        Command::Prime => {
            // Same pattern: prime needs to hold the backend alive in
            // the foreground until a signal, so main() bypasses
            // dispatch and runs its own loop.
            unreachable!("Prime short-circuits before dispatch");
        }
        #[cfg(feature = "recorder")]
        Command::Replay { file, speed } => {
            replay::run(ctx.backend, &file, speed).await?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn io_err(e: std::io::Error) -> WdoError {
    // dispatch's writers are real stdout/stderr in production and an
    // in-memory Vec<u8> in tests; the only thing that can realistically
    // fail is a closed pipe (`wdotool foo | head`), so map to InvalidArg
    // to keep the error type honest without inventing a new variant.
    WdoError::InvalidArg(format!("write failed: {e}"))
}

/// Look up a window by its id string. Used by the `getwindow*` commands
/// which all need to resolve an id to a `WindowInfo` before reading a
/// single field. Returns `WindowNotFound` if no window in the current
/// list has that id, which xdotool also signals via non-zero exit.
async fn find_window(backend: &dyn Backend, id: &str) -> Result<WindowInfo> {
    let windows = backend.list_windows().await?;
    windows
        .into_iter()
        .find(|w| w.id.0 == id)
        .ok_or_else(|| WdoError::WindowNotFound(id.to_string()))
}

/// Approximates xdotool's --clearmodifiers. Wayland doesn't let a normal
/// client query the compositor's current modifier state, so we can't do the
/// "save + restore" dance xdotool does. Best effort: release every standard
/// modifier unconditionally, ignoring backend errors per-key (a modifier
/// that isn't in the keymap is a no-op, not a user-visible failure).
async fn clear_modifiers(backend: &dyn Backend) {
    const STANDARD_MODIFIERS: &[&str] = &[
        "Control_L",
        "Control_R",
        "Shift_L",
        "Shift_R",
        "Alt_L",
        "Alt_R",
        "Super_L",
        "Super_R",
        "ISO_Level3_Shift",
    ];
    for sym in STANDARD_MODIFIERS {
        let _ = backend.key(sym, KeyDirection::Release).await;
    }
}

/// Resolve the text to type: from --file (path or `-` for stdin) or the
/// positional argument. clap enforces mutual exclusion; this function just
/// dispatches and errors if neither source is present.
fn resolve_type_input(file: Option<String>, text: Option<String>) -> Result<String> {
    use std::io::Read;
    match (file, text) {
        (Some(path), _) => {
            if path == "-" {
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .map_err(|e| WdoError::InvalidArg(format!("failed to read stdin: {e}")))?;
                Ok(buf)
            } else {
                std::fs::read_to_string(&path)
                    .map_err(|e| WdoError::InvalidArg(format!("failed to read {path}: {e}")))
            }
        }
        (None, Some(t)) => Ok(t),
        (None, None) => Err(WdoError::InvalidArg(
            "type requires either --file <path> or a positional text argument".into(),
        )),
    }
}

// Press modifiers, then the key, then release in reverse — matches xdotool
// ordering so scripts relying on this behaviour continue to work.
pub(crate) async fn run_key(backend: &dyn Backend, chain: &str, dir: KeyDirection) -> Result<()> {
    let parsed = keysym::parse_chain(chain)?;
    match dir {
        KeyDirection::Press => {
            for m in &parsed.modifiers {
                backend.key(m, KeyDirection::Press).await?;
            }
            backend.key(&parsed.key, KeyDirection::Press).await?;
        }
        KeyDirection::Release => {
            backend.key(&parsed.key, KeyDirection::Release).await?;
            for m in parsed.modifiers.iter().rev() {
                backend.key(m, KeyDirection::Release).await?;
            }
        }
        KeyDirection::PressRelease => {
            for m in &parsed.modifiers {
                backend.key(m, KeyDirection::Press).await?;
            }
            backend.key(&parsed.key, KeyDirection::PressRelease).await?;
            for m in parsed.modifiers.iter().rev() {
                backend.key(m, KeyDirection::Release).await?;
            }
        }
    }
    Ok(())
}

pub fn init_tracing(verbose: bool) {
    let default = if verbose {
        "wdotool=debug"
    } else {
        "wdotool=info,warn"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Inputs to [`SearchFilters::compile`]; mirrors the CLI flags so the
/// filter compilation is testable without round-tripping through clap.
pub struct SearchFlags<'a> {
    pub name: Option<&'a str>,
    pub class: Option<&'a str>,
    pub pid: Option<u32>,
    pub regex: bool,
    pub ignore_case: bool,
    pub any: bool,
}

/// Compiled search predicates. Built once per `wdotool search` call and
/// then applied to each window. Holding the regex(es) here avoids
/// recompiling per-window.
pub struct SearchFilters {
    name: Option<Regex>,
    class: Option<Regex>,
    pid: Option<u32>,
    /// True when `--any` was passed: matching switches from AND to OR.
    any: bool,
}

impl SearchFilters {
    pub fn compile(flags: SearchFlags<'_>) -> Result<Self> {
        let make_regex = |pat: &str, field: &'static str| -> Result<Regex> {
            // Without --regex, escape so substring patterns are
            // taken literally. With --ignore-case, prefix with the
            // (?i) inline flag; works in both modes uniformly.
            let body = if flags.regex {
                pat.to_string()
            } else {
                regex::escape(pat)
            };
            let full = if flags.ignore_case {
                format!("(?i){body}")
            } else {
                body
            };
            Regex::new(&full)
                .map_err(|e| WdoError::InvalidArg(format!("invalid {field} pattern {pat:?}: {e}")))
        };
        Ok(Self {
            name: flags.name.map(|p| make_regex(p, "--name")).transpose()?,
            class: flags.class.map(|p| make_regex(p, "--class")).transpose()?,
            pid: flags.pid,
            any: flags.any,
        })
    }

    pub fn matches(&self, w: &WindowInfo) -> bool {
        if self.any {
            self.matches_any(w)
        } else {
            self.matches_all(w)
        }
    }

    /// AND semantics (default): every set filter must match.
    fn matches_all(&self, w: &WindowInfo) -> bool {
        if let Some(re) = &self.name {
            if !re.is_match(&w.title) {
                return false;
            }
        }
        if let Some(re) = &self.class {
            // app_id is the Wayland equivalent of WM_CLASS. Backends
            // that don't expose it (uinput, bare libei) can't match
            // here at all, which is correct.
            match w.app_id.as_deref() {
                Some(a) if re.is_match(a) => {}
                _ => return false,
            }
        }
        if let Some(p) = self.pid {
            if w.pid != Some(p) {
                return false;
            }
        }
        true
    }

    /// OR semantics (`--any`): at least one set filter must match.
    /// With zero set filters, falls back to "match everything" so that
    /// `wdotool search --any` (no filters) lists all windows, same as
    /// `wdotool search` does.
    fn matches_any(&self, w: &WindowInfo) -> bool {
        let any_set = self.name.is_some() || self.class.is_some() || self.pid.is_some();
        if !any_set {
            return true;
        }
        if let Some(re) = &self.name {
            if re.is_match(&w.title) {
                return true;
            }
        }
        if let Some(re) = &self.class {
            if let Some(a) = w.app_id.as_deref() {
                if re.is_match(a) {
                    return true;
                }
            }
        }
        if let Some(p) = self.pid {
            if w.pid == Some(p) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use wdotool_core::WindowId;

    fn win(id: &str, title: &str, app_id: Option<&str>, pid: Option<u32>) -> WindowInfo {
        WindowInfo {
            id: WindowId(id.into()),
            title: title.into(),
            app_id: app_id.map(str::to_string),
            pid,
        }
    }

    fn flags<'a>(
        name: Option<&'a str>,
        class: Option<&'a str>,
        pid: Option<u32>,
    ) -> SearchFlags<'a> {
        SearchFlags {
            name,
            class,
            pid,
            regex: false,
            ignore_case: false,
            any: false,
        }
    }

    #[test]
    fn substring_name_match_is_default() {
        let f = SearchFilters::compile(flags(Some("fox"), None, None)).unwrap();
        assert!(f.matches(&win("1", "Firefox", None, None)));
        assert!(!f.matches(&win("2", "Chromium", None, None)));
    }

    #[test]
    fn dot_in_pattern_is_escaped_without_regex_flag() {
        // With --regex off, `Fire.fox` only matches the literal string,
        // not `Fire?fox` (which a regex would match).
        let f = SearchFilters::compile(flags(Some("Fire.fox"), None, None)).unwrap();
        assert!(!f.matches(&win("1", "Firefox", None, None)));
        assert!(f.matches(&win("2", "Fire.fox Browser", None, None)));
    }

    #[test]
    fn regex_flag_enables_pattern_semantics() {
        let f = SearchFilters::compile(SearchFlags {
            name: Some("Fire.*x"),
            class: None,
            pid: None,
            regex: true,
            ignore_case: false,
            any: false,
        })
        .unwrap();
        assert!(f.matches(&win("1", "Firefox", None, None)));
        assert!(!f.matches(&win("2", "Chromium", None, None)));
    }

    #[test]
    fn ignore_case_works_in_substring_mode() {
        let f = SearchFilters::compile(SearchFlags {
            name: Some("FIREFOX"),
            class: None,
            pid: None,
            regex: false,
            ignore_case: true,
            any: false,
        })
        .unwrap();
        assert!(f.matches(&win("1", "Mozilla Firefox", None, None)));
    }

    #[test]
    fn ignore_case_works_in_regex_mode() {
        let f = SearchFilters::compile(SearchFlags {
            name: Some("FIRE.*X"),
            class: None,
            pid: None,
            regex: true,
            ignore_case: true,
            any: false,
        })
        .unwrap();
        assert!(f.matches(&win("1", "Mozilla Firefox", None, None)));
    }

    #[test]
    fn class_filter_matches_app_id() {
        let f = SearchFilters::compile(flags(None, Some("firefox"), None)).unwrap();
        assert!(f.matches(&win("1", "Some Page", Some("org.mozilla.firefox"), None)));
        assert!(!f.matches(&win("2", "kitty", Some("kitty"), None)));
    }

    #[test]
    fn class_filter_skips_windows_without_app_id() {
        let f = SearchFilters::compile(flags(None, Some("anything"), None)).unwrap();
        // app_id None means the backend doesn't expose it (uinput,
        // bare libei). Such windows can never satisfy a class filter.
        assert!(!f.matches(&win("1", "Some Page", None, None)));
    }

    #[test]
    fn pid_filter_requires_exact_match() {
        let f = SearchFilters::compile(flags(None, None, Some(1234))).unwrap();
        assert!(f.matches(&win("1", "Firefox", None, Some(1234))));
        assert!(!f.matches(&win("2", "Firefox", None, Some(5678))));
        // Backends that don't populate pid never match a pid filter.
        assert!(!f.matches(&win("3", "Firefox", None, None)));
    }

    #[test]
    fn filters_are_anded_together() {
        let f =
            SearchFilters::compile(flags(Some("Firefox"), Some("mozilla"), Some(1234))).unwrap();
        assert!(f.matches(&win(
            "1",
            "Firefox - Wikipedia",
            Some("org.mozilla.firefox"),
            Some(1234)
        )));
        // Right title + class but wrong pid: rejected.
        assert!(!f.matches(&win(
            "2",
            "Firefox - Wikipedia",
            Some("org.mozilla.firefox"),
            Some(5678)
        )));
    }

    #[test]
    fn no_filters_matches_everything() {
        let f = SearchFilters::compile(flags(None, None, None)).unwrap();
        assert!(f.matches(&win("1", "Anything", None, None)));
        assert!(f.matches(&win("2", "Else", Some("kitty"), Some(99))));
    }

    fn flags_any<'a>(
        name: Option<&'a str>,
        class: Option<&'a str>,
        pid: Option<u32>,
    ) -> SearchFlags<'a> {
        SearchFlags {
            name,
            class,
            pid,
            regex: false,
            ignore_case: false,
            any: true,
        }
    }

    #[test]
    fn any_matches_when_only_name_matches() {
        let f = SearchFilters::compile(flags_any(Some("Firefox"), Some("nope"), Some(99))).unwrap();
        // Title matches, class and pid don't. With AND this would be
        // rejected; with --any, name alone is enough.
        assert!(f.matches(&win(
            "1",
            "Firefox - Wikipedia",
            Some("org.mozilla.firefox"),
            Some(1234)
        )));
    }

    #[test]
    fn any_matches_when_only_class_matches() {
        let f = SearchFilters::compile(flags_any(Some("nope"), Some("firefox"), Some(99))).unwrap();
        assert!(f.matches(&win(
            "1",
            "Wikipedia",
            Some("org.mozilla.firefox"),
            Some(1234)
        )));
    }

    #[test]
    fn any_matches_when_only_pid_matches() {
        let f = SearchFilters::compile(flags_any(Some("nope"), Some("nope"), Some(1234))).unwrap();
        assert!(f.matches(&win(
            "1",
            "Wikipedia",
            Some("org.mozilla.firefox"),
            Some(1234)
        )));
    }

    #[test]
    fn any_rejects_when_no_filter_matches() {
        let f = SearchFilters::compile(flags_any(Some("nope"), Some("nope"), Some(99))).unwrap();
        assert!(!f.matches(&win(
            "1",
            "Wikipedia",
            Some("org.mozilla.firefox"),
            Some(1234)
        )));
    }

    #[test]
    fn any_with_no_filters_matches_everything() {
        // Same fall-through as default: zero filters lists all windows
        // regardless of which combinator was chosen.
        let f = SearchFilters::compile(flags_any(None, None, None)).unwrap();
        assert!(f.matches(&win("1", "Anything", None, None)));
        assert!(f.matches(&win("2", "Else", Some("kitty"), Some(99))));
    }

    #[test]
    fn any_with_class_filter_skips_window_without_app_id() {
        // app_id None can't satisfy a class regex; with --any and only
        // a class filter set, that means the window doesn't match.
        let f = SearchFilters::compile(flags_any(None, Some("anything"), None)).unwrap();
        assert!(!f.matches(&win("1", "Some Page", None, None)));
    }

    #[test]
    fn invalid_regex_pattern_returns_invalid_arg() {
        let result = SearchFilters::compile(SearchFlags {
            name: Some("[unclosed"),
            class: None,
            pid: None,
            regex: true,
            ignore_case: false,
            any: false,
        });
        match result {
            Err(WdoError::InvalidArg(msg)) => assert!(msg.contains("--name")),
            Err(other) => panic!("expected InvalidArg, got {other:?}"),
            Ok(_) => panic!("expected InvalidArg, got Ok"),
        }
    }
}
