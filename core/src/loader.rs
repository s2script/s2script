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

/// One derived `publishes` entry: the contract's resolved version + the sha256 of the
/// exact `.d.ts` bytes the implementation typechecked against (design spec §4.2).
/// The interface NAME is the map key and is deliberately independent of the plugin id.
#[derive(Debug, Deserialize, Clone)]
pub struct PublishDecl {
    pub version: String,
    #[serde(rename = "typesSha256", default)]
    pub types_sha256: String,
}

/// Minimal manifest parsed from `manifest.json` inside a `.s2sp` archive.
/// Unknown extra fields are ignored (forward-compatible via serde's default).
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub id: String,
    /// Carried in the manifest contract; consumed by the crash-reporter breadcrumb's plugin table
    /// (semver enforcement itself is still deferred).
    pub version: String,
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    #[serde(rename = "pluginDependencies", default)]
    pub plugin_dependencies: std::collections::HashMap<String, String>,
    #[serde(rename = "optionalPluginDependencies", default)]
    pub optional_plugin_dependencies: std::collections::HashMap<String, String>,
    /// Interfaces this plugin implements: interface-name → {version, typesSha256}.
    /// Empty when the plugin publishes nothing. The host injects an interface's version
    /// from HERE — a plugin may never type a version string (spec §4.3).
    #[serde(default)]
    pub publishes: std::collections::HashMap<String, PublishDecl>,
    #[serde(default)]
    pub config: std::collections::HashMap<String, crate::config::ConfigEntry>,
}

/// The major apiVersion this host speaks.  A plugin whose declared apiVersion major differs is
/// refused at load (degrade-never-crash: WARN + skip) — spec §5.  Bumping the host's breaking
/// contract bumps this constant.
pub(crate) const HOST_API_VERSION_MAJOR: u32 = 2;

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

/// Start a plugin's load (L1 lifecycle v2). Records the manifest version (for the `Active`
/// breadcrumb), then STARTS the awaited-factory load. The plugin's actual transition
/// (arm-at-Active or teardown-on-Failure) and its `publishes` reconciliation now happen inside
/// `v8host::finalize_loading_plugins` — inline on the synchronous fast-path (the whole base suite)
/// or on a later `frame_async_drain` for an async factory. A `publishes` mismatch there fails the
/// load (WARN + teardown), so a typo'd `publishInterface` name never runs green — the silent drift
/// this design exists to remove.
///
/// Degrade per-descriptor: only THIS plugin is refused; the framework keeps running.
fn start_load(manifest: &Manifest, js: &str, cfg: &str) {
    crate::v8host::set_plugin_version(&manifest.id, &manifest.version);
    crate::v8host::load_plugin_js(&manifest.id, js, cfg);
}

/// True when every hard `pluginDependencies` interface of `manifest` is currently published (design
/// spec §4). An in-batch producer that went Active synchronously has already published its interface
/// by the consumer's turn (topo order), so this single check subsumes the "producer earlier in the
/// batch AND Active" gate; an async (still-Loading) producer is NOT yet published, so its consumer
/// parks in WAITING.
fn deps_satisfied(manifest: &Manifest) -> bool {
    manifest.plugin_dependencies.keys().all(|n| crate::v8host::iface_published(n))
}

/// Materialize + start a plugin's load and record it in WATCH_STATE. Shared by the poll file scan,
/// the reload path, and `start_unblocked_waiters` (the parked-then-unblocked path).
fn begin_load(manifest: &Manifest, js: &str, path: &Path, mtime: SystemTime) {
    crate::v8host::set_plugin_imports(&manifest.id, imports_from_manifest(manifest));
    crate::v8host::set_plugin_publishes(&manifest.id, manifest.publishes.clone());
    let cfg = crate::v8host::materialize_for_load(&manifest.id, &manifest.config);
    start_load(manifest, js, &cfg);
    crate::v8host::store_config_decls(&manifest.id, manifest.config.clone());
    WATCH_STATE.with(|ws| {
        ws.borrow_mut().insert(path.to_path_buf(), WatchedPlugin { mtime, id: manifest.id.clone() });
    });
}

