mod cli;
mod diag;

use std::time::Duration;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use regex::Regex;

use wdotool_core::detector::{self, BackendKind, Environment};
use wdotool_core::keysym;
use wdotool_core::{Backend, KeyDirection, MouseButton, Result, WdoError, WindowId, WindowInfo};

use cli::{Cli, Command};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    // Diag has to short-circuit before detector::build so the probes
    // never touch the portal session (which would pop a consent dialog
    // for libei users, exactly the surprise diag is meant to remove).
    if let Command::Diag { json, copy } = cli.command {
        let format = if json {
            diag::DiagFormat::Json
        } else {
            diag::DiagFormat::Markdown
        };
        return diag::run(format, copy);
    }

    let env = Environment::detect();

    let forced = match cli.backend.as_deref() {
        Some(s) => Some(BackendKind::parse(s).ok_or_else(|| {
            WdoError::InvalidArg(format!(
                "unknown backend '{s}' (expected libei, wlroots, kde, gnome, uinput)"
            ))
        })?),
        None => None,
    };

    let backend = detector::build(&env, forced).await?;

    dispatch(&*backend, &env, cli.command).await?;
    Ok(())
}

async fn dispatch(backend: &dyn Backend, env: &Environment, cmd: Command) -> Result<()> {
    match cmd {
        Command::Capabilities => {
            let value = wdotool_core::capabilities::report_json(env, backend);
            // pretty for humans tailing the output; consumers parse
            // either form fine.
            let pretty = serde_json::to_string_pretty(&value)
                .map_err(|e| WdoError::InvalidArg(format!("capabilities serialization: {e}")))?;
            println!("{pretty}");
        }
        Command::Info => {
            let caps = backend.capabilities();
            println!("backend:  {}", backend.name());
            println!("desktop:  {:?}", env.desktop);
            println!("session:  {:?}", env.session_type);
            println!("display:  {:?}", env.wayland_display);
            println!("hints:    {:?}", env.compositor_hints);
            println!("wayland:  {}", env.is_wayland());
            println!("capabilities:");
            println!("  key_input:             {}", caps.key_input);
            println!("  text_input:            {}", caps.text_input);
            println!("  pointer_move_absolute: {}", caps.pointer_move_absolute);
            println!("  pointer_move_relative: {}", caps.pointer_move_relative);
            println!("  pointer_button:        {}", caps.pointer_button);
            println!("  scroll:                {}", caps.scroll);
            println!("  list_windows:          {}", caps.list_windows);
            println!("  active_window:         {}", caps.active_window);
            println!("  activate_window:       {}", caps.activate_window);
            println!("  close_window:          {}", caps.close_window);
            println!("  pointer_position:      {}", caps.pointer_position);
        }
        Command::Key {
            clearmodifiers,
            chain,
        } => {
            if clearmodifiers {
                clear_modifiers(backend).await;
            }
            run_key(backend, &chain, KeyDirection::PressRelease).await?;
        }
        Command::Keydown {
            clearmodifiers,
            chain,
        } => {
            if clearmodifiers {
                clear_modifiers(backend).await;
            }
            run_key(backend, &chain, KeyDirection::Press).await?;
        }
        Command::Keyup {
            clearmodifiers,
            chain,
        } => {
            if clearmodifiers {
                clear_modifiers(backend).await;
            }
            run_key(backend, &chain, KeyDirection::Release).await?;
        }
        Command::Type {
            delay,
            file,
            clearmodifiers,
            text,
        } => {
            let resolved = resolve_type_input(file, text)?;
            if clearmodifiers {
                clear_modifiers(backend).await;
            }
            backend
                .type_text(&resolved, Duration::from_millis(delay))
                .await?;
        }
        Command::Mousemove { relative, x, y } => {
            backend.mouse_move(x, y, !relative).await?;
        }
        Command::Click { button } => {
            backend
                .mouse_button(MouseButton::from_index(button), KeyDirection::PressRelease)
                .await?;
        }
        Command::Mousedown { button } => {
            backend
                .mouse_button(MouseButton::from_index(button), KeyDirection::Press)
                .await?;
        }
        Command::Mouseup { button } => {
            backend
                .mouse_button(MouseButton::from_index(button), KeyDirection::Release)
                .await?;
        }
        Command::Scroll { dx, dy } => {
            backend.scroll(dx, dy).await?;
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
            let windows = backend.list_windows().await?;
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
                println!("{}\t{}", w.id, w.title);
                matched = true;
            }
            // xdotool exits 1 when nothing matched; preserve that for
            // shell scripts that branch on `if wdotool search ...`.
            if !matched {
                std::process::exit(1);
            }
        }
        Command::Getactivewindow => match backend.active_window().await? {
            Some(w) => println!("{}", w.id),
            None => return Err(WdoError::WindowNotFound("active".into())),
        },
        Command::Getmouselocation => match backend.pointer_position().await? {
            Some((x, y)) => println!("x:{x} y:{y}"),
            None => {
                eprintln!(
                    "wdotool: pointer position is unreadable on the {} backend (Wayland \
                     virtual-pointer protocols are send-only). Use the kde or gnome backend, \
                     or your compositor's IPC (hyprctl cursorpos, swaymsg get_seats).",
                    backend.name()
                );
                std::process::exit(1);
            }
        },
        Command::Windowactivate { id } => backend.activate_window(&WindowId(id)).await?,
        Command::Windowclose { id } => backend.close_window(&WindowId(id)).await?,
        Command::Getwindowname { id } => {
            let w = find_window(backend, &id).await?;
            println!("{}", w.title);
        }
        Command::Getwindowpid { id } => {
            let w = find_window(backend, &id).await?;
            match w.pid {
                Some(pid) => println!("{pid}"),
                None => {
                    eprintln!("wdotool: pid not available for window {id}");
                    std::process::exit(1);
                }
            }
        }
        Command::Getwindowclassname { id } => {
            let w = find_window(backend, &id).await?;
            match w.app_id {
                Some(app_id) => println!("{app_id}"),
                None => {
                    eprintln!("wdotool: classname (app_id) not available for window {id}");
                    std::process::exit(1);
                }
            }
        }
        Command::Diag { .. } => {
            // Handled in main() before dispatch is called so diag never
            // bootstraps a backend.
            unreachable!("Diag short-circuits before dispatch");
        }
    }
    Ok(())
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
async fn run_key(backend: &dyn Backend, chain: &str, dir: KeyDirection) -> Result<()> {
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

fn init_tracing(verbose: bool) {
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
struct SearchFlags<'a> {
    name: Option<&'a str>,
    class: Option<&'a str>,
    pid: Option<u32>,
    regex: bool,
    ignore_case: bool,
    any: bool,
}

/// Compiled search predicates. Built once per `wdotool search` call and
/// then applied to each window. Holding the regex(es) here avoids
/// recompiling per-window.
struct SearchFilters {
    name: Option<Regex>,
    class: Option<Regex>,
    pid: Option<u32>,
    /// True when `--any` was passed: matching switches from AND to OR.
    any: bool,
}

impl SearchFilters {
    fn compile(flags: SearchFlags<'_>) -> Result<Self> {
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

    fn matches(&self, w: &WindowInfo) -> bool {
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
