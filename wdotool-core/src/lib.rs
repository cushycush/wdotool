//! Wayland input/window primitives — the engine library for wdotool.
//!
//! Public API:
//! - [`Backend`] / [`DynBackend`]: the trait and box-erased handle every
//!   backend implements (input + window operations).
//! - [`detector::build`] / [`detector::Environment`]: detect the running
//!   compositor and return a ready-to-use backend.
//! - [`types::*`]: input/window value types ([`Capabilities`],
//!   [`KeyDirection`], [`MouseButton`], [`WindowId`], [`WindowInfo`]).
//! - [`error::WdoError`] / [`error::Result`]: the error type returned by
//!   every fallible call.
//! - [`keysym`]: the chord parser used by the CLI (`ctrl+shift+a` etc.).
//!
//! Per-backend modules ([`backend::libei`], [`backend::wlroots`], ...)
//! are gated behind Cargo features. Default features enable all five.
//! Downstream consumers that don't need a particular backend (e.g. a
//! sandboxed Flatpak that excludes `uinput`) can opt out via
//! `default-features = false` + a custom feature list.

pub mod backend;
pub mod error;
pub mod keysym;
pub mod types;

pub use backend::detector;
pub use backend::{Backend, DynBackend};
pub use error::{Result, WdoError};
pub use types::{Capabilities, KeyDirection, MouseButton, WindowId, WindowInfo};