/// Flatten a manifest's two dependency maps into the (name, range, Kind) decls core expects.
///
/// Every `pluginDependencies`/`optionalPluginDependencies` entry flows through as an interface dep.
/// Post-consolidation there is no builtin-skip: the framework modules are `@s2script/sdk/<cap>`
/// subpaths resolved by the prelude's `__s2require`, and no plugin declares them in its dependency
/// maps anymore (the manifest grammar lists only inter-plugin interfaces there). A first-party
/// plugin's PUBLISHED interface (e.g. `@s2script/zones`) is one of these interface deps. A legacy
/// `.s2sp` that still carries a builtin under its old `@s2script/<cap>` name flows through as a
/// phantom Hard dep — behaviorally benign: `call_target_inner` is lazy (Unavailable only at CALL
/// time, never at load) and `__s2require` is prelude-first, so the phantom is never called.
fn imports_from_manifest(m: &Manifest) -> Vec<(String, String, crate::interfaces::Kind)> {
    let mut out = Vec::new();
    for (name, range) in &m.plugin_dependencies {
        out.push((name.clone(), range.clone(), crate::interfaces::Kind::Hard));
    }
    for (name, range) in &m.optional_plugin_dependencies {
        out.push((name.clone(), range.clone(), crate::interfaces::Kind::Optional));
    }
    out
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
    /// Config-file watcher (Slice 5E.2): maps plugin id → last-seen file content (None = absent).
    /// Populated via `watch_config_for` (idempotent; seeds baseline on first call so the initial
    /// auto-generated file does not cause a spurious onChange fire).  Polled each `poll_plugins`
    /// cycle: when content changes, `re_materialize_config` fires the plugin's onChange handlers.
    static WATCHED_CONFIGS: std::cell::RefCell<HashMap<String, Option<String>>> =
        std::cell::RefCell::new(HashMap::new());
    /// Slice 6.12 (`sm plugins`): pending load/unload/reload requested from a command. Drained at the
    /// start of `poll_plugins` (the frame drain, OUTSIDE any command's isolate borrow) so the loader
    /// never runs re-entrantly. The natives only enqueue.
    static PENDING_OPS: std::cell::RefCell<Vec<PendingOp>> = std::cell::RefCell::new(Vec::new());
    /// Paths manually unloaded via `sm plugins unload` (path → id). `poll_plugins` must NOT auto-reload
    /// a suppressed file; `sm plugins load` un-suppresses it so the next scan loads it fresh.
    static SUPPRESSED: std::cell::RefCell<HashMap<PathBuf, String>> = std::cell::RefCell::new(HashMap::new());
    /// L1 lifecycle v2: plugins parked because a hard dependency was not yet published (id → parsed
    /// load). Re-checked every frame by `start_unblocked_waiters`: started once every hard-dep
    /// interface is published, or forced to load anyway after `LOAD_TIMEOUT_FRAMES` (resolved
    /// decision #3 — an unmet hard dep loads lazily; the proxy throws `InterfaceUnavailable` at call).
    static WAITING: std::cell::RefCell<HashMap<String, WaitingLoad>> = std::cell::RefCell::new(HashMap::new());
}

/// A parsed load parked pending its hard dependencies (L1 lifecycle v2).
struct WaitingLoad {
    path: PathBuf,
    mtime: SystemTime,
    manifest: Manifest,
    js: String,
    since_frame: u64,
}

/// A command-requested plugin lifecycle op (Slice 6.12), keyed by plugin id.
enum PendingOp { Unload(String), Reload(String), Load(String) }

