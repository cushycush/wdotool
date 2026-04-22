use tracing::{debug, info, warn};

use super::gnome::GnomeExtBackend;
use super::kde::KdeBackend;
use super::libei::LibeiBackend;
use super::uinput::UinputBackend;
use super::wlroots::WlrootsBackend;
use super::DynBackend;
use crate::error::{Result, WdoError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Libei,
    Wlroots,
    KdeDBus,
    GnomeExt,
    Uinput,
}

impl BackendKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Libei => "libei",
            Self::Wlroots => "wlroots",
            Self::KdeDBus => "kde-dbus",
            Self::GnomeExt => "gnome-ext",
            Self::Uinput => "uinput",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "libei" => Some(Self::Libei),
            "wlroots" | "wlr" => Some(Self::Wlroots),
            "kde" | "kde-dbus" | "kwin" => Some(Self::KdeDBus),
            "gnome" | "gnome-ext" | "gnome-shell" => Some(Self::GnomeExt),
            "uinput" => Some(Self::Uinput),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Environment {
    pub desktop: Option<String>,
    pub session_type: Option<String>,
    pub wayland_display: Option<String>,
    pub compositor_hints: Vec<&'static str>,
}

impl Environment {
    pub fn detect() -> Self {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok();
        let session_type = std::env::var("XDG_SESSION_TYPE").ok();
        let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();

        let mut hints: Vec<&'static str> = Vec::new();
        if std::env::var_os("SWAYSOCK").is_some() {
            hints.push("sway");
        }
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            hints.push("hyprland");
        }
        if std::env::var_os("WAYFIRE_CONFIG_FILE").is_some() {
            hints.push("wayfire");
        }

        Self {
            desktop,
            session_type,
            wayland_display,
            compositor_hints: hints,
        }
    }

    pub fn is_wayland(&self) -> bool {
        self.session_type.as_deref() == Some("wayland") || self.wayland_display.is_some()
    }

    pub fn desktop_is(&self, needle: &str) -> bool {
        self.desktop
            .as_deref()
            .map(|d| d.split(':').any(|s| s.eq_ignore_ascii_case(needle)))
            .unwrap_or(false)
    }

    pub fn has_hint(&self, needle: &str) -> bool {
        self.compositor_hints.contains(&needle)
    }
}

/// Produce a preference-ordered list of backends for this environment.
/// The first entry is the "best" choice; the rest are fallbacks.
pub fn priority(env: &Environment) -> Vec<BackendKind> {
    let mut order: Vec<BackendKind> = Vec::new();

    let is_wlroots = env.has_hint("sway")
        || env.has_hint("hyprland")
        || env.has_hint("wayfire")
        || env.desktop_is("sway")
        || env.desktop_is("Hyprland");

    if is_wlroots {
        // wlroots compositors expose protocols libei can't match on these hosts
        order.push(BackendKind::Wlroots);
        order.push(BackendKind::Libei);
    } else if env.desktop_is("KDE") {
        // KdeDBus composes libei input + KWin scripting windows, so it's a
        // strict superset of bare libei on Plasma sessions.
        order.push(BackendKind::KdeDBus);
        order.push(BackendKind::Libei);
        order.push(BackendKind::Wlroots);
    } else if env.desktop_is("GNOME") {
        // GnomeExt (libei input + Shell-extension window bridge) is a strict
        // superset of libei on GNOME — when the extension is installed. If
        // its init fails, build() falls through to bare libei automatically.
        order.push(BackendKind::GnomeExt);
        order.push(BackendKind::Libei);
        order.push(BackendKind::Wlroots);
    } else {
        // Other portal-capable sessions prefer bare libei.
        order.push(BackendKind::Libei);
        order.push(BackendKind::Wlroots);
    }

    order.push(BackendKind::Uinput);

    let mut deduped: Vec<BackendKind> = Vec::with_capacity(order.len());
    for k in order {
        if !deduped.contains(&k) {
            deduped.push(k);
        }
    }
    deduped
}

