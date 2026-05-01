//! Headless test harness for wdotool integration tests. The library
//! piece is [`HeadlessSway`]: spawn a sway compositor with the
//! `headless` wlroots backend, give it a private `XDG_RUNTIME_DIR` and
//! `WAYLAND_DISPLAY`, and tear it down on drop. The companion binary
//! (`wdotool-observer`, in `src/bin/`) is the Wayland client tests
//! spawn inside that compositor to capture received input events.
//!
//! Round-trip integration tests look like:
//!
//! ```ignore
//! let sway = HeadlessSway::start()?;
//! let observer = sway.spawn_observer()?;
//! observer.wait_for_ready(Duration::from_secs(2))?;
//! sway.run_wdotool(&["key", "ctrl+a"])?;
//! let events = observer.collect_events(Duration::from_millis(200));
//! assert!(events.iter().any(|l| l.contains("Control_L press")));
//! ```
//!
//! `HeadlessSway::start()` returns `Err(SwayUnavailable)` when sway
//! isn't installed on the system. Tests use that signal to skip
//! themselves rather than fail; CI installs sway as a setup step.

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader};
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Per-test-binary monotonic counter used to give each `HeadlessSway`
/// a unique `WAYLAND_DISPLAY` name. Cargo runs integration tests in
/// parallel inside the same test binary, so all tests share the same
/// pid; without this, two concurrent tests would race for the same
/// socket and one would silently connect to the other's compositor.
static SESSION_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Top-level error type. Tests pattern-match on the variants so
/// "compositor is not installed" gets turned into a skip rather than a
/// failure.
#[derive(Debug)]
pub enum HarnessError {
    /// `sway` is not on PATH or otherwise can't be spawned. CI
    /// installs sway before running these tests; on a dev box that
    /// doesn't have it, the right move is to skip the test.
    SwayUnavailable(std::io::Error),
    /// Sway started but never created a socket in the runtime dir
    /// inside the timeout. Suggests a sway misconfiguration.
    SwayStartTimeout,
    /// Sway exited before becoming ready. Stderr is captured.
    SwayExitedEarly { stderr: String },
    /// Spawn failure for `wdotool-observer` or `wdotool` itself.
    SpawnFailed(std::io::Error),
    /// Observer never emitted `ready` within the timeout.
    ObserverNotReady,
    /// Other I/O.
    Io(std::io::Error),
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SwayUnavailable(e) => write!(
                f,
                "sway is not available (install sway to run headless tests): {e}"
            ),
            Self::SwayStartTimeout => {
                write!(f, "sway did not create its socket within the start timeout")
            }
            Self::SwayExitedEarly { stderr } => {
                write!(f, "sway exited before becoming ready. stderr: {stderr}")
            }
            Self::SpawnFailed(e) => write!(f, "failed to spawn child process: {e}"),
            Self::ObserverNotReady => write!(f, "observer did not become ready within timeout"),
            Self::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for HarnessError {}

impl From<std::io::Error> for HarnessError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// A running sway compositor with the `headless` wlroots backend, a
/// private `XDG_RUNTIME_DIR`, and a unique `WAYLAND_DISPLAY` socket.
/// Drop kills sway and cleans up the runtime dir.
pub struct HeadlessSway {
    runtime_dir: TempDir,
    display_name: String,
    sway: Child,
}

impl HeadlessSway {
    /// Spawn sway with the `headless` wlroots backend and wait for its
    /// Wayland socket to appear. Returns `Err(SwayUnavailable)` when
    /// sway isn't on `PATH` so tests can skip themselves.
    pub fn start() -> Result<Self, HarnessError> {
        Self::start_with(StartOptions::default())
    }

    /// Same as [`start`](Self::start) but lets the caller tweak the
    /// timeout and headless output count. Useful for the multi-output
    /// `mousemove --output` round-trip test.
    pub fn start_with(opts: StartOptions) -> Result<Self, HarnessError> {
        let runtime_dir = tempfile::Builder::new()
            .prefix("wdotool-headless-")
            .tempdir()?;
        // Sway picks its own socket name via wl_display_add_socket_auto
        // (it ignores WAYLAND_DISPLAY in the env), so we don't try to
        // pre-set it. Each test has its own XDG_RUNTIME_DIR via
        // tempfile, so collisions are impossible: sway will land on
        // `wayland-1` inside an empty tmpdir every time.
        let _ = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);

