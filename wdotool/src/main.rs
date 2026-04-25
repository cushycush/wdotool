mod cli;
mod diag;

use std::time::Duration;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use wdotool_core::detector::{self, BackendKind, Environment};
use wdotool_core::keysym;
use wdotool_core::{Backend, KeyDirection, MouseButton, Result, WdoError, WindowId};

use cli::{Cli, Command};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    // Diag has to short-circuit before detector::build so the probes
    // never touch the portal session (which would pop a consent dialog
    // for libei users — exactly the surprise diag is meant to remove).
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
        Command::Search { name, class } => {
            let windows = backend.list_windows().await?;
            for w in windows.into_iter().filter(|w| {
                name.as_deref().is_none_or(|n| w.title.contains(n))
                    && class
                        .as_deref()
                        .is_none_or(|c| w.app_id.as_deref().is_some_and(|a| a.contains(c)))
            }) {
                println!("{}\t{}", w.id, w.title);
            }
        }
        Command::Getactivewindow => match backend.active_window().await? {
            Some(w) => println!("{}", w.id),
            None => return Err(WdoError::WindowNotFound("active".into())),
        },
        Command::Windowactivate { id } => backend.activate_window(&WindowId(id)).await?,
        Command::Windowclose { id } => backend.close_window(&WindowId(id)).await?,
        Command::Diag { .. } => {
            // Handled in main() before dispatch is called so diag never
            // bootstraps a backend.
            unreachable!("Diag short-circuits before dispatch");
        }
    }
    Ok(())
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
