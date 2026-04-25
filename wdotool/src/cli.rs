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

    /// List windows matching the filters. With no filter, lists all windows.
    Search {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        class: Option<String>,
    },

    /// Print the active window's id.
    Getactivewindow,

    /// Activate (raise + focus) a window by id.
    Windowactivate { id: String },

    /// Close a window by id.
    Windowclose { id: String },

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
}
