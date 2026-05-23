//! UNIX socket discovery and connect for Drift's IPC.
//!
//! Path: `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock`.
//! Drift currently inherits `HYPRLAND_INSTANCE_SIGNATURE` from its
//! Hyprland heritage; if a future Drift sets `DRIFT_INSTANCE_SIGNATURE`
//! (DRIFT-228) we pick that up too.

use std::path::PathBuf;

use super::EventError;

pub(crate) fn discover_socket_path() -> Result<PathBuf, EventError> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|s| !s.is_empty())
        .ok_or(EventError::MissingEnv("XDG_RUNTIME_DIR"))?;
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var_os("DRIFT_INSTANCE_SIGNATURE").filter(|s| !s.is_empty()))
        .ok_or(EventError::MissingEnv("HYPRLAND_INSTANCE_SIGNATURE"))?;
    let mut path = PathBuf::from(runtime_dir);
    path.push("hypr");
    path.push(signature);
    path.push(".socket2.sock");
    Ok(path)
}