/// Every known plugin: `(id, state)` where state is one of
/// `running | loading | waiting | failed | unloaded` (L1 lifecycle v2). Backs `Plugins.list()` /
/// `sm plugins list`. Sorted by id (BTreeMap collapses the WATCH_STATE / WAITING / FAILED / SUPPRESSED
/// sources; the last-wins priority below is deliberate: a suppressed file reads `unloaded` even if a
/// stale WATCH_STATE row lingers, and a parked/failed row wins over a bare `running` classification).
pub(crate) fn plugin_list() -> Vec<(String, String)> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<String, String> = BTreeMap::new();

    // WATCH_STATE rows: a live/parked/failed on-disk plugin.
    WATCH_STATE.with(|ws| {
        for wp in ws.borrow().values() {
            let id = &wp.id;
            let state = if WAITING.with(|w| w.borrow().contains_key(id)) {
                "waiting"
            } else if crate::v8host::is_failed(id) {
                "failed"
            } else if crate::v8host::is_loading(id) {
                "loading"
            } else {
                "running"
            };
            out.insert(id.clone(), state.to_string());
        }
    });
    // Parked plugins not represented in WATCH_STATE (defensive).
    WAITING.with(|w| for id in w.borrow().keys() {
        out.entry(id.clone()).or_insert_with(|| "waiting".to_string());
    });
    // Failed plugins whose WATCH_STATE row was never inserted (defensive).
    for id in crate::v8host::failed_plugin_ids() {
        out.entry(id).or_insert_with(|| "failed".to_string());
    }
    // Suppressed (manually unloaded) files win: on disk but not running.
    SUPPRESSED.with(|s| for id in s.borrow().values() {
        out.insert(id.clone(), "unloaded".to_string());
    });

    out.into_iter().collect()
}

/// Find the path of a currently-loaded plugin by id.
fn path_of_loaded(id: &str) -> Option<PathBuf> {
    WATCH_STATE.with(|ws| ws.borrow().iter().find(|(_, wp)| wp.id == id).map(|(p, _)| p.clone()))
}

/// Enqueue an unload of a currently-loaded plugin. Returns false if no such plugin is loaded.
pub(crate) fn request_unload(id: &str) -> bool {
    if path_of_loaded(id).is_none() { return false; }
    PENDING_OPS.with(|q| q.borrow_mut().push(PendingOp::Unload(id.to_string())));
    true
}
/// Enqueue a reload of a loaded plugin (or a re-load of a suppressed one). False if the id is unknown.
pub(crate) fn request_reload(id: &str) -> bool {
    let known = path_of_loaded(id).is_some()
        || SUPPRESSED.with(|s| s.borrow().values().any(|v| v == id));
    if known { PENDING_OPS.with(|q| q.borrow_mut().push(PendingOp::Reload(id.to_string()))); }
    known
}
/// L1 lifecycle v2: start any parked loads whose hard-dependency wait window has cleared. Called at
/// the tail of `v8host::finalize_loading_plugins` (a producer reaching Active may unblock consumers,
/// and every frame's drain re-checks the timeout). A parked plugin starts when every hard dep is
/// published, or is forced to load anyway once past `LOAD_TIMEOUT_FRAMES` (resolved decision #3).
///
/// Re-entrancy: each ready plugin is removed from WAITING BEFORE `begin_load`, and `begin_load`'s
/// synchronous fast-path re-enters `finalize_loading_plugins` → back here; removing first bounds the
/// recursion by the (finite, shrinking) WAITING set.
pub(crate) fn start_unblocked_waiters() {
    let frame = crate::v8host::current_frame();
    let ready: Vec<String> = WAITING.with(|w| {
        w.borrow()
            .iter()
            .filter_map(|(id, wl)| {
                let unblocked = wl.manifest.plugin_dependencies.keys().all(|n| crate::v8host::iface_published(n));
                let expired = frame.saturating_sub(wl.since_frame) > crate::v8host::LOAD_TIMEOUT_FRAMES;
                (unblocked || expired).then(|| id.clone())
            })
            .collect()
    });
    for id in ready {
        let Some(wl) = WAITING.with(|w| w.borrow_mut().remove(&id)) else { continue };
        let unblocked = wl.manifest.plugin_dependencies.keys().all(|n| crate::v8host::iface_published(n));
        if !unblocked {
            crate::v8host::log_warn(&format!(
                "WARN: '{}': hard dependency producer not Active after ~30s - loading anyway (calls will throw InterfaceUnavailable until it appears)",
                id
            ));
        }
        begin_load(&wl.manifest, &wl.js, &wl.path, wl.mtime);
    }
}

/// Enqueue a load of a suppressed (previously `sm plugins unload`ed) plugin. False if not suppressed.
pub(crate) fn request_load(id: &str) -> bool {
    let suppressed = SUPPRESSED.with(|s| s.borrow().values().any(|v| v == id));
    if suppressed { PENDING_OPS.with(|q| q.borrow_mut().push(PendingOp::Load(id.to_string()))); }
    suppressed
}