        // sway needs a config; pass a minimal one through stdin via a
        // tempfile. Empty config means "load defaults", which is fine
        // for headless tests: we don't care about keybindings.
        let config_path = runtime_dir.path().join("sway.conf");
        std::fs::write(
            &config_path,
            // Minimal config: no keybindings (we send our own), no bar,
            // a single fake output, focus follows the mouse so a
            // single-window scene gets keyboard focus without manual
            // intervention.
            b"focus_follows_mouse yes\n",
        )?;

        let mut cmd = Command::new("sway");
        cmd.env("XDG_RUNTIME_DIR", runtime_dir.path())
            .env("WLR_BACKENDS", "headless")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            // Without this sway tries to auto-detect a real DRM device
            // even with WLR_BACKENDS=headless. Pin to "" to be safe.
            .env("WLR_DRM_DEVICES", "")
            .env("WLR_HEADLESS_OUTPUTS", opts.outputs.to_string())
            // Strip any inherited Wayland/X11 session so sway doesn't
            // try to act as a nested compositor or pick up the dev
            // box's DISPLAY.
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .env_remove("DISPLAY");
        cmd.arg("-c").arg(&config_path);
        // Suppress sway's own stdout/stderr unless the test wants it.
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());

        let mut sway = cmd.spawn().map_err(|e| {
            // Distinguish "sway not on PATH" (test should skip) from
            // other spawn failures.
            if e.kind() == std::io::ErrorKind::NotFound {
                HarnessError::SwayUnavailable(e)
            } else {
                HarnessError::SpawnFailed(e)
            }
        })?;

        let deadline = Instant::now() + opts.start_timeout;
        loop {
            if let Some(name) = find_wayland_socket(runtime_dir.path())? {
                return Ok(Self {
                    runtime_dir,
                    display_name: name,
                    sway,
                });
            }
            // If sway died before creating the socket, surface its
            // stderr so the test can debug it.
            if let Some(_status) = sway.try_wait()? {
                let mut stderr = String::new();
                if let Some(mut e) = sway.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut stderr);
                }
                return Err(HarnessError::SwayExitedEarly { stderr });
            }
            if Instant::now() > deadline {
                let _ = sway.kill();
                let _ = sway.wait();
                // Drain whatever sway wrote to stderr so the test can
                // diagnose. SwayStartTimeout previously hid this.
                let mut stderr = String::new();
                if let Some(mut e) = sway.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut stderr);
                }
                eprintln!("sway start timeout. captured stderr:\n{stderr}");
                return Err(HarnessError::SwayStartTimeout);
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Path to the runtime dir sway is using. Children that want to
    /// connect to this compositor need `XDG_RUNTIME_DIR` set to this.
    pub fn runtime_dir(&self) -> &Path {
        self.runtime_dir.path()
    }

    /// Wayland display name to set on children. Pair with
    /// [`runtime_dir`](Self::runtime_dir) on `XDG_RUNTIME_DIR`.
    pub fn display(&self) -> &str {
        &self.display_name
    }

    /// Apply the env vars needed to make a child talk to this
    /// compositor. Used by [`spawn_observer`](Self::spawn_observer)
    /// and [`run_wdotool`](Self::run_wdotool); call manually for any
    /// additional child you spawn yourself.
    pub fn apply_env(&self, cmd: &mut Command) {
        apply_env_to(cmd, self.runtime_dir.path(), &self.display_name);
    }

    /// Spawn the `wdotool-observer` binary inside this compositor and
    /// return a handle to read its event stream.
    pub fn spawn_observer(&self) -> Result<Observer, HarnessError> {
        spawn_observer_with(self.runtime_dir.path(), &self.display_name)
    }

    /// Run `wdotool <args>` against this compositor, forcing the
    /// wlroots backend (which is the only sender backend that works in
    /// a headless wlroots compositor without a portal). Returns the
    /// completed [`std::process::Output`] so tests can assert on
    /// stdout, stderr, and exit status.
    pub fn run_wdotool(&self, args: &[&str]) -> Result<std::process::Output, HarnessError> {
        run_wdotool_with(self.runtime_dir.path(), &self.display_name, args)
    }

    /// Spawn `wdotool prime` against this compositor. The returned
    /// [`Prime`] handle holds the wlroots virtual_keyboard +
    /// virtual_pointer alive until dropped (which sends SIGTERM and
    /// waits for clean release). Blocks until prime prints `ready` to
    /// stdout, so the caller knows the seat caps are up before it
    /// proceeds.
    pub fn spawn_prime(&self) -> Result<Prime, HarnessError> {
        let bin = std::env::var_os("CARGO_BIN_EXE_wdotool")
            .map(PathBuf::from)
            .or_else(default_wdotool_path)
            .ok_or_else(|| {
                HarnessError::SpawnFailed(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "could not locate wdotool binary",
                ))
            })?;

        let mut cmd = Command::new(bin);
        self.apply_env(&mut cmd);
        cmd.args(["--backend", "wlroots", "prime"]);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(HarnessError::SpawnFailed)?;

        // Block until prime prints `ready` (or it dies). 5s budget;
        // prime is essentially as fast as any wdotool invocation
        // because it just builds the wlroots backend and prints.
        let stdout = child.stdout.take().expect("piped");
        let mut reader = BufReader::new(stdout);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    return Err(HarnessError::SpawnFailed(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "prime exited before printing ready",
                    )))
                }
                Ok(_) => {
                    if line.trim() == "ready" {
                        return Ok(Prime { child });
                    }
                }
                Err(_) if Instant::now() > deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(HarnessError::SpawnFailed(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "timed out waiting for `wdotool prime` to print ready",
                    )));
                }
                Err(_) => continue,
            }
        }
    }
}