pub async fn build(env: &Environment, forced: Option<BackendKind>) -> Result<DynBackend> {
    if !env.is_wayland() {
        debug!(?env.session_type, ?env.wayland_display, "no wayland session detected");
    }

    match forced {
        Some(k) => {
            info!(backend = k.label(), "using forced backend");
            build_one(k).await
        }
        None => {
            // Walk the preference list; if the preferred backend fails to
            // bootstrap (portal unavailable, timeout, etc.) fall through to
            // the next. Only the final failure propagates.
            let order = priority(env);
            let mut last_err: Option<WdoError> = None;
            for kind in order {
                info!(backend = kind.label(), "trying backend");
                match build_one(kind).await {
                    Ok(b) => return Ok(b),
                    Err(err) => {
                        warn!(
                            backend = kind.label(),
                            ?err,
                            "backend unavailable, trying next"
                        );
                        last_err = Some(err);
                    }
                }
            }
            Err(last_err.unwrap_or(WdoError::NoBackend))
        }
    }
}

async fn build_one(kind: BackendKind) -> Result<DynBackend> {
    match kind {
        BackendKind::Libei => Ok(Box::new(LibeiBackend::try_new().await?)),
        BackendKind::Wlroots => Ok(Box::new(WlrootsBackend::try_new().await?)),
        BackendKind::KdeDBus => Ok(Box::new(KdeBackend::try_new().await?)),
        BackendKind::GnomeExt => Ok(Box::new(GnomeExtBackend::try_new().await?)),
        BackendKind::Uinput => Ok(Box::new(UinputBackend::try_new()?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_prefers_wlroots_on_sway() {
        let env = Environment {
            desktop: Some("sway".into()),
            session_type: Some("wayland".into()),
            wayland_display: Some("wayland-0".into()),
            compositor_hints: vec!["sway"],
        };
        let order = priority(&env);
        assert_eq!(order.first().copied(), Some(BackendKind::Wlroots));
        assert!(order.contains(&BackendKind::Uinput));
    }

    #[test]
    fn priority_on_gnome_prefers_gnome_ext_then_falls_back_to_libei() {
        // GnomeExtBackend is a strict superset of libei on GNOME (input via
        // libei + windows via the Shell extension), so it leads. If the
        // extension isn't installed, build() auto-falls-through to libei.
        let env = Environment {
            desktop: Some("GNOME".into()),
            session_type: Some("wayland".into()),
            wayland_display: Some("wayland-0".into()),
            compositor_hints: vec![],
        };
        let order = priority(&env);
        assert_eq!(order.first().copied(), Some(BackendKind::GnomeExt));
        let libei_idx = order.iter().position(|k| *k == BackendKind::Libei);
        let gnome_idx = order.iter().position(|k| *k == BackendKind::GnomeExt);
        assert!(
            gnome_idx < libei_idx,
            "gnome-ext must come before libei: {order:?}"
        );
    }

    #[test]
    fn priority_on_kde_prefers_kde_dbus() {
        // KdeBackend composes libei input + KWin window scripting, so it
        // should outrank bare libei on Plasma sessions.
        let env = Environment {
            desktop: Some("KDE".into()),
            session_type: Some("wayland".into()),
            wayland_display: Some("wayland-0".into()),
            compositor_hints: vec![],
        };
        let order = priority(&env);
        assert_eq!(order.first().copied(), Some(BackendKind::KdeDBus));
        assert!(order.contains(&BackendKind::Libei));
    }

    #[test]
    fn desktop_is_handles_colon_list() {
        let env = Environment {
            desktop: Some("ubuntu:GNOME".into()),
            ..Default::default()
        };
        assert!(env.desktop_is("GNOME"));
        assert!(env.desktop_is("ubuntu"));
        assert!(!env.desktop_is("KDE"));
    }
}
