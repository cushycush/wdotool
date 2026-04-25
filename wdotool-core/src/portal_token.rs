//! Portal restore_token cache for the libei backend.
//!
//! The XDG RemoteDesktop portal can issue a `restore_token` after the
//! user grants consent. Future sessions that present the same token
//! skip the consent dialog (the portal validates the token against the
//! user's stored grant). Without this cache, every `wdotool` invocation
//! on GNOME / KDE / any xdg-portal-supporting compositor pops a fresh
//! consent dialog, which makes a 20-step wflow workflow impossible to
//! actually run.
//!
//! The cache is best-effort: load failures (missing file, malformed
//! JSON, unknown schema) all degrade gracefully to "no cached token,
//! run first-time consent flow." Only OS-level errors propagate.
//!
//! On-disk format: `$XDG_STATE_HOME/wdotool/portal.token`, mode 0600,
//! containing JSON with a `schema_version` field. Bumps to the schema
//! version invalidate older caches (load() treats them as "no cached
//! token" and the next session re-issues one).

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::warn;

/// The current cache schema. Bump on incompatible field changes;
/// `load()` treats unknown versions as "no cached token."
const SCHEMA_VERSION: u32 = 1;

/// Cached portal restore_token + metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedToken {
    pub schema_version: u32,
    pub token: String,
    /// Best-effort identifier of the portal backend that issued the
    /// token (`"gnome"` / `"kde"` / `"other"`). Diagnostic only — the
    /// retry-without-token recovery flow handles backend switches.
    pub portal_backend: String,
    /// `env!("CARGO_PKG_VERSION")` at the time of the save.
    pub wdotool_version: String,
    pub created_at_unix: u64,
}

/// Resolve the cache file path: `$XDG_STATE_HOME/wdotool/portal.token`,
/// falling back to `$HOME/.local/state/wdotool/portal.token`. Returns
/// `None` only when neither env var resolves (shouldn't happen in any
/// normal session, but we don't want to panic on malformed environments).
pub(crate) fn cache_path() -> Option<PathBuf> {
    state_dir().map(|d| cache_path_in(&d))
}

fn state_dir() -> Option<PathBuf> {
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        let path = PathBuf::from(state);
        if path.is_absolute() {
            return Some(path);
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state"))
}

fn cache_path_in(state_dir: &Path) -> PathBuf {
    state_dir.join("wdotool").join("portal.token")
}

/// Load the cached token, if any.
///
/// Returns `Ok(None)` for: missing file, malformed JSON, unknown
/// `schema_version`. Returns `Err(_)` only on unexpected I/O errors
/// (permission denied, EIO, etc.).
pub(crate) fn load() -> io::Result<Option<CachedToken>> {
    let Some(path) = cache_path() else {
        return Ok(None);
    };
    load_from(&path)
}

fn load_from(path: &Path) -> io::Result<Option<CachedToken>> {
    let contents = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    match serde_json::from_str::<CachedToken>(&contents) {
        Ok(t) if t.schema_version == SCHEMA_VERSION => Ok(Some(t)),
        Ok(t) => {
            warn!(
                schema_version = t.schema_version,
                expected = SCHEMA_VERSION,
                "ignoring portal token cache: unknown schema_version"
            );
            Ok(None)
        }
        Err(e) => {
            warn!(
                error = %e,
                path = %path.display(),
                "ignoring portal token cache: malformed JSON"
            );
            Ok(None)
        }
    }
}

/// Save a freshly-issued token to the cache atomically.
///
/// - Creates the parent directory with mode 0700 if missing.
/// - Writes to a pid-suffixed tmp file with mode 0600 set at create
///   time via `OpenOptions::mode(0o600)`. Setting the mode at create
///   (rather than chmod-after-write) closes the umask race window
///   where another local process could read the token before the
///   chmod lands.
/// - `rename(2)` to the final path. Atomic on the same filesystem.
pub(crate) fn save(token: &str, portal_backend: &str) -> io::Result<()> {
    let Some(path) = cache_path() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no XDG_STATE_HOME or HOME — cannot persist portal token",
        ));
    };
    save_to(&path, token, portal_backend)
}