impl Drop for HeadlessSway {
    fn drop(&mut self) {
        let _ = self.sway.kill();
        let _ = self.sway.wait();
    }
}

/// Handle to a running `wdotool prime` subprocess. While alive, the
/// wlroots virtual_keyboard + virtual_pointer stay registered on the
/// compositor's seat, so observer clients in the same session keep
/// the keyboard/pointer cap visible without having to rebind on every
/// transient `wdotool` invocation. Drop sends SIGTERM and waits.
pub struct Prime {
    child: Child,
}

impl Drop for Prime {
    fn drop(&mut self) {
        // Try a polite SIGTERM first via libc::kill; fall back to
        // Child::kill (SIGKILL) if that fails. Either way we wait
        // before returning so the compositor sees the device removal.
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        // Give it a beat to clean up; then make sure it's gone.
        let _ = self.child.wait();
    }
}

// ============================================================
// Helpers shared between the compositor runner's instance methods.
// Lifted out of `impl HeadlessSway` so additional runners can call
// the same machinery without copy-pasting; right now sway is the
// only target (weston dropped due to missing zwlr_virtual_pointer).
// ============================================================

fn apply_env_to(cmd: &mut Command, runtime_dir: &Path, display: &str) {
    cmd.env("XDG_RUNTIME_DIR", runtime_dir);
    cmd.env("WAYLAND_DISPLAY", display);
    cmd.env_remove("DISPLAY");
    cmd.env_remove("WAYLAND_SOCKET");
}

fn spawn_observer_with(runtime_dir: &Path, display: &str) -> Result<Observer, HarnessError> {
    let bin = std::env::var_os("CARGO_BIN_EXE_wdotool-observer")
        .map(PathBuf::from)
        .or_else(default_observer_path)
        .ok_or_else(|| {
            HarnessError::SpawnFailed(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "could not locate wdotool-observer binary",
            ))
        })?;

    let mut cmd = Command::new(bin);
    apply_env_to(&mut cmd, runtime_dir, display);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(HarnessError::SpawnFailed)?;

    let stdout = child.stdout.take().expect("piped");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    Ok(Observer { child, lines: rx })
}

fn run_wdotool_with(
    runtime_dir: &Path,
    display: &str,
    args: &[&str],
) -> Result<std::process::Output, HarnessError> {
    let bin = std::env::var_os("CARGO_BIN_EXE_wdotool")
        .map(PathBuf::from)
        .or_else(default_wdotool_path)
        .ok_or_else(|| {
            HarnessError::SpawnFailed(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "could not locate wdotool binary",
            ))
        })?;

    let mut cmd = Command::new(bin);
    apply_env_to(&mut cmd, runtime_dir, display);
    cmd.args(["--backend", "wlroots"]);
    cmd.args(args);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.output().map_err(HarnessError::SpawnFailed)
}

