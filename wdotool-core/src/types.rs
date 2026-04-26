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
