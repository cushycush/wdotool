mod backend;
mod cli;
mod error;
mod keysym;
mod types;

use std::time::Duration;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use backend::detector::{self, BackendKind, Environment};
use backend::Backend;
use cli::{Cli, Command};
use error::{Result, WdoError};
use types::{KeyDirection, MouseButton, WindowId};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

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
        Command::Key { chain } => run_key(backend, &chain, KeyDirection::PressRelease).await?,
        Command::Keydown { chain } => run_key(backend, &chain, KeyDirection::Press).await?,
        Command::Keyup { chain } => run_key(backend, &chain, KeyDirection::Release).await?,
        Command::Type { delay, text } => {
            backend
                .type_text(&text, Duration::from_millis(delay))
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
                name.as_deref().map_or(true, |n| w.title.contains(n))
                    && class.as_deref().map_or(true, |c| {
                        w.app_id.as_deref().map_or(false, |a| a.contains(c))
                    })
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
    }
    Ok(())
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
            backend
                .key(&parsed.key, KeyDirection::PressRelease)
                .await?;
            for m in parsed.modifiers.iter().rev() {
                backend.key(m, KeyDirection::Release).await?;
            }
        }
    }
    Ok(())
}

fn init_tracing(verbose: bool) {
    let default = if verbose { "wdotool=debug" } else { "wdotool=info,warn" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}
