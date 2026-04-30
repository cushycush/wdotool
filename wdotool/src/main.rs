use clap::Parser;

use wdotool::{cli::Command, diag, Cli, DispatchCtx, ExitCode};
use wdotool_core::detector::{self, BackendKind, Environment};
use wdotool_core::WdoError;

#[cfg(feature = "recorder")]
use wdotool::record;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    wdotool::init_tracing(cli.verbose);

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

    // Record short-circuits before detector::build for the same
    // reason Diag does: the recorder owns its own portal session
    // (libei in receiver mode) and doesn't need a sender Backend
    // to bootstrap.
    #[cfg(feature = "recorder")]
    if let Command::Record {
        output,
        max_duration,
        backend,
    } = cli.command
    {
        return record::run(output, max_duration, backend).await;
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

    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let mut ctx = DispatchCtx {
        backend: &*backend,
        env: &env,
        stdout: &mut stdout,
        stderr: &mut stderr,
    };
    let exit = wdotool::dispatch(&mut ctx, cli.command).await?;
    drop(ctx);
    drop(stdout);
    drop(stderr);
    if exit != ExitCode::SUCCESS {
        std::process::exit(exit.0);
    }
    Ok(())
}