/// Drain the command-requested plugin ops (called at the top of `poll_plugins`, borrow-free).
fn drain_pending_ops() {
    let ops: Vec<PendingOp> = PENDING_OPS.with(|q| q.borrow_mut().drain(..).collect());
    for op in ops {
        match op {
            PendingOp::Unload(id) => {
                // A parked (WAITING) plugin was never started: drop it from WAITING and do NOT run the
                // (never-Active) teardown; a loaded one gets the normal unload (L1 lifecycle v2).
                let was_waiting = WAITING.with(|w| w.borrow_mut().remove(&id).is_some());
                if let Some(path) = path_of_loaded(&id) {
                    if !was_waiting { crate::v8host::unload_plugin(&id); }
                    crate::v8host::clear_pending_handoff(&id);
                    WATCH_STATE.with(|ws| { ws.borrow_mut().remove(&path); });
                    SUPPRESSED.with(|s| { s.borrow_mut().insert(path, id.clone()); });  // don't auto-reload
                    crate::v8host::log_warn(&format!("[plugins] unloaded '{}' (sm plugins unload)", id));
                } else if was_waiting {
                    crate::v8host::clear_pending_handoff(&id);
                    crate::v8host::log_warn(&format!("[plugins] unloaded parked '{}' (sm plugins unload)", id));
                }
            }
            PendingOp::Reload(id) => {
                // Reload-while-Loading: coalesce into a queued reload rather than tearing down a
                // half-loaded context (L1 lifecycle v2 §5.3); it fires when the in-flight load settles.
                if crate::v8host::is_loading(&id) {
                    crate::v8host::queue_pending_reload(&id);
                    crate::v8host::log_warn(&format!("[plugins] reload '{}' queued (still loading)", id));
                    continue;
                }
                // Un-suppress if needed, then let the next file scan re-load it fresh (mtime bump not
                // required — for a loaded plugin we do the reload inline; for a suppressed one, unsuppress).
                let path = path_of_loaded(&id).or_else(||
                    SUPPRESSED.with(|s| s.borrow().iter().find(|(_, v)| **v == id).map(|(p, _)| p.clone())));
                let Some(path) = path else { continue };
                SUPPRESSED.with(|s| { s.borrow_mut().remove(&path); });
                match read_file_and_parse(&path) {
                    Ok((manifest, js)) => {
                        crate::v8host::unload_plugin(&id);   // no-op if not currently loaded
                        crate::v8host::set_plugin_imports(&manifest.id, imports_from_manifest(&manifest));
                        crate::v8host::set_plugin_publishes(&manifest.id, manifest.publishes.clone());
                        let cfg = crate::v8host::materialize_for_load(&manifest.id, &manifest.config);
                        start_load(&manifest, &js, &cfg);
                        crate::v8host::store_config_decls(&manifest.id, manifest.config.clone());
                        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                        WATCH_STATE.with(|ws| { ws.borrow_mut().insert(path.clone(), WatchedPlugin { mtime, id: manifest.id }); });
                        crate::v8host::log_warn(&format!("[plugins] reloaded '{}' (sm plugins reload)", id));
                    }
                    Err(e) => crate::v8host::log_warn(&format!("[plugins] reload '{}' failed: {}", id, e)),
                }
            }
            PendingOp::Load(id) => {
                // Un-suppress; the next `poll_plugins` file scan sees it as new and Loads it.
                SUPPRESSED.with(|s| { s.borrow_mut().retain(|_, v| *v != id); });
                crate::v8host::log_warn(&format!("[plugins] load '{}' (sm plugins load) — will load next scan", id));
            }
        }
    }
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

/// Opt a plugin into config-file change detection (Slice 5E.2).  Idempotent: if the id is already
/// watched, this is a no-op (the baseline was seeded on the first call, so repeated calls from
/// multiple `config.onChange` registrations don't reset the baseline and cause spurious fires).
/// On the first call, seeds the last-seen content with the CURRENT file content so the initial
/// auto-generated file does NOT trigger a spurious onChange on the very next poll.
pub(crate) fn watch_config_for(id: &str) {
    WATCHED_CONFIGS.with(|wc| {
        let mut map = wc.borrow_mut();
        if map.contains_key(id) { return; }  // already watched — idempotent
        // Seed the baseline with the current file content (None if file absent / no ops).
        let content = crate::v8host::config_file_content(id);
        map.insert(id.to_string(), content);
    });
}

/// Stop watching a plugin's config file (called from `unload_plugin` teardown).
pub(crate) fn unwatch_config_for(id: &str) {
    WATCHED_CONFIGS.with(|wc| { wc.borrow_mut().remove(id); });
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

    // Slice 6.12: drain command-requested plugin ops (unload/reload/load) BEFORE the file scan, so the
    // loader runs them here (borrow-free) rather than re-entrantly inside a command handler.
    drain_pending_ops();

    // Get the configured directory (cheap clone of Option<PathBuf>).
    let dir = PLUGINS_DIR.with(|d| d.borrow().clone());
    let Some(dir) = dir else { return };

    // Snapshot the current directory (gracefully empty if the dir doesn't exist yet).
    let current = collect_s2sp_mtimes(&dir);

    // Compute the action list while briefly borrowing WATCH_STATE; release before any V8 call.
    let actions = compute_actions(&current);

    // L1 lifecycle v2: parse every Load/Reload up front, then start them in TOPOLOGICAL order so an
    // interface's producer reaches Active before its hard-dep consumers (a consumer whose producer is
    // still Loading parks in WAITING). Unloads run first (borrow-free); WATCH_STATE mutations are
    // applied inline by `begin_load`/the park path.
    let mut removes: Vec<PathBuf> = Vec::new();
    let mut parsed: HashMap<String, ParsedLoad> = HashMap::new();
    let mut order_input: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();

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
                    order_input.push((
                        manifest.id.clone(),
                        manifest.plugin_dependencies.keys().cloned().collect(),
                        manifest.publishes.keys().cloned().collect(),
                    ));
                    parsed.insert(manifest.id.clone(), ParsedLoad { path, mtime, manifest, js, old_id: None });
                }
                Err(e) => {
                    crate::v8host::log_warn(&format!("WARN: poll_plugins: failed to load {:?}: {}", path, e));
                }
            },

            Action::Reload { path, mtime, old_id } => {
                // Reload-while-Loading: coalesce into a queued reload instead of tearing down a
                // half-loaded context (L1 §5.3). Bump the tracked mtime so the poll doesn't re-fire.
                if crate::v8host::is_loading(&old_id) {
                    crate::v8host::queue_pending_reload(&old_id);
                    WATCH_STATE.with(|ws| { if let Some(wp) = ws.borrow_mut().get_mut(&path) { wp.mtime = mtime; } });
                    continue;
                }
                match read_file_and_parse(&path) {
                    Ok((manifest, js)) => {
                        if !api_version_compatible(&manifest.api_version) {
                            crate::v8host::log_warn(&format!(
                                "WARN: poll_plugins: refusing reload of {:?}: apiVersion {:?} incompatible with host major {}",
                                path, manifest.api_version, HOST_API_VERSION_MAJOR
                            ));
                            continue;
                        }
                        order_input.push((
                            manifest.id.clone(),
                            manifest.plugin_dependencies.keys().cloned().collect(),
                            manifest.publishes.keys().cloned().collect(),
                        ));
                        parsed.insert(manifest.id.clone(), ParsedLoad { path, mtime, manifest, js, old_id: Some(old_id) });
                    }
                    Err(e) => {
                        crate::v8host::log_warn(&format!("WARN: poll_plugins: failed to reload {:?}: {}", path, e));
                        // Leave the old entry in WATCH_STATE (old mtime) so the next poll detects this
                        // as "changed" and retries once the file is valid.
                    }
                }
            }

            Action::Unload { path, id } => {
                // A parked (never-started) plugin just drops from WAITING; a loaded one gets teardown.
                let was_waiting = WAITING.with(|w| w.borrow_mut().remove(&id).is_some());
                if !was_waiting { crate::v8host::unload_plugin(&id); }
                crate::v8host::clear_pending_handoff(&id);   // Slice 5E.3: a final removal discards any captured handoff
                removes.push(path);
            }
        }
    }

    // Start (or park) the parsed batch in dependency order.
    let frame = crate::v8host::current_frame();
    for id in topo_order(&order_input) {
        let Some(pl) = parsed.remove(&id) else { continue };
        // RELOAD discipline: explicit unload of the old id BEFORE (re)load, so the ledger is authority.
        if let Some(old_id) = &pl.old_id { crate::v8host::unload_plugin(old_id); }

        if deps_satisfied(&pl.manifest) {
            begin_load(&pl.manifest, &pl.js, &pl.path, pl.mtime);
        } else {
            // Park: record in WATCH_STATE (so mtime edits still retrigger) + WAITING (spec §4).
            WATCH_STATE.with(|ws| {
                ws.borrow_mut().insert(pl.path.clone(), WatchedPlugin { mtime: pl.mtime, id: pl.manifest.id.clone() });
            });
            crate::v8host::log_warn(&format!(
                "[plugins] '{}' WAITING on unpublished hard dependency (will load when its producer is Active, or after ~30s)",
                pl.manifest.id
            ));
            WAITING.with(|w| {
                w.borrow_mut().insert(pl.manifest.id.clone(), WaitingLoad {
                    path: pl.path, mtime: pl.mtime, manifest: pl.manifest, js: pl.js, since_frame: frame,
                });
            });
        }
    }

    // Apply the deferred WATCH_STATE removals (unloads/vanished).
    WATCH_STATE.with(|ws| {
        let mut state = ws.borrow_mut();
        for path in removes { state.remove(&path); }
    });

    // Poll config file changes for opted-in plugins (Slice 5E.2).
    poll_watched_configs();
}

