//! Shared scaffolding for the integration test suites in
//! `wdotool/tests/`. Each `cli_*.rs` test crate brings this in via
//! `mod common;` and uses it to build a `DispatchCtx` over a
//! `MockBackend` plus capture buffers, run a parsed command, and
//! return the recorded call sequence + captured stdout/stderr.
//!
//! The single entry point is [`run`]: pass the argv you'd pass to the
//! CLI (with or without the leading `wdotool`), and it returns
//! everything the test needs to assert on.

use std::sync::Arc;

use clap::Parser;

use wdotool::{Cli, DispatchCtx, ExitCode};
use wdotool_core::backend::mock::{BackendCall, MockBackend};
use wdotool_core::detector::Environment;
use wdotool_core::{Result as WdoResult, WdoError};

/// Outcome of a single `dispatch` run against the mock. Each
/// integration test file picks the fields it cares about, so the
/// dead-code allow keeps the suite quiet across the lot.
#[allow(dead_code)]
pub struct RunResult {
    pub exit: ExitCode,
    pub error: Option<WdoError>,
    pub calls: Vec<BackendCall>,
    pub stdout: String,
    pub stderr: String,
}

/// Run `argv` through clap → `dispatch` against a fresh `MockBackend`.
/// `argv` may or may not include a leading `wdotool` token; if missing,
/// it's prepended for clap.
pub async fn run(argv: &[&str]) -> RunResult {
    run_with(argv, MockBackend::new(), Environment::default()).await
}

/// Same as [`run`] but lets the caller pre-configure the mock (canned
/// windows / pointer / outputs / forced failures) and the environment.
pub async fn run_with(argv: &[&str], mock: MockBackend, env: Environment) -> RunResult {
    let mock = Arc::new(mock);
    let argv_owned: Vec<String> = if argv.first().copied() == Some("wdotool") {
        argv.iter().map(|s| s.to_string()).collect()
    } else {
        std::iter::once("wdotool".to_string())
            .chain(argv.iter().map(|s| s.to_string()))
            .collect()
    };
    let cli = match Cli::try_parse_from(&argv_owned) {
        Ok(c) => c,
        Err(e) => panic!("clap parse failed for {argv:?}: {e}"),
    };

    let mut stdout = Vec::<u8>::new();
    let mut stderr = Vec::<u8>::new();

    let outcome: WdoResult<ExitCode> = {
        let mut ctx = DispatchCtx {
            backend: &*mock,
            env: &env,
            stdout: &mut stdout,
            stderr: &mut stderr,
        };
        wdotool::dispatch(&mut ctx, cli.command).await
    };

    let (exit, error) = match outcome {
        Ok(code) => (code, None),
        Err(e) => (ExitCode::FAILURE, Some(e)),
    };

    let calls = mock.calls();
    RunResult {
        exit,
        error,
        calls,
        stdout: String::from_utf8(stdout).expect("stdout is not utf8"),
        stderr: String::from_utf8(stderr).expect("stderr is not utf8"),
    }
}
