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

    // Prime short-circuits the dispatch loop so it can hold the
    // wlroots backend alive in the foreground until a signal arrives.
    // We always pick wlroots regardless of cli.backend because that's
    // the only backend whose devices live across calls.
    if let Command::Prime = cli.command {
        let backend = detector::build(&env, Some(BackendKind::Wlroots)).await?;
        // Stdout is the readiness signal for any test harness (or
        // human) waiting on us. Flush so the `ready` line lands
        // before this process blocks on the signal handler.
        println!("ready");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        // Block until Ctrl-C or SIGTERM. Both end the wait the same
        // way; the backend (and its virtual devices) gets dropped
        // when this function returns, which cleanly releases the
        // devices in the compositor.
        tokio::signal::ctrl_c().await?;
        drop(backend);
        return Ok(());
    }

    let forced = match cli.backend.as_deref() {
        Some(s) => Some(BackendKind::parse(s).ok_or_else(|| {
            WdoError::InvalidArg(format!(
                "unknown backend '{s}' (expected libei, wlroots, kde, gnome, uinput)"
            ))
        })?),
        None => None,
    };

    let backend = detector::build(&env, forced).await?;

    let exit = {
        let mut stdout = std::io::stdout().lock();
        let mut stderr = std::io::stderr().lock();
        let mut ctx = DispatchCtx {
            backend: &*backend,
            env: &env,
            stdout: &mut stdout,
            stderr: &mut stderr,
        };
        wdotool::dispatch(&mut ctx, cli.command).await?
    };
    if exit != ExitCode::SUCCESS {
        std::process::exit(exit.0);
    }
    Ok(())
}
