#[derive(Clone, Debug)]
pub struct Capabilities {
    pub key_input: bool,
    pub text_input: bool,
    pub pointer_move_absolute: bool,
    pub pointer_move_relative: bool,
    pub pointer_button: bool,
    pub scroll: bool,
    pub list_windows: bool,
    pub active_window: bool,
    pub activate_window: bool,
    pub close_window: bool,
    pub pointer_position: bool,
    /// True when the backend can enumerate outputs and `mousemove
    /// --output <name>` will translate output-local to global coords.
    /// wlroots: yes (foreign-toplevel + wl_output). KDE / GNOME / libei:
    /// not yet (each compositor has a different way to enumerate
    /// monitors). uinput: never (kernel layer, no notion of screens).
    pub list_outputs: bool,
    /// True when the backend can read a window's frame position and
    /// size for `wdotool getwindowgeometry`. KDE (kwin script) and
    /// GNOME (Shell extension) can; wlroots' foreign-toplevel doesn't
    /// expose geometry, libei and uinput have no window concept.
    pub window_geometry: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyDirection {
    Press,
    Release,
    PressRelease,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Back,
    Forward,
    Other(u32),
}

impl MouseButton {
    // xdotool indices: 1=left, 2=middle, 3=right, 4/5=scroll (handled elsewhere),
    // 8=back, 9=forward. Unknowns pass through as Other.
    pub fn from_index(n: u32) -> Self {
        match n {
            1 => Self::Left,
            2 => Self::Middle,
            3 => Self::Right,
            8 => Self::Back,
            9 => Self::Forward,
            _ => Self::Other(n),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub String);

impl std::fmt::Display for WindowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: String,
    pub app_id: Option<String>,
    pub pid: Option<u32>,
}

/// A monitor / display the compositor exposes. Coordinates are in
/// compositor "logical" pixels — the same coordinate space that
/// `mousemove` (absolute) targets. `mousemove --output <name>` adds
/// `(x, y)` to the user-supplied coordinates, so a window in the
/// top-left of `DP-1` is at `(x, y)` globally regardless of how the
/// monitors are arranged.
#[derive(Clone, Debug)]
pub struct OutputInfo {
    /// wl_output name, e.g. `DP-1`, `HDMI-A-1`, `eDP-1`. The string
    /// `wdotool mousemove --output <NAME>` matches against.
    pub name: String,
    /// Origin of this output in compositor coordinates.
    pub x: i32,
    pub y: i32,
    /// Current mode dimensions in pixels.
    pub width: u32,
    pub height: u32,
    /// Integer scale factor (1, 2, 3...). Fractional scaling is
    /// reported by compositors as the next integer up; the per-output
    /// fractional value is not exposed to wl_output clients.
    pub scale: i32,
}

/// Position and size of a window's frame in compositor coordinates,
/// matching xdotool's `getwindowgeometry` semantics. Backends that
/// can't read window geometry (wlroots' foreign-toplevel doesn't
/// expose it; libei and uinput have no window concept) return `None`
/// instead of populating this. KDE reads it via a transient kwin
/// script; GNOME via the companion Shell extension.
#[derive(Clone, Debug)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
