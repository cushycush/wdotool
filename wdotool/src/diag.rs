//! `wdotool diag`: environment + backend availability report.
//!
//! Probes pre-conditions only. Never opens a portal session, so it will
//! not trigger a consent dialog. Output is markdown by default; pass
//! `--json` for a machine-readable shape, `--copy` to send the markdown
//! through `wl-copy` (or `xclip`).
//!
//! The point: when something doesn't work, the user runs `wdotool diag
//! --copy` and pastes the output into a bug report. No more guessing
//! whether the portal is missing, the user is in the wrong group, or
//! the GNOME extension is uninstalled.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use wdotool_core::detector::Environment;

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagFormat {
    Markdown,
    Json,
}

#[derive(Debug, Serialize)]
pub struct DiagReport {
    schema_version: u32,
    wdotool_version: &'static str,
    environment: EnvSection,
    backends: Vec<BackendSection>,
    suggested_fixes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct EnvSection {
    desktop: Option<String>,
    session_type: Option<String>,
    wayland_display: Option<String>,
    display: Option<String>,
    compositor_hints: Vec<&'static str>,
    portal_token_cache_path: Option<String>,
    portal_token_cached: bool,
}

#[derive(Debug, Serialize)]
pub struct BackendSection {
    name: &'static str,
    status: BackendStatus,
    detail: String,
    fix_hint: Option<String>,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BackendStatus {
    Available,
    Unavailable,
    Warning,
}

pub fn run(format: DiagFormat, copy: bool) -> anyhow::Result<()> {
    let report = build_report();
    let markdown = render_markdown(&report);
    let output = match format {
        DiagFormat::Markdown => markdown.clone(),
        DiagFormat::Json => serde_json::to_string_pretty(&report)?,
    };

    if copy {
        // --copy always pipes the markdown body, regardless of --json.
        // The markdown is what a human pastes into a bug report.
        match try_clipboard(&markdown) {
            Ok(tool) => {
                eprintln!("Copied diag output to clipboard via {tool}.");
                if format == DiagFormat::Json {
                    // User asked for both --json and --copy; print JSON to
                    // stdout so scripts still see it.
                    println!("{output}");
                }
            }
            Err(err) => {
                eprintln!("(no clipboard tool available: {err}; printing to stdout)");
                println!("{output}");
            }
        }
    } else {
        println!("{output}");
    }
    Ok(())
}

fn build_report() -> DiagReport {
    let env = Environment::detect();
    let cache_path = portal_token_cache_path();
    let portal_token_cached = cache_path.as_ref().is_some_and(|p| Path::new(p).exists());

    let backends = vec![
        probe_libei(&env),
        probe_wlroots(&env),
        probe_kde(&env),
        probe_gnome(&env),
        probe_uinput(),
    ];

    let suggested_fixes = backends
        .iter()
        .filter_map(|b| b.fix_hint.as_ref().map(|h| format!("[{}] {}", b.name, h)))
        .collect();

    DiagReport {
        schema_version: SCHEMA_VERSION,
        wdotool_version: env!("CARGO_PKG_VERSION"),
        environment: EnvSection {
            desktop: env.desktop.clone(),
            session_type: env.session_type.clone(),
            wayland_display: env.wayland_display.clone(),
            display: std::env::var("DISPLAY").ok(),
            compositor_hints: env.compositor_hints.clone(),
            portal_token_cache_path: cache_path.map(|p| p.display().to_string()),
            portal_token_cached,
        },
        backends,
        suggested_fixes,
    }
}

// ---- backend probes --------------------------------------------------

fn probe_libei(env: &Environment) -> BackendSection {
    match check_portal_remote_desktop() {
        PortalCheck::Found => BackendSection {
            name: "libei",
            status: BackendStatus::Available,
            detail: "portal RemoteDesktop interface is exposed".into(),
            fix_hint: None,
        },
        PortalCheck::PortalMissingRemoteDesktop => BackendSection {
            name: "libei",
            status: BackendStatus::Unavailable,
            detail: "xdg-desktop-portal is running but does not expose org.freedesktop.portal.RemoteDesktop".into(),
            fix_hint: Some(suggest_portal_install(env)),
        },
        PortalCheck::PortalNotRunning => BackendSection {
            name: "libei",
            status: BackendStatus::Unavailable,
            detail: "xdg-desktop-portal is not running on the session bus".into(),
            fix_hint: Some(format!(
                "install + start xdg-desktop-portal and {}",
                suggest_portal_install(env)
            )),
        },
        PortalCheck::Error(msg) => BackendSection {
            name: "libei",
            status: BackendStatus::Warning,
            detail: format!("could not probe the portal: {msg}"),
            fix_hint: Some(
                "install systemd's `busctl` or set $DBUS_SESSION_BUS_ADDRESS so the probe can reach the session bus".into(),
            ),
        },
    }
}

fn probe_wlroots(env: &Environment) -> BackendSection {
    // Probing the actual wl_registry globals would need wayland-client,
    // which lives behind wdotool-core's wlroots feature and is not part
    // of the CLI's public API surface. Inferring from compositor hints
    // is good enough for diag; the wlroots backend's own bootstrap
    // surfaces a precise error if the protocols aren't there.
    let is_wlroots = env.has_hint("sway")
        || env.has_hint("hyprland")
        || env.has_hint("wayfire")
        || env.desktop_is("sway")
        || env.desktop_is("Hyprland");
    if is_wlroots {
        BackendSection {
            name: "wlroots",
            status: BackendStatus::Available,
            detail: format!(
                "wlroots-flavored compositor detected: {:?}",
                env.compositor_hints
            ),
            fix_hint: None,
        }
    } else {
        BackendSection {
            name: "wlroots",
            status: BackendStatus::Unavailable,
            detail: "no Sway / Hyprland / river / Wayfire markers in the environment".into(),
            fix_hint: None,
        }
    }
}

fn probe_kde(env: &Environment) -> BackendSection {
    if env.desktop_is("KDE") {
        BackendSection {
            name: "kde",
            status: BackendStatus::Available,
            detail: "XDG_CURRENT_DESKTOP=KDE; window mgmt via KWin scripting D-Bus".into(),
            fix_hint: None,
        }
    } else {
        BackendSection {
            name: "kde",
            status: BackendStatus::Unavailable,
            detail: format!("XDG_CURRENT_DESKTOP={}", display_opt(&env.desktop)),
            fix_hint: None,
        }
    }
}

fn probe_gnome(env: &Environment) -> BackendSection {
    let extension_path = home_dir().map(|h| {
        h.join(".local/share/gnome-shell/extensions/wdotool@wdotool.github.io/metadata.json")
    });
    probe_gnome_with(env, extension_path.as_deref())
}

fn probe_gnome_with(env: &Environment, extension_metadata: Option<&Path>) -> BackendSection {
    let extension_installed = extension_metadata.map(|p| p.exists()).unwrap_or(false);

    match (env.desktop_is("GNOME"), extension_installed) {
        (true, true) => BackendSection {
            name: "gnome",
            status: BackendStatus::Available,
            detail: "GNOME detected; wdotool@wdotool.github.io extension installed".into(),
            fix_hint: None,
        },
        (true, false) => BackendSection {
            name: "gnome",
            status: BackendStatus::Warning,
            detail: "GNOME detected but the wdotool shell extension is not installed; window management will fall back to bare libei (input only)".into(),
            fix_hint: Some(
                "cp -r packaging/gnome-extension/wdotool@wdotool.github.io ~/.local/share/gnome-shell/extensions/ && log out + back in && gnome-extensions enable wdotool@wdotool.github.io".into(),
            ),
        },
        (false, _) => BackendSection {
            name: "gnome",
            status: BackendStatus::Unavailable,
            detail: format!("XDG_CURRENT_DESKTOP={}", display_opt(&env.desktop)),
            fix_hint: None,
        },
    }
}

fn probe_uinput() -> BackendSection {
    let path = Path::new("/dev/uinput");
    if !path.exists() {
        return BackendSection {
            name: "uinput",
            status: BackendStatus::Unavailable,
            detail: "/dev/uinput does not exist".into(),
            fix_hint: Some("load the uinput kernel module: sudo modprobe uinput (or rebuild your kernel with CONFIG_INPUT_UINPUT=m)".into()),
        };
    }
    match fs::OpenOptions::new().write(true).open(path) {
        Ok(_) => BackendSection {
            name: "uinput",
            status: BackendStatus::Available,
            detail: "/dev/uinput is writable by the current user".into(),
            fix_hint: None,
        },
        Err(_) => BackendSection {
            name: "uinput",
            status: BackendStatus::Warning,
            detail: "/dev/uinput exists but the current user cannot open it for writing".into(),
            fix_hint: Some(
                "sudo usermod -aG uinput $USER (or `input` on some distros), then log out + back in. A udev rule may also be needed: `KERNEL==\"uinput\", GROUP=\"uinput\", MODE=\"0660\"`".into(),
            ),
        },
    }
}

// ---- helpers ---------------------------------------------------------

#[derive(Debug)]
enum PortalCheck {
    Found,
    PortalMissingRemoteDesktop,
    PortalNotRunning,
    Error(String),
}

fn check_portal_remote_desktop() -> PortalCheck {
    let output = match Command::new("busctl")
        .args([
            "--user",
            "introspect",
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
        ])
        .output()
    {
        Ok(o) => o,
        Err(e) => return PortalCheck::Error(format!("could not run busctl: {e}")),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Could not activate") || stderr.contains("not provided by any service") {
            return PortalCheck::PortalNotRunning;
        }
        return PortalCheck::Error(format!(
            "busctl exited with status {} (stderr: {})",
            output.status,
            stderr.trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains("org.freedesktop.portal.RemoteDesktop") {
        PortalCheck::Found
    } else {
        PortalCheck::PortalMissingRemoteDesktop
    }
}

fn suggest_portal_install(env: &Environment) -> String {
    if env.desktop_is("GNOME") {
        "install xdg-desktop-portal-gnome".into()
    } else if env.desktop_is("KDE") {
        "install xdg-desktop-portal-kde".into()
    } else if env.has_hint("hyprland") {
        "xdg-desktop-portal-hyprland 1.3.x does not expose RemoteDesktop yet; pass --backend wlroots to use the virtual-keyboard / virtual-pointer protocols directly".into()
    } else {
        "install an xdg-desktop-portal backend that exposes RemoteDesktop (xdg-desktop-portal-gnome or xdg-desktop-portal-kde)".into()
    }
}

fn portal_token_cache_path() -> Option<PathBuf> {
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        let p = PathBuf::from(state);
        if p.is_absolute() {
            return Some(p.join("wdotool").join("portal.token"));
        }
    }
    home_dir().map(|h| h.join(".local/state/wdotool/portal.token"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn display_opt(opt: &Option<String>) -> String {
    opt.clone().unwrap_or_else(|| "(unset)".into())
}

fn try_clipboard(content: &str) -> std::result::Result<&'static str, String> {
    use std::io::Write;
    use std::process::Stdio;

    for tool in ["wl-copy", "xclip"] {
        let mut cmd = Command::new(tool);
        if tool == "xclip" {
            cmd.args(["-selection", "clipboard"]);
        }
        let child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(_) => continue,
        };
        if let Some(stdin) = child.stdin.as_mut() {
            if stdin.write_all(content.as_bytes()).is_err() {
                let _ = child.kill();
                continue;
            }
        }
        match child.wait() {
            Ok(status) if status.success() => return Ok(tool),
            _ => continue,
        }
    }
    Err("neither wl-copy nor xclip is on $PATH".into())
}

// ---- markdown rendering ---------------------------------------------

fn render_markdown(r: &DiagReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# wdotool diag (wdotool {})\n\n",
        r.wdotool_version
    ));

    // Environment
    out.push_str("## Environment\n");
    out.push_str(&format!(
        "- Desktop: {}\n- Session: {}\n- Wayland display: {}\n- X11 display: {}\n- Compositor hints: {}\n",
        display_opt(&r.environment.desktop),
        display_opt(&r.environment.session_type),
        display_opt(&r.environment.wayland_display),
        display_opt(&r.environment.display),
        if r.environment.compositor_hints.is_empty() {
            "(none)".to_string()
        } else {
            r.environment.compositor_hints.join(", ")
        },
    ));
    if let Some(p) = &r.environment.portal_token_cache_path {
        let mark = if r.environment.portal_token_cached {
            "cached"
        } else {
            "not yet cached"
        };
        out.push_str(&format!("- Portal token cache: `{p}` ({mark})\n"));
    }
    out.push('\n');

    // Backends
    out.push_str("## Backends\n");
    for b in &r.backends {
        let mark = match b.status {
            BackendStatus::Available => "available",
            BackendStatus::Unavailable => "unavailable",
            BackendStatus::Warning => "warning",
        };
        out.push_str(&format!("- **{}** ({mark}): {}\n", b.name, b.detail));
    }
    out.push('\n');

    // Suggested fixes
    if !r.suggested_fixes.is_empty() {
        out.push_str("## Suggested fixes\n");
        for fix in &r.suggested_fixes {
            out.push_str(&format!("- {fix}\n"));
        }
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_env() -> Environment {
        Environment {
            desktop: None,
            session_type: None,
            wayland_display: None,
            compositor_hints: Vec::new(),
        }
    }

    #[test]
    fn probe_wlroots_says_available_on_hyprland() {
        let env = Environment {
            desktop: Some("Hyprland".into()),
            compositor_hints: vec!["hyprland"],
            ..empty_env()
        };
        let report = probe_wlroots(&env);
        assert_eq!(report.status, BackendStatus::Available);
    }

    #[test]
    fn probe_wlroots_says_unavailable_on_gnome() {
        let env = Environment {
            desktop: Some("GNOME".into()),
            ..empty_env()
        };
        let report = probe_wlroots(&env);
        assert_eq!(report.status, BackendStatus::Unavailable);
    }

    #[test]
    fn probe_kde_keys_off_xdg_current_desktop() {
        let env = Environment {
            desktop: Some("KDE".into()),
            ..empty_env()
        };
        assert_eq!(probe_kde(&env).status, BackendStatus::Available);

        let env = Environment {
            desktop: Some("GNOME".into()),
            ..empty_env()
        };
        assert_eq!(probe_kde(&env).status, BackendStatus::Unavailable);
    }

    #[test]
    fn probe_gnome_warns_when_extension_missing_but_desktop_is_gnome() {
        let env = Environment {
            desktop: Some("GNOME".into()),
            ..empty_env()
        };
        let nonexistent = Path::new("/nonexistent/wdotool-test/metadata.json");
        let report = probe_gnome_with(&env, Some(nonexistent));
        assert_eq!(report.status, BackendStatus::Warning);
        assert!(report.fix_hint.is_some());
    }

    #[test]
    fn probe_gnome_unavailable_when_desktop_is_not_gnome() {
        let env = Environment {
            desktop: Some("KDE".into()),
            ..empty_env()
        };
        // Extension presence is irrelevant when desktop != GNOME.
        let report = probe_gnome_with(&env, None);
        assert_eq!(report.status, BackendStatus::Unavailable);
    }

    #[test]
    fn render_markdown_includes_all_sections() {
        let report = DiagReport {
            schema_version: 1,
            wdotool_version: "0.1.6",
            environment: EnvSection {
                desktop: Some("GNOME".into()),
                session_type: Some("wayland".into()),
                wayland_display: Some("wayland-0".into()),
                display: None,
                compositor_hints: vec!["gnome-shell"],
                portal_token_cache_path: Some("/home/u/.local/state/wdotool/portal.token".into()),
                portal_token_cached: true,
            },
            backends: vec![BackendSection {
                name: "libei",
                status: BackendStatus::Available,
                detail: "portal ok".into(),
                fix_hint: None,
            }],
            suggested_fixes: vec!["[uinput] fix me".into()],
        };
        let md = render_markdown(&report);
        assert!(md.starts_with("# wdotool diag"));
        assert!(md.contains("## Environment"));
        assert!(md.contains("## Backends"));
        assert!(md.contains("## Suggested fixes"));
        assert!(md.contains("portal.token"));
        assert!(md.contains("[uinput] fix me"));
    }

    #[test]
    fn json_render_round_trips_through_serde() {
        let report = DiagReport {
            schema_version: 1,
            wdotool_version: "0.1.6",
            environment: EnvSection {
                desktop: None,
                session_type: None,
                wayland_display: None,
                display: None,
                compositor_hints: vec![],
                portal_token_cache_path: None,
                portal_token_cached: false,
            },
            backends: vec![],
            suggested_fixes: vec![],
        };
        let json = serde_json::to_string(&report).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["environment"]["portal_token_cached"], false);
    }
}