fn save_to(path: &Path, token: &str, portal_backend: &str) -> io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        match fs::metadata(parent) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "cache parent path exists but is not a directory",
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)?;
            }
            Err(e) => return Err(e),
        }
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let cached = CachedToken {
        schema_version: SCHEMA_VERSION,
        token: token.to_owned(),
        portal_backend: portal_backend.to_owned(),
        wdotool_version: env!("CARGO_PKG_VERSION").to_owned(),
        created_at_unix: now,
    };
    let payload = serde_json::to_vec_pretty(&cached)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    // Pid-suffixed tmp name so concurrent writers from different
    // processes don't trample each other's tmp file. create_new flag
    // means open fails (instead of clobbering) if the tmp already
    // exists.
    let tmp_path = path.with_file_name(format!("portal.token.tmp.{}", std::process::id()));

    let write_result: io::Result<()> = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp_path)?;
        f.write_all(&payload)?;
        // Best-effort fsync — durability isn't load-bearing for a
        // regenerable token cache, but lowers the chance of an empty
        // file surviving a power loss between write and rename.
        let _ = f.sync_all();
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    // rename(2) is atomic on the same filesystem; the receiving open()
    // either sees the old file or the new file, never a half-written one.
    if let Err(e) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    fn cache_in(dir: &Path) -> PathBuf {
        cache_path_in(dir)
    }

    #[test]
    fn load_returns_none_for_missing_file() {
        let dir = tempdir().unwrap();
        let path = cache_in(dir.path());
        assert!(load_from(&path).unwrap().is_none());
    }

    #[test]
    fn load_returns_none_for_malformed_json() {
        let dir = tempdir().unwrap();
        let path = cache_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ this is not json").unwrap();
        assert!(load_from(&path).unwrap().is_none());
    }

    #[test]
    fn load_returns_none_for_unknown_schema_version() {
        let dir = tempdir().unwrap();
        let path = cache_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let payload = r#"{
            "schema_version": 99,
            "token": "abc",
            "portal_backend": "gnome",
            "wdotool_version": "0.99.0",
            "created_at_unix": 1
        }"#;
        fs::write(&path, payload).unwrap();
        assert!(load_from(&path).unwrap().is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempdir().unwrap();
        let path = cache_in(dir.path());
        save_to(&path, "secret-token", "gnome").unwrap();
        let loaded = load_from(&path).unwrap().expect("token must load back");
        assert_eq!(loaded.schema_version, SCHEMA_VERSION);
        assert_eq!(loaded.token, "secret-token");
        assert_eq!(loaded.portal_backend, "gnome");
        assert_eq!(loaded.wdotool_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn save_creates_parent_dir_with_mode_0700() {
        let dir = tempdir().unwrap();
        let path = cache_in(dir.path());
        let parent = path.parent().unwrap();
        assert!(!parent.exists());
        save_to(&path, "tok", "kde").unwrap();
        let meta = fs::metadata(parent).unwrap();
        assert!(meta.is_dir());
        // Mask off the file-type bits and check that no group/world
        // permissions made it through.
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "parent dir mode = {mode:o}, want 0700");
    }

    #[test]
    fn save_writes_file_with_mode_0600() {
        let dir = tempdir().unwrap();
        let path = cache_in(dir.path());
        save_to(&path, "tok", "gnome").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "cache file mode = {mode:o}, want 0600");
    }

    #[test]
    fn save_overwrites_existing_cache_atomically() {
        let dir = tempdir().unwrap();
        let path = cache_in(dir.path());
        save_to(&path, "first", "gnome").unwrap();
        save_to(&path, "second", "kde").unwrap();
        let loaded = load_from(&path).unwrap().unwrap();
        assert_eq!(loaded.token, "second");
        assert_eq!(loaded.portal_backend, "kde");
        // No leftover .tmp files in the directory.
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "leftover tmp files: {leftovers:?}");
    }

    #[test]
    fn save_propagates_eacces_when_parent_unwritable() {
        let dir = tempdir().unwrap();
        // Point the cache at a deep path under a read-only directory:
        // the parent-dir creation hits EACCES.
        let ro = dir.path().join("readonly");
        fs::create_dir(&ro).unwrap();
        let mut perms = fs::metadata(&ro).unwrap().permissions();
        perms.set_mode(0o500);
        fs::set_permissions(&ro, perms).unwrap();

        let path = cache_in(&ro.join("blocked"));
        let err = save_to(&path, "tok", "gnome").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);

        // Restore permissions so tempdir cleanup works.
        let mut perms = fs::metadata(&ro).unwrap().permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&ro, perms).unwrap();
    }
}
