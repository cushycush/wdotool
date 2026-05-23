//! Synthetic-input provenance: the stable identifier wdotool sends to
//! compositors and portals so they can attribute the events it injects.
//!
//! The reverse-domain identifier matches the Flatpak app id at
//! `packaging/flatpak/io.github.cushycush.wdotool.yml` and the metainfo
//! file alongside it, so a compositor that already knows about wdotool
//! from one surface (Flatpak install metadata, desktop entry) sees the
//! same string from libei.
//!
//! Sender vs receiver role is already conveyed by the libei
//! `ei_handshake.ContextType`, so the name string doesn't need to
//! encode it.
//!
//! ## Reading provenance on the compositor side
//!
//! `APP_ID` is the textual half of the contract. The PID half lives in
//! the Unix socket peer credentials: a compositor that wants to
//! attribute synthetic input to a specific wdotool invocation reads
//! `SO_PEERCRED` on its end of the EIS socket, then cross-references
//! the resulting PID against `/proc/<pid>/exe` and
//! `/proc/<pid>/cmdline` if it wants the binary path and the exact
//! subcommand. wdotool deliberately does not embed the PID in the
//! handshake name, since SCM_CREDENTIALS is the more trustworthy
//! source (the kernel sets it, not the client).

/// Reverse-domain identifier sent to libei (and to the XDG
/// RemoteDesktop portal) so the compositor can attribute synthetic
/// input back to wdotool. Stable across releases; never include
/// version or PID inline since both have better-trusted sources
/// elsewhere.
pub const APP_ID: &str = "io.github.cushycush.wdotool";

#[cfg(test)]
mod tests {
    use super::APP_ID;

    #[test]
    fn app_id_is_reverse_domain() {
        // Three components, no slashes, no whitespace. Catches a
        // future edit that accidentally appends a version or role
        // suffix; per the docs that information belongs in
        // SO_PEERCRED / /proc, not in the handshake name.
        assert_eq!(APP_ID, "io.github.cushycush.wdotool");
        assert!(!APP_ID.contains('/'));
        assert!(!APP_ID.contains(' '));
        assert!(APP_ID.split('.').count() >= 3);
    }

    #[test]
    fn app_id_matches_flatpak_id() {
        // The Flatpak manifest at packaging/flatpak/<id>.yml is the
        // canonical source of the reverse-domain id. If this string
        // ever drifts from that filename, downstream consumers
        // (portal allowlists, compositor app filters) start looking
        // at the wrong name. Catch the drift here.
        let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("packaging")
            .join("flatpak")
            .join(format!("{APP_ID}.yml"));
        assert!(
            manifest_path.exists(),
            "expected Flatpak manifest at {} to confirm APP_ID; if the manifest moved, update APP_ID or the test",
            manifest_path.display()
        );
    }
}
