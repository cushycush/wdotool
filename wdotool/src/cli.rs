use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "wdotool",
    version,
    about = "xdotool-compatible automation for Wayland",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Force a specific backend (libei, wlroots, kde, gnome, uinput).
    #[arg(long, global = true)]
    pub backend: Option<String>,

    /// Enable debug logging.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Press and release a key chain (e.g. "ctrl+c").
    Key {
        /// Release stuck modifiers (Ctrl/Shift/Alt/Super/AltGr) before the op.
        /// Approximates xdotool's --clearmodifiers — unlike xdotool we can't
        /// observe current modifier state on Wayland, so this unconditionally
        /// releases the standard set.
        #[arg(long)]
        clearmodifiers: bool,
        chain: String,
    },

    /// Press a key chain without releasing.
    Keydown {
        #[arg(long)]
        clearmodifiers: bool,
        chain: String,
    },

    /// Release a previously pressed key chain.
    Keyup {
        #[arg(long)]
        clearmodifiers: bool,
        chain: String,
    },

    /// Type a literal string.
    Type {
        /// Delay between characters in milliseconds.
        #[arg(long, default_value_t = 12)]
        delay: u64,
        /// Read the text from a file instead of the positional arg.
        /// Use `-` to read from stdin. Mutually exclusive with the text arg.
        #[arg(long, conflicts_with = "text")]
        file: Option<String>,
        /// See `key --clearmodifiers`.
        #[arg(long)]
        clearmodifiers: bool,
        text: Option<String>,
    },

    /// Move the mouse to (x, y) or by (dx, dy) with --relative.
    Mousemove {
        #[arg(long)]
        relative: bool,
        /// Treat (x, y) as relative to this output's origin instead of
        /// the global compositor coordinate space. Use the name from
        /// `wdotool outputs` (e.g. `DP-1`, `HDMI-A-1`). Conflicts with
        /// `--relative`. Currently only the wlroots backend enumerates
        /// outputs; other backends error if you pass `--output`.
        #[arg(long, conflicts_with = "relative")]
        output: Option<String>,
        x: i32,
        y: i32,
    },

    /// Press and release a mouse button by xdotool index (1=left, 2=middle, 3=right).
    Click { button: u32 },

    /// Press a mouse button.
    Mousedown { button: u32 },

    /// Release a mouse button.
    Mouseup { button: u32 },

    /// Scroll. Positive dy scrolls down; positive dx scrolls right.
    Scroll { dx: f64, dy: f64 },

    /// List windows matching the filters. With no filter, lists all
    /// windows. Exits with status 1 if no windows matched, status 0
    /// otherwise (matches xdotool's behavior so `if wdotool search ...`
    /// works in shell scripts).
    Search {
        /// Substring (or regex with --regex) matched against the
        /// window title.
        #[arg(long)]
        name: Option<String>,
        /// Substring (or regex with --regex) matched against the
        /// Wayland app_id (the closest equivalent to X11's WM_CLASS).
        #[arg(long)]
        class: Option<String>,
        /// Match windows owned by this exact PID. Backends that can't
        /// resolve a PID for a window will never match this filter.
        #[arg(long)]
        pid: Option<u32>,
        /// Treat --name and --class values as regular expressions
        /// instead of plain substrings. Without this flag, the
        /// patterns are escaped before matching, so `Fire.fox` matches
        /// only that literal string.
        #[arg(long)]
        regex: bool,
        /// Case-insensitive matching for --name and --class. Works in
        /// both substring and regex modes.
        #[arg(long)]
        ignore_case: bool,
        /// Combine filters with OR instead of the default AND. Without
        /// this, a window must match every set filter. With this, a
        /// window matching at least one set filter is included.
        /// Mirrors xdotool's `--any`. Conflicts with `--all`.
        #[arg(long, conflicts_with = "all")]
        any: bool,
        /// Combine filters with AND. This is already the default; the
        /// flag exists so xdotool scripts that explicitly pass `--all`
        /// keep working unchanged.
        #[arg(long)]
        all: bool,
    },

    /// Print the active window's id.
    Getactivewindow,

    /// Print the current pointer position as `x:N y:N` (xdotool's
    /// default format). Exits 1 on backends that can't read pointer
    /// position (libei, wlroots, uinput); KDE and GNOME both can.
    Getmouselocation,

    /// List the compositor's outputs (monitors). One row per output:
    /// name, x, y, width, height, scale. Pass `--json` for structured
    /// output. Currently only the wlroots backend enumerates outputs;
    /// others exit 0 with empty output.
    Outputs {
        /// Emit JSON instead of the default tab-separated table.
        #[arg(long)]
        json: bool,
    },

    /// Activate (raise + focus) a window by id.
    Windowactivate { id: String },

    /// Close a window by id.
    Windowclose { id: String },

    /// Print the title of the window with the given id.
    Getwindowname { id: String },

    /// Print the PID of the window with the given id. Exits 1 if the
    /// backend can't resolve a PID for that window (some compositors
    /// don't expose it).
    Getwindowpid { id: String },

    /// Print the app_id of the window with the given id. This is the
    /// Wayland equivalent of xdotool's WM_CLASS classname. Exits 1 if
    /// the window has no app_id set.
    Getwindowclassname { id: String },

    /// Print the frame position and size of a window by id. Output
    /// matches xdotool's default format ("Window <id>" header,
    /// position, geometry). Exits 1 if the id doesn't exist or the
    /// active backend can't read window geometry (libei / wlroots /
    /// uinput); KDE and GNOME both can.
    Getwindowgeometry { id: String },

    /// Show detected environment and backend capabilities.
    Info,

    /// Print a structured capabilities report (schema v1) as JSON.
    /// This is the machine-readable cousin of `info`. The schema is
    /// documented at `docs/capabilities-schema.json` and is the
    /// contract that wflows.com and other downstream tools consume.
    Capabilities,

    /// Print an environment + backend availability report. Use this
    /// when a wdotool command isn't behaving the way you expect; the
    /// output names the missing piece (portal? group? extension?) and
    /// prints the fix command. Pass `--copy` to send the report to
    /// the clipboard, `--json` for machine-readable output.
    Diag {
        /// Emit machine-readable JSON instead of markdown.
        #[arg(long)]
        json: bool,
        /// Copy the markdown report to the clipboard via wl-copy
        /// (falls back to xclip).
        #[arg(long)]
        copy: bool,
    },

    /// Capture user input until Ctrl-C (or `--max-duration` elapses)
    /// and write the events as JSON. Three capture sources are
    /// available: the XDG RemoteDesktop portal (default on Plasma 6 /
    /// GNOME 46+), `/dev/input/event*` via evdev (works on any
    /// compositor if you're in the `input` group), and a deterministic
    /// test source. With no `--backend` argument, portal is tried
    /// first, then evdev.
    #[cfg(feature = "recorder")]
    Record {
        /// Path to write the captured events as JSON. Default is
        /// stdout. Use a real path to capture without the events
        /// also being printed to the terminal.
        #[arg(long, short = 'o')]
        output: Option<String>,
        /// Stop automatically after this many seconds. Useful for
        /// scripted captures; without it the recording runs until
        /// Ctrl-C.
        #[arg(long)]
        max_duration: Option<u64>,
        /// Force a specific capture source. `auto` (the default)
        /// cascades portal -> evdev. `simulated` plays a deterministic
        /// seven-event script and is for tests / CI.
        #[arg(long, default_value = "auto")]
        backend: String,
    },
}