/// Knobs for [`HeadlessSway::start_with`]. Defaults are the right
/// answer for most tests; use this when you need a multi-output scene
/// or a longer startup window.
pub struct StartOptions {
    /// How long to wait for sway's Wayland socket to appear before
    /// giving up. Default 15 seconds: sway typically comes up in well
    /// under one, but `cargo test --workspace` runs every test binary
    /// in parallel and these integration tests fight every other
    /// suite for CPU. The timeout only costs anything when sway is
    /// genuinely broken.
    pub start_timeout: Duration,
    /// Number of fake outputs the headless backend creates (sets
    /// `WLR_HEADLESS_OUTPUTS`). Default 1; bump to 2 for multi-output
    /// tests.
    pub outputs: u32,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            start_timeout: Duration::from_secs(15),
            outputs: 1,
        }
    }
}

/// Handle to a running `wdotool-observer` subprocess. Reads events
/// from its stdout via a background thread, exposed as a channel.
pub struct Observer {
    child: Child,
    lines: Receiver<String>,
}

impl Observer {
    /// Block until the observer has emitted `ready` (meaning its
    /// surface is mapped and the compositor will deliver input to it).
    /// Returns the lines emitted up to and including `ready`, or
    /// `Err(ObserverNotReady)` on timeout.
    pub fn wait_for_ready(&self, timeout: Duration) -> Result<Vec<String>, HarnessError> {
        let deadline = Instant::now() + timeout;
        let mut prelude = Vec::new();
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(HarnessError::ObserverNotReady)?;
            match self.lines.recv_timeout(remaining) {
                Ok(line) => {
                    let is_ready = line == "ready";
                    prelude.push(line);
                    if is_ready {
                        return Ok(prelude);
                    }
                }
                Err(RecvTimeoutError::Timeout) => return Err(HarnessError::ObserverNotReady),
                Err(RecvTimeoutError::Disconnected) => return Err(HarnessError::ObserverNotReady),
            }
        }
    }

    /// Drain every event line the observer produced within `window`
    /// of now, then return them. Useful after sending input: wait a
    /// short window for events to arrive, then assert.
    pub fn collect_events(&self, window: Duration) -> Vec<String> {
        let deadline = Instant::now() + window;
        let mut events = Vec::new();
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                break;
            }
            match self.lines.recv_timeout(remaining) {
                Ok(line) => events.push(line),
                Err(_) => break,
            }
        }
        events
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Walk the runtime dir looking for the `wayland-N` socket file sway
/// created via `wl_display_add_socket_auto`. Returns the socket name
/// (e.g. "wayland-1") so the caller can set `WAYLAND_DISPLAY` for
/// child clients.
fn find_wayland_socket(dir: &Path) -> std::io::Result<Option<String>> {
    if !dir.exists() {
        return Ok(None);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str() {
            // Sway writes both `wayland-N` (the socket) and
            // `wayland-N.lock`. We want the socket itself, identified
            // by being a unix-domain socket via file_type().
            if name.starts_with("wayland-") && !name.ends_with(".lock") {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_socket() {
                        return Ok(Some(name.to_string()));
                    }
                }
            }
        }
    }
    Ok(None)
}

// ============================================================
// Path helpers for non-`cargo test` callers (e.g. a debug script that
// uses the runner outside the test framework).
// ============================================================

fn default_observer_path() -> Option<PathBuf> {
    sibling_exe("wdotool-observer")
}

fn default_wdotool_path() -> Option<PathBuf> {
    sibling_exe("wdotool")
}

fn sibling_exe(name: &str) -> Option<PathBuf> {
    let cur = std::env::current_exe().ok()?;
    let dir = cur.parent()?;
    let candidate = dir.join(name);
    if candidate.exists() {
        Some(candidate)
    } else {
        // Walk up to find a workspace target/{debug,release}/<name>.
        let mut at = dir;
        while let Some(parent) = at.parent() {
            for sub in ["debug", "release"] {
                let c = parent.join(sub).join(name);
                if c.exists() {
                    return Some(c);
                }
            }
            at = parent;
        }
        None
    }
}
