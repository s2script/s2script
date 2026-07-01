//! Plugin directory watcher: polls for `.s2sp` archives, reads and validates them
//! in-memory, and drives `v8host::{load_plugin_js, unload_plugin}`.
//!
//! Engine-generic: no CS2 identifiers appear here.  The plugin `id` and JS source
//! come entirely from the manifest and archive; core never inspects their content.
//!
//! Degrade-never-crash: any read/parse/load error logs a named WARN and continues;
//! the broken entry is left at its OLD mtime so the next poll re-tries it.

use serde::Deserialize;
use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/// Minimal manifest parsed from `manifest.json` inside a `.s2sp` archive.
/// Unknown extra fields are ignored (forward-compatible via serde's default).
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub id: String,
    /// Carried in the manifest contract; not yet validated (semver enforcement deferred).
    #[allow(dead_code)]
    pub version: String,
    #[serde(rename = "apiVersion")]
    pub api_version: String,
}

/// The major apiVersion this host speaks.  A plugin whose declared apiVersion major differs is
/// refused at load (degrade-never-crash: WARN + skip) — spec §5.  Bumping the host's breaking
/// contract bumps this constant.
pub(crate) const HOST_API_VERSION_MAJOR: u32 = 1;

/// Parse the leading integer (semver major) from a plugin's declared apiVersion string.
/// Tolerates a leading range operator: "1.x", "1.0.0", "^1.2.3", "~1.0" all → Some(1).
/// Returns None when there is no leading integer ("x", "").
fn parse_api_major(api_version: &str) -> Option<u32> {
    let after_op = api_version.trim_start_matches(|c: char| !c.is_ascii_digit());
    let digits: String = after_op.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u32>().ok()
}

/// True if a plugin declaring `api_version` is compatible with this host (same major) — spec §5.
fn api_version_compatible(api_version: &str) -> bool {
    matches!(parse_api_major(api_version), Some(m) if m == HOST_API_VERSION_MAJOR)
}

// ---------------------------------------------------------------------------
// read_s2sp
// ---------------------------------------------------------------------------

/// Unzip a `.s2sp` archive from raw bytes and extract `(Manifest, plugin_js)`.
///
/// Returns `Err(named_reason)` when:
/// - `bytes` is not a valid zip archive
/// - `manifest.json` is absent or fails JSON parsing into `Manifest`
/// - `plugin.js` is absent or contains invalid UTF-8
pub fn read_s2sp(bytes: &[u8]) -> Result<(Manifest, String), String> {
    use std::io::{Cursor, Read};

    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| format!("read_s2sp: not a valid zip: {}", e))?;

    // Read and parse manifest.json (borrow released when entry drops).
    let manifest: Manifest = {
        let mut entry = archive
            .by_name("manifest.json")
            .map_err(|_| "read_s2sp: missing manifest.json in archive".to_string())?;
        let mut s = String::new();
        entry
            .read_to_string(&mut s)
            .map_err(|e| format!("read_s2sp: failed to read manifest.json: {}", e))?;
        serde_json::from_str(&s)
            .map_err(|e| format!("read_s2sp: invalid manifest.json: {}", e))?
    };

    // Read plugin.js (borrow released when entry drops).
    let plugin_js: String = {
        let mut entry = archive
            .by_name("plugin.js")
            .map_err(|_| "read_s2sp: missing plugin.js in archive".to_string())?;
        let mut s = String::new();
        entry
            .read_to_string(&mut s)
            .map_err(|e| format!("read_s2sp: failed to read plugin.js: {}", e))?;
        s
    };

    Ok((manifest, plugin_js))
}

// ---------------------------------------------------------------------------
// Poll state
// ---------------------------------------------------------------------------

/// Per-file state tracked across `poll_plugins` calls.
struct WatchedPlugin {
    mtime: SystemTime,
    /// Plugin id taken from the last successfully parsed `manifest.json`.
    /// Needed for VANISHED → `unload_plugin(id)` when the file is gone.
    id: String,
}

thread_local! {
    /// The directory `poll_plugins` watches.  Set once by the shim at load time.
    static PLUGINS_DIR: std::cell::RefCell<Option<PathBuf>> =
        std::cell::RefCell::new(None);
    /// Live snapshot: `{path → (mtime, plugin_id)}` for every `.s2sp` file last
    /// successfully loaded or parsed.  Updated after each action set.
    static WATCH_STATE: std::cell::RefCell<HashMap<PathBuf, WatchedPlugin>> =
        std::cell::RefCell::new(HashMap::new());
    /// Counts how many times `poll_plugins` has been called (throttle counter).
    static DRAIN_COUNT: Cell<u64> = Cell::new(0);
}

/// Number of Post-drain calls between each real directory scan.
/// At ~64 Hz (CS2 default tick rate), `64` ≈ 1 second between scans.
const POLL_THROTTLE: u64 = 64;