/// A parsed `.s2sp` awaiting its turn in the topo-ordered load batch (L1 lifecycle v2).
struct ParsedLoad {
    path: PathBuf,
    mtime: SystemTime,
    manifest: Manifest,
    js: String,
    /// `Some(old_id)` when this parse came from a Reload — the old instance is unloaded first.
    old_id: Option<String>,
}

/// Check each watched plugin's config file for content changes.  When a change is detected,
/// update the stored baseline and call `re_materialize_config` to re-inject the updated values
/// and fire the plugin's `onChange` handlers.
///
/// Borrow discipline: WATCHED_CONFIGS is never held across `re_materialize_config` (which enters
/// V8 and may itself trigger `watch_config_for` → borrow WATCHED_CONFIGS again).  We collect the
/// changed ids into a Vec, release the borrow, then update + fire each one individually.
fn poll_watched_configs() {
    // Phase 1: collect (id, new_content) for every plugin whose content changed.
    // WATCHED_CONFIGS borrow is held only for this snapshot; released before any V8 call.
    let changes: Vec<(String, Option<String>)> = WATCHED_CONFIGS.with(|wc| {
        let map = wc.borrow();
        map.iter()
            .filter_map(|(id, last)| {
                // SAFETY: config_file_content must NOT re-borrow WATCHED_CONFIGS — it is called under
                // this immutable borrow.  It only touches ENGINE_OPS + the shim config_read op today.
                let cur = crate::v8host::config_file_content(id);
                if cur != *last { Some((id.clone(), cur)) } else { None }
            })
            .collect()
    });

    if changes.is_empty() { return; }

    // Phase 2: update the stored baseline and fire re_materialize for each changed plugin.
    // Each WATCHED_CONFIGS borrow is scoped to just the update; released before re_materialize.
    for (id, new_content) in &changes {
        WATCHED_CONFIGS.with(|wc| { wc.borrow_mut().insert(id.clone(), new_content.clone()); });
        crate::v8host::re_materialize_config(id);
    }
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
            // Slice 6.12: a path manually unloaded via `sm plugins unload` is suppressed — do NOT
            // auto-reload it (until `sm plugins load`/`reload` un-suppresses it).
            if SUPPRESSED.with(|s| s.borrow().contains_key(path)) { continue; }
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

/// Order a load batch so an interface's producer loads before its hard-dep consumers (design spec §4).
///
/// Input is one `(id, hard-dep interface names, published interface names)` tuple per plugin in the
/// batch. Edges run producer → consumer for every hard dep whose interface is published by another
/// plugin IN THIS SAME BATCH (a dep already satisfied by an earlier poll has no in-batch producer and
/// so imposes no edge). Kahn's algorithm with a stable lexicographic tie-break by id; on a dependency
/// cycle we WARN and fall back to lexicographic id order (degrade-never-crash — the hard-dep proxy is
/// lazy, so a mis-ordered pair still runs, throwing `InterfaceUnavailable` only at call time).
fn topo_order(batch: &[(String, Vec<String>, Vec<String>)]) -> Vec<String> {
    // iface name → the in-batch producer's id.
    let mut producer_of: HashMap<&str, &str> = HashMap::new();
    for (id, _deps, publishes) in batch {
        for iface in publishes {
            producer_of.insert(iface.as_str(), id.as_str());
        }
    }

    // Adjacency (producer → consumers) + indegree per id.
    let mut consumers_of: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut indegree: HashMap<&str, usize> = batch.iter().map(|(id, _, _)| (id.as_str(), 0usize)).collect();
    for (id, deps, _publishes) in batch {
        for dep in deps {
            if let Some(&producer) = producer_of.get(dep.as_str()) {
                if producer == id.as_str() { continue; } // a plugin depending on its own interface: no edge
                consumers_of.entry(producer).or_default().push(id.as_str());
                *indegree.entry(id.as_str()).or_default() += 1;
            }
        }
    }

    // Kahn's with a lexicographic ready-set (stable, deterministic order).
    let mut order: Vec<String> = Vec::with_capacity(batch.len());
    let mut ready: Vec<&str> = indegree.iter().filter(|(_, &d)| d == 0).map(|(&id, _)| id).collect();
    ready.sort_unstable();
    while let Some(&next) = ready.first() {
        ready.remove(0);
        order.push(next.to_string());
        if let Some(cs) = consumers_of.get(next) {
            let mut newly_ready: Vec<&str> = Vec::new();
            for &c in cs {
                if let Some(d) = indegree.get_mut(c) {
                    *d = d.saturating_sub(1);
                    if *d == 0 { newly_ready.push(c); }
                }
            }
            for c in newly_ready { ready.push(c); }
            ready.sort_unstable();
        }
    }

    if order.len() != batch.len() {
        // A cycle: append every not-yet-emitted id in lexicographic order.
        let mut remaining: Vec<&str> = batch
            .iter()
            .map(|(id, _, _)| id.as_str())
            .filter(|id| !order.iter().any(|o| o == id))
            .collect();
        remaining.sort_unstable();
        crate::v8host::log_warn(&format!(
            "WARN: plugin load batch has a hard-dependency cycle ({:?}) - loading in name order; interface calls throw InterfaceUnavailable until each producer is Active",
            remaining
        ));
        for id in remaining { order.push(id.to_string()); }
    }

    order
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
            r#"{"id":"@demo/hello","version":"0.1.0","apiVersion":"2.x"}"#,
            "module.exports.default={__s2plugin:1,factory:function(ctx){}};",
        );
        let (m, js) = read_s2sp(&bytes).expect("valid s2sp");
        assert_eq!(m.id, "@demo/hello");
        assert!(js.contains("factory"));
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
        assert!(api_version_compatible("2.x"));
        assert!(api_version_compatible("2.0.0"));
        assert!(api_version_compatible("^2.1.0"));
        assert!(api_version_compatible("~2.0"));
    }

    #[test]
    fn api_version_incompatible_rejects_wrong_or_missing_major() {
        assert!(!api_version_compatible("1.x"));
        assert!(!api_version_compatible("0.9.0"));
        assert!(!api_version_compatible("x"));
        assert!(!api_version_compatible(""));
    }

    #[test]
    fn manifest_parses_both_dependency_maps() {
        let bytes = make_test_s2sp(
            r#"{"id":"@demo/consumer","version":"0.1.0","apiVersion":"2.x",
                "pluginDependencies":{"@s2script/entity":"^1.0.0","@demo/greeter":"^1.0.0"},
                "optionalPluginDependencies":{"@demo/extra":"^1.0.0"}}"#,
            "module.exports.default={__s2plugin:1,factory:function(ctx){}};",
        );
        let (m, _js) = read_s2sp(&bytes).expect("valid s2sp");
        assert_eq!(m.plugin_dependencies.get("@demo/greeter").map(String::as_str), Some("^1.0.0"));
        assert_eq!(m.optional_plugin_dependencies.get("@demo/extra").map(String::as_str), Some("^1.0.0"));
    }

    #[test]
    fn manifest_defaults_missing_dependency_maps_to_empty() {
        let bytes = make_test_s2sp(
            r#"{"id":"@demo/x","version":"0.1.0","apiVersion":"2.x"}"#,
            "module.exports.default={__s2plugin:1,factory:function(ctx){}};",
        );
        let (m, _js) = read_s2sp(&bytes).expect("valid s2sp");
        assert!(m.plugin_dependencies.is_empty());
        assert!(m.optional_plugin_dependencies.is_empty());
    }

    #[test]
    fn legacy_manifest_with_builtins_in_plugin_deps_still_loads() {
        // A pre-consolidation .s2sp declares builtins as pluginDependencies. Post-BUILTIN_MODULES-deletion
        // these flow through as Hard imports with no producer — behaviorally benign: call_target_inner is
        // lazy (Unavailable at CALL time, never at load) and __s2require is prelude-first, so the phantom
        // is never called. The manifest must still parse and its imports flatten without panic.
        let bytes = make_test_s2sp(
            r#"{"id":"@legacy/plugin","version":"0.1.0","apiVersion":"2.x",
                "pluginDependencies":{"@s2script/entity":"^0.2.0","@s2script/math":"^0.1.0"}}"#,
            "module.exports.onLoad=()=>{};",
        );
        let (m, _js) = read_s2sp(&bytes).expect("legacy manifest parses");
        let imports = imports_from_manifest(&m);
        // Builtins are no longer skipped — they become phantom Hard deps (lazy, never called).
        assert_eq!(imports.len(), 2, "both builtin deps flow through post-deletion");
        assert!(imports.iter().all(|(_, _, k)| matches!(k, crate::interfaces::Kind::Hard)));
        assert!(imports.iter().any(|(n, _, _)| n == "@s2script/entity"));
    }

    #[test]
    fn manifest_parses_derived_publishes_block() {
        let json = r#"{
            "id":"@s2script/zones","version":"1.2.0","apiVersion":"2.x",
            "publishes":{"@s2script/zones":{"version":"1.2.0","typesSha256":"abc123"}}
        }"#;
        let m: Manifest = serde_json::from_str(json).expect("parse");
        let d = m.publishes.get("@s2script/zones").expect("entry present");
        assert_eq!(d.version, "1.2.0");
        assert_eq!(d.types_sha256, "abc123");
    }

    #[test]
    fn manifest_without_publishes_yields_an_empty_map() {
        let json = r#"{"id":"@demo/x","version":"0.1.0","apiVersion":"2.x"}"#;
        let m: Manifest = serde_json::from_str(json).expect("parse");
        assert!(m.publishes.is_empty());
    }

    #[test]
    fn load_batch_orders_producers_before_consumers() {
        // c depends (hard) on iface "@x/if" which p publishes; order must put p first regardless of name order.
        let batch = vec![
            ("c".to_string(), vec!["@x/if".to_string()], vec![]),                      // (id, hard dep ifaces, publishes)
            ("p".to_string(), vec![], vec!["@x/if".to_string()]),
        ];
        let order = topo_order(&batch);
        assert_eq!(order, vec!["p".to_string(), "c".to_string()]);
    }

    #[test]
    fn topo_cycle_falls_back_to_name_order() {
        let batch = vec![
            ("a".into(), vec!["@b/if".into()], vec!["@a/if".into()]),
            ("b".into(), vec!["@a/if".into()], vec!["@b/if".into()]),
        ];
        assert_eq!(topo_order(&batch), vec!["a".to_string(), "b".to_string()]); // + a WARN
    }

    #[test]
    fn manifest_publishes_may_name_a_different_interface_than_the_package() {
        // @edge/mce publishes @community/mapchooser — the decoupling this grammar exists for.
        let json = r#"{
            "id":"@edge/mce","version":"3.1.0","apiVersion":"2.x",
            "publishes":{"@community/mapchooser":{"version":"1.2.0","typesSha256":"deadbeef"}}
        }"#;
        let m: Manifest = serde_json::from_str(json).expect("parse");
        assert_eq!(m.publishes["@community/mapchooser"].version, "1.2.0");
        assert!(!m.publishes.contains_key("@edge/mce"));
    }
}
