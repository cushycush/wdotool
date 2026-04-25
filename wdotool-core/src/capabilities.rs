//! Capabilities report: what does this wdotool installation support?
//!
//! This module is the canonical exit point for "tell me about this
//! wdotool" data. The shape is locked by the JSON Schema at
//! `docs/capabilities-schema.json` and the rules baked in there:
//! `schema_version` is `1` for this file; adding new fields is
//! backward-compat (consumers must ignore unknown fields); removing,
//! renaming, or narrowing types requires a `schema_version` bump and
//! a separate `capabilities/v2.json` schema file.
//!
//! Public entry points:
//! - [`report`] returns a typed [`CapabilitiesReport`] for callers
//!   that want struct access.
//! - [`report_json`] returns the same shape as a `serde_json::Value`,
//!   matching the eng plan's `Capabilities::to_schema_v1() -> Value`
//!   contract. wflows.com and other JSON consumers go through this.

use serde::{Deserialize, Serialize};

use crate::backend::Backend;
use crate::detector::{priority, BackendKind, Environment};
use crate::types::Capabilities;

/// Locked at 1 for this schema. See `docs/capabilities-schema.json`.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesReport {
    pub schema_version: u32,
    pub wdotool_version: String,
    pub backend: BackendInfo,
    pub input: InputCaps,
    pub window: WindowCaps,
    pub extras: Extras,
    pub platform: PlatformInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    pub selected: String,
    pub kind: BackendKindLabel,
    pub delegated_to: Option<String>,
    pub fallback_chain: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKindLabel {
    Direct,
    Daemon,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputCaps {
    pub key: bool,
    pub type_text: bool,
    pub type_unicode: TypeUnicode,
    pub mouse_move_absolute: bool,
    pub mouse_move_relative: bool,
    pub mouse_button: bool,
    pub scroll: bool,
    pub modifiers: ModifiersCap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeUnicode {
    /// Arbitrary Unicode including the astral plane. Currently only
    /// the wlroots backend, which uploads a transient xkb keymap.
    Full,
    /// Basic Multilingual Plane (BMP) characters work; astral chars
    /// (e.g. emoji beyond U+FFFF) are dropped.
    BmpOnly,
    /// Only ASCII works. Non-ASCII chars are skipped with a warning.
    /// libei / kde / gnome / uinput sit here because the EIS server
    /// or the kernel owns the keymap and we cannot install our own.
    AsciiOnly,
    /// No `type` support at all.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModifiersCap {
    /// We can send modifier press/release but cannot read the current
    /// modifier state from the compositor. Wayland's security model
    /// does not expose that read; v0.2.0 emits this for every backend.
    SendOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowCaps {
    pub list: bool,
    pub active: bool,
    pub activate: bool,
    pub close: bool,
    pub match_by: Vec<MatchBy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchBy {
    Title,
    AppId,
    Pid,
    Class,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extras {
    pub diag: bool,
    pub outputs: bool,
    pub record: RecordCaps,
    pub json_output: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordCaps {
    pub supported: bool,
    pub source: Option<RecordSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordSource {
    LibeiReceiver,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformInfo {
    pub desktop: Option<String>,
    pub session_type: Option<String>,
    pub compositor_hints: Vec<String>,
}

/// Build the typed report for the current environment + selected backend.
pub fn report(env: &Environment, backend: &dyn Backend) -> CapabilitiesReport {
    let caps: Capabilities = backend.capabilities();
    let backend_name = backend.name().to_string();

    CapabilitiesReport {
        schema_version: SCHEMA_VERSION,
        wdotool_version: env!("CARGO_PKG_VERSION").to_string(),
        backend: BackendInfo {
            selected: backend_name.clone(),
            // v0.2.0 only ships direct-mode backends. Daemon mode is
            // a v0.4.0+ shape; the schema reserves the enum value so
            // future consumers can branch cleanly.
            kind: BackendKindLabel::Direct,
            delegated_to: None,
            fallback_chain: priority(env)
                .into_iter()
                .map(backend_kind_label)
                .map(str::to_string)
                .collect(),
        },
        input: InputCaps {
            key: caps.key_input,
            type_text: caps.text_input,
            type_unicode: type_unicode_for(&backend_name),
            mouse_move_absolute: caps.pointer_move_absolute,
            mouse_move_relative: caps.pointer_move_relative,
            mouse_button: caps.pointer_button,
            scroll: caps.scroll,
            // Wayland clients cannot read the compositor's current
            // modifier state. Every backend is send-only in v0.2.0;
            // the enum is forward-compat for a future read-capable
            // backend.
            modifiers: ModifiersCap::SendOnly,
        },
        window: WindowCaps {
            list: caps.list_windows,
            active: caps.active_window,
            activate: caps.activate_window,
            close: caps.close_window,
            // v0.2.0 only matches by title (`wdotool search --name`).
            // The `match_by` array can grow without a schema bump
            // when class / app_id / pid matchers land.
            match_by: vec![MatchBy::Title],
        },
        extras: Extras {
            diag: true,
            outputs: false,
            record: RecordCaps {
                supported: false,
                source: None,
            },
            // wdotool diag --json + wdotool capabilities both emit
            // structured output, so the flag is true.
            json_output: true,
        },
        platform: PlatformInfo {
            desktop: env.desktop.clone(),
            session_type: env.session_type.clone(),
            compositor_hints: env
                .compositor_hints
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        },
    }
}

/// Same shape as [`report`] but emitted as a `serde_json::Value`.
/// Matches the `Capabilities::to_schema_v1() -> Value` contract from
/// the v0.2.0 eng plan; wflows.com and other JSON-only consumers
/// should use this entry point so we do not commit to the in-memory
/// Rust type as part of the cross-language API.
pub fn report_json(env: &Environment, backend: &dyn Backend) -> serde_json::Value {
    serde_json::to_value(report(env, backend))
        .expect("CapabilitiesReport derives Serialize and contains no maps with non-string keys")
}

/// Map a [`BackendKind`] to its canonical string label, matching the
/// names the schema expects in `backend.fallback_chain`.
fn backend_kind_label(kind: BackendKind) -> &'static str {
    match kind {
        BackendKind::Libei => "libei",
        BackendKind::Wlroots => "wlroots",
        BackendKind::KdeDBus => "kde",
        BackendKind::GnomeExt => "gnome",
        BackendKind::Uinput => "uinput",
    }
}

/// Per-backend Unicode-typing capability. Hardcoded because the
/// `Capabilities` struct does not carry this info. Real-world testing
/// on KDE / GNOME may refine these mappings; that refinement bumps
/// individual values, not the schema_version (the enum is stable).
fn type_unicode_for(backend_name: &str) -> TypeUnicode {
    match backend_name {
        "wlroots" => TypeUnicode::Full,
        "libei" | "kde" | "gnome" | "uinput" => TypeUnicode::AsciiOnly,
        _ => TypeUnicode::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::time::Duration;

    use crate::error::Result;
    use crate::types::{KeyDirection, MouseButton, WindowId, WindowInfo};

    /// Minimal fake backend for capabilities tests. Returns a fixed
    /// `name` and a fully-on `Capabilities` struct so the report's
    /// shape can be inspected without booting a real backend.
    struct FakeBackend {
        name: &'static str,
    }

    #[async_trait]
    impl Backend for FakeBackend {
        fn name(&self) -> &'static str {
            self.name
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                key_input: true,
                text_input: true,
                pointer_move_absolute: true,
                pointer_move_relative: true,
                pointer_button: true,
                scroll: true,
                list_windows: true,
                active_window: true,
                activate_window: true,
                close_window: true,
            }
        }
        async fn key(&self, _: &str, _: KeyDirection) -> Result<()> {
            unimplemented!()
        }
        async fn type_text(&self, _: &str, _: Duration) -> Result<()> {
            unimplemented!()
        }
        async fn mouse_move(&self, _: i32, _: i32, _: bool) -> Result<()> {
            unimplemented!()
        }
        async fn mouse_button(&self, _: MouseButton, _: KeyDirection) -> Result<()> {
            unimplemented!()
        }
        async fn scroll(&self, _: f64, _: f64) -> Result<()> {
            unimplemented!()
        }
        async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
            unimplemented!()
        }
        async fn active_window(&self) -> Result<Option<WindowInfo>> {
            unimplemented!()
        }
        async fn activate_window(&self, _: &WindowId) -> Result<()> {
            unimplemented!()
        }
        async fn close_window(&self, _: &WindowId) -> Result<()> {
            unimplemented!()
        }
    }

    fn fake_env() -> Environment {
        Environment {
            desktop: Some("GNOME".into()),
            session_type: Some("wayland".into()),
            wayland_display: Some("wayland-0".into()),
            compositor_hints: vec!["gnome-shell"],
        }
    }

    #[test]
    fn report_shape_matches_schema_v1() {
        let env = fake_env();
        let backend = FakeBackend { name: "libei" };
        let r = report(&env, &backend);
        assert_eq!(r.schema_version, 1);
        assert_eq!(r.backend.selected, "libei");
        assert_eq!(r.backend.kind, BackendKindLabel::Direct);
        assert!(r.backend.delegated_to.is_none());
        assert!(!r.backend.fallback_chain.is_empty());
        assert_eq!(r.input.modifiers, ModifiersCap::SendOnly);
        assert_eq!(r.window.match_by, vec![MatchBy::Title]);
        assert!(r.extras.diag);
        assert!(!r.extras.outputs);
        assert!(!r.extras.record.supported);
        assert!(r.extras.record.source.is_none());
        assert!(r.extras.json_output);
        assert_eq!(r.platform.desktop.as_deref(), Some("GNOME"));
    }

    #[test]
    fn type_unicode_is_full_only_on_wlroots() {
        assert_eq!(type_unicode_for("wlroots"), TypeUnicode::Full);
        assert_eq!(type_unicode_for("libei"), TypeUnicode::AsciiOnly);
        assert_eq!(type_unicode_for("kde"), TypeUnicode::AsciiOnly);
        assert_eq!(type_unicode_for("gnome"), TypeUnicode::AsciiOnly);
        assert_eq!(type_unicode_for("uinput"), TypeUnicode::AsciiOnly);
        // Unknown backends fail closed: caller sees TypeUnicode::None.
        assert_eq!(type_unicode_for("future-backend"), TypeUnicode::None);
    }

    #[test]
    fn report_json_round_trips_through_serde() {
        let env = fake_env();
        let backend = FakeBackend { name: "wlroots" };
        let value = report_json(&env, &backend);
        // schema_version pins to 1 in the JSON shape.
        assert_eq!(value["schema_version"], 1);
        // type_unicode serializes as snake_case "full" for wlroots.
        assert_eq!(value["input"]["type_unicode"], "full");
        // Closed enum values come through as their schema strings.
        assert_eq!(value["backend"]["kind"], "direct");
        assert_eq!(value["input"]["modifiers"], "send-only");
        // match_by is an array of snake_case strings.
        assert_eq!(value["window"]["match_by"], serde_json::json!(["title"]));
        // record.source is null in v0.2.0.
        assert!(value["extras"]["record"]["source"].is_null());
    }

    #[test]
    fn fallback_chain_uses_canonical_backend_labels() {
        let env = fake_env();
        let backend = FakeBackend { name: "gnome" };
        let r = report(&env, &backend);
        // Every entry should be one of the five known labels.
        let known = ["libei", "wlroots", "kde", "gnome", "uinput"];
        for entry in &r.backend.fallback_chain {
            assert!(
                known.contains(&entry.as_str()),
                "unexpected fallback_chain entry: {entry}"
            );
        }
    }
}