/// Store the plugins directory path for `poll_plugins`.
/// Called once by the shim at load time via the `s2script_core_set_plugins_dir` C-ABI.
pub(crate) fn set_plugins_dir(path: &str) {
    PLUGINS_DIR.with(|d| *d.borrow_mut() = Some(PathBuf::from(path)));
    // The watcher runs on the GameFrame Post drain, which only fires while the detour is installed.
    // With no plugin loaded there is no subscriber, so poke the lazy-detour predicate now (it now
    // includes `is_watching()`) to install the detour and start the poll loop.
    crate::v8host::refresh_detour();
}

/// True once a plugins directory has been set — feeds the lazy-detour predicate so the Post drain
/// (and thus `poll_plugins`) runs every frame even before the first plugin subscribes anything.
pub(crate) fn is_watching() -> bool {
    PLUGINS_DIR.with(|d| d.borrow().is_some())
}

// ---------------------------------------------------------------------------
// poll_plugins
// ---------------------------------------------------------------------------

/// Called from the Post-drain path (throttled to `POLL_THROTTLE` calls apart).
///
/// Diffs the current `.s2sp` listing against the last snapshot and drives
/// `v8host::load_plugin_js` / `v8host::unload_plugin`:
/// - NEW file    → `load_plugin_js(id, js)`
/// - CHANGED mtime → `unload_plugin(old_id)` then `load_plugin_js(new_id, js)`
/// - VANISHED file → `unload_plugin(id)`
///
/// Degrade-never-crash: any step that fails logs a named WARN and the loop continues.
pub(crate) fn poll_plugins() {
    // Throttle: only act once every POLL_THROTTLE calls.
    let count = DRAIN_COUNT.with(|c| {
        let v = c.get();
        c.set(v.wrapping_add(1));
        v
    });
    if count % POLL_THROTTLE != 0 {
        return;
    }

    // Get the configured directory (cheap clone of Option<PathBuf>).
    let dir = PLUGINS_DIR.with(|d| d.borrow().clone());
    let Some(dir) = dir else { return };

    // Snapshot the current directory (gracefully empty if the dir doesn't exist yet).
    let current = collect_s2sp_mtimes(&dir);

    // Compute the action list while briefly borrowing WATCH_STATE; release before any V8 call.
    let actions = compute_actions(&current);

    // Execute actions; collect state mutations (no WATCH_STATE borrow held here).
    let mut inserts: Vec<(PathBuf, WatchedPlugin)> = Vec::new();
    let mut removes: Vec<PathBuf> = Vec::new();

    for action in actions {
        match action {
            Action::Load { path, mtime } => match read_file_and_parse(&path) {
                Ok((manifest, js)) => {
                    if !api_version_compatible(&manifest.api_version) {
                        crate::v8host::log_warn(&format!(
                            "WARN: poll_plugins: refusing {:?}: apiVersion {:?} incompatible with host major {}",
                            path, manifest.api_version, HOST_API_VERSION_MAJOR
                        ));
                        continue;
                    }
                    crate::v8host::load_plugin_js(&manifest.id, &js);
                    inserts.push((path, WatchedPlugin { mtime, id: manifest.id }));
                }
                Err(e) => {
                    crate::v8host::log_warn(&format!(
                        "WARN: poll_plugins: failed to load {:?}: {}",
                        path, e
                    ));
                }
            },

            Action::Reload { path, mtime, old_id } => match read_file_and_parse(&path) {
                Ok((manifest, js)) => {
                    if !api_version_compatible(&manifest.api_version) {
                        crate::v8host::log_warn(&format!(
                            "WARN: poll_plugins: refusing reload of {:?}: apiVersion {:?} incompatible with host major {}",
                            path, manifest.api_version, HOST_API_VERSION_MAJOR
                        ));
                        continue;
                    }
                    // RELOAD discipline (T7): explicit unload of the old id BEFORE load.
                    // `load_plugin_js` also carries a defensive guard, but we unload here
                    // explicitly so the intent is clear and the ledger is the authority.
                    crate::v8host::unload_plugin(&old_id);
                    crate::v8host::load_plugin_js(&manifest.id, &js);
                    inserts.push((path, WatchedPlugin { mtime, id: manifest.id }));
                }
                Err(e) => {
                    crate::v8host::log_warn(&format!(
                        "WARN: poll_plugins: failed to reload {:?}: {}",
                        path, e
                    ));
                    // Leave the old entry in WATCH_STATE (old mtime) so the next poll
                    // detects this as "changed" and retries once the file is valid.
                }
            },

            Action::Unload { path, id } => {
                crate::v8host::unload_plugin(&id);
                removes.push(path);
            }
        }
    }

    // Apply state mutations (re-borrow WATCH_STATE now that all V8 calls are done).
    WATCH_STATE.with(|ws| {
        let mut state = ws.borrow_mut();
        for (path, wp) in inserts {
            state.insert(path, wp);
        }
        for path in removes {
            state.remove(&path);
        }
    });
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

enum Action {
    Load { path: PathBuf, mtime: SystemTime },
    Reload { path: PathBuf, mtime: SystemTime, old_id: String },
    Unload { path: PathBuf, id: String },
}

/// Collect `path → mtime` for every `*.s2sp` file in `dir`.
/// Returns an empty map (not an error) if the directory does not yet exist.
fn collect_s2sp_mtimes(dir: &Path) -> HashMap<PathBuf, SystemTime> {
    let mut map = HashMap::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return map,
    };
    for entry_res in rd {
        let entry = match entry_res {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("s2sp") {
            continue;
        }
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        map.insert(path, mtime);
    }
    map
}

/// Diff `current` against WATCH_STATE to produce the list of actions.
/// Borrows WATCH_STATE briefly; must not call any V8 function while the borrow is held.
fn compute_actions(current: &HashMap<PathBuf, SystemTime>) -> Vec<Action> {
    WATCH_STATE.with(|ws| {
        let state = ws.borrow();
        let mut actions = Vec::new();

        // New or changed files.
        for (path, &mtime) in current {
            match state.get(path) {
                None => actions.push(Action::Load { path: path.clone(), mtime }),
                Some(wp) if wp.mtime != mtime => actions.push(Action::Reload {
                    path: path.clone(),
                    mtime,
                    old_id: wp.id.clone(),
                }),
                _ => {} // unchanged
            }
        }

        // Vanished files.
        for (path, wp) in state.iter() {
            if !current.contains_key(path) {
                actions.push(Action::Unload { path: path.clone(), id: wp.id.clone() });
            }
        }

        actions
    })
}

/// Read a `.s2sp` file from disk then parse it via `read_s2sp`.
fn read_file_and_parse(path: &Path) -> Result<(Manifest, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read failed: {}", e))?;
    read_s2sp(&bytes)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // -------------------------------------------------------------------
    // In-memory test-zip helpers
    // -------------------------------------------------------------------

    /// Build an in-memory `.s2sp` zip containing `manifest.json` + `plugin.js`.
    fn make_test_s2sp(manifest_json: &str, plugin_js: &str) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let opts = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        writer.start_file("manifest.json", opts).expect("start manifest.json");
        writer.write_all(manifest_json.as_bytes()).expect("write manifest.json");

        let opts = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("plugin.js", opts).expect("start plugin.js");
        writer.write_all(plugin_js.as_bytes()).expect("write plugin.js");

        writer.finish().expect("finish zip").into_inner()
    }

    /// Build an in-memory `.s2sp` zip containing ONLY `plugin.js` (no manifest.json).
    fn make_test_s2sp_missing_manifest(plugin_js: &str) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let opts = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        writer.start_file("plugin.js", opts).expect("start plugin.js");
        writer.write_all(plugin_js.as_bytes()).expect("write plugin.js");

        writer.finish().expect("finish zip").into_inner()
    }

    // -------------------------------------------------------------------
    // Tests required by the T7 brief
    // -------------------------------------------------------------------

    /// A valid `.s2sp` (manifest.json + plugin.js) is extracted correctly.
    #[test]
    fn read_s2sp_extracts_manifest_and_plugin_js() {
        // Build an in-memory .s2sp: zip { manifest.json, plugin.js }.
        let bytes = make_test_s2sp(
            r#"{"id":"@demo/hello","version":"0.1.0","apiVersion":"1.x"}"#,
            "module.exports.onLoad=()=>{};",
        );
        let (m, js) = read_s2sp(&bytes).expect("valid s2sp");
        assert_eq!(m.id, "@demo/hello");
        assert!(js.contains("onLoad"));
    }

    /// A `.s2sp` without `manifest.json` is rejected with an error mentioning "manifest".
    #[test]
    fn read_s2sp_rejects_missing_manifest_named() {
        let bytes = make_test_s2sp_missing_manifest("module.exports={};");
        let err = read_s2sp(&bytes)
            .expect_err("a .s2sp without manifest.json is rejected with a reason");
        assert!(
            err.to_lowercase().contains("manifest"),
            "error must mention 'manifest', got: {}",
            err
        );
    }

    #[test]
    fn api_version_compatible_accepts_matching_major() {
        assert!(api_version_compatible("1.x"));
        assert!(api_version_compatible("1.0.0"));
        assert!(api_version_compatible("^1.2.3"));
        assert!(api_version_compatible("~1.0"));
    }

    #[test]
    fn api_version_incompatible_rejects_wrong_or_missing_major() {
        assert!(!api_version_compatible("2.x"));
        assert!(!api_version_compatible("0.9.0"));
        assert!(!api_version_compatible("x"));
        assert!(!api_version_compatible(""));
    }
}
