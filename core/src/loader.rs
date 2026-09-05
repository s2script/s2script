//! Plugin directory watcher: submits `.s2sp` discovery, reads, and archive parsing to a
//! dedicated worker, then validates and applies owned results on the game thread.
//!
//! Engine-generic: no CS2 identifiers appear here.  The plugin `id` and JS source
//! come entirely from the manifest and archive; core never inspects their content.
//!
//! Degrade-never-crash: any read/parse/load error logs a named WARN and continues. Retryable
//! failures leave their strong path stamp uncommitted; semantic refusals commit it to warn once.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

pub use crate::loader_worker::{Manifest, PublishDecl};
use crate::loader_worker::{ConfigSnapshot, FileStamp, LoaderPolicy, LoaderWorker, PreparedPlugin, Submit, WatchDelta, WorkerResult};

// ---------------------------------------------------------------------------
// Operator permission allow-list (spec §6)
// ---------------------------------------------------------------------------

/// The operator allow-list: permission name → the EXACT plugin ids allowed to use it.
/// `None` = never loaded ⇒ default-DENY (mirrors the admin system's fail-safe posture).
/// Host-global (not thread_local) like the admin/ban caches: V8 contexts are per-plugin, the
/// authorization decision is not.
static PERMISSIONS: std::sync::RwLock<Option<HashMap<String, Vec<String>>>> =
    std::sync::RwLock::new(None);

/// Config id of the operator allow-list file (`addons/s2script/configs/permissions.json`).
const PERMISSIONS_CONFIG_ID: &str = "permissions";

/// One-shot so a malformed `permissions.json` WARNs once, not once per descriptor check.
static PERMISSIONS_WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Parse an operator allow-list: `{"engine:calls":["@me/burn"]}`. Exact-match ids, no globs (v1).
///
/// Tolerant of non-array values: the shipped template carries a `_help` string like every other
/// framework config template, and an entry whose value is not a string array is SKIPPED rather than
/// failing the whole file — a strict parse would leave the allow-list unloaded, i.e. deny everything,
/// which is exactly the invisible failure the named-reason doctrine exists to avoid.
pub fn load_permissions_from_str(s: &str) -> Result<(), String> {
    let raw: HashMap<String, serde_json::Value> =
        serde_json::from_str(s).map_err(|e| format!("permissions.json: {}", e))?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (name, value) in raw {
        let Some(arr) = value.as_array() else { continue };   // `_help` and friends
        map.insert(
            name,
            arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect(),
        );
    }
    *PERMISSIONS.write().map_err(|_| "permissions lock poisoned".to_string())? = Some(map);
    Ok(())
}

/// The periodic loader reads `configs/permissions.json` on its worker. Fail-safe: a missing or
/// malformed file leaves the allow-list unloaded (or retains the last valid snapshot), so gated
/// capabilities stay denied until a later worker snapshot parses successfully.
/// Default-DENY: unloaded or absent allow-list permits nothing.
pub fn permission_allowed(plugin_id: &str, permission: &str) -> bool {
    PERMISSIONS
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|m| m.get(permission).is_some_and(|v| v.iter().any(|p| p == plugin_id))))
        .unwrap_or(false)
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

/// B1 (north-star §5.2): fail-fast contract-drift gate. A consumer that compiled against a
/// dependency contract whose hash differs from what the producer CURRENTLY publishes is refused
/// at load — completing "fails at typecheck AND again at load". Producer-absent deps are not
/// checked here (lazy hard-dep contract); if such a producer appears later with a different
/// hash, every call throws `InterfaceTypesMismatch` (interfaces.rs backstop).
fn verify_compiled_against_batch(
    manifest: &Manifest,
    batch_hashes: &HashMap<String, String>,
) -> Result<(), String> {
    verify_compiled_against_with(manifest, batch_hashes, |iface| {
        crate::v8host::iface_published_types_sha256(iface)
    })
}

fn verify_compiled_against_with(
    manifest: &Manifest,
    batch_hashes: &HashMap<String, String>,
    live_hash: impl Fn(&str) -> Option<String>,
) -> Result<(), String> {
    let mut names: Vec<&String> = manifest.compiled_against.keys().collect();
    names.sort(); // deterministic first-error
    for iface in names {
        let built = &manifest.compiled_against[iface];
        if built.is_empty() { continue; }
        let published = batch_hashes.get(iface).cloned()
            .or_else(|| live_hash(iface));
        let Some(published) = published else { continue };
        if !published.is_empty() && published != *built {
            return Err(format!(
                "contract drift on '{}': compiled against typesSha256 {}… but the producer publishes {}… — refresh .s2script/types/{}/index.d.ts from the producer and rebuild",
                iface,
                &built[..12.min(built.len())],
                &published[..12.min(published.len())],
                iface
            ));
        }
    }
    Ok(())
}

fn resolve_contract_rejections(
    candidates: &[(PathBuf, &Manifest)],
    initially_rejected: &std::collections::HashSet<PathBuf>,
    live_hash: impl Fn(&str) -> Option<String> + Copy,
) -> HashMap<PathBuf, String> {
    let mut ordered: Vec<(PathBuf, &Manifest)> = candidates.iter().cloned().collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0));
    let mut eligible_providers: std::collections::HashSet<PathBuf> = ordered
        .iter()
        .filter(|(path, manifest)| {
            !initially_rejected.contains(path) && !manifest.publishes.is_empty()
        })
        .map(|(path, _)| path.clone())
        .collect();
    let mut accepted_providers = eligible_providers.clone();
    let mut cycle_rejections = HashMap::new();
    let mut history: Vec<std::collections::HashSet<PathBuf>> = Vec::new();

    // Only provider viability can change the hashes seen by another candidate. Stabilize that set
    // first, reconsidering providers against the retained live generation when a candidate falls
    // out. If a provider cycle oscillates instead of reaching a fixed point, conservatively retain
    // the live generations for every toggling provider.
    loop {
        history.push(accepted_providers.clone());
        let mut hashes = HashMap::new();
        for (path, manifest) in &ordered {
            if !accepted_providers.contains(path) { continue; }
            for (name, publish) in &manifest.publishes {
                hashes.insert(name.clone(), publish.types_sha256.clone());
            }
        }
        let next: std::collections::HashSet<PathBuf> = ordered
            .iter()
            .filter(|(path, manifest)| {
                eligible_providers.contains(path)
                    && verify_compiled_against_with(manifest, &hashes, live_hash).is_ok()
            })
            .map(|(path, _)| path.clone())
            .collect();
        if next == accepted_providers { break; }
        if let Some(cycle_start) = history.iter().position(|seen| *seen == next) {
            let cycle = history[cycle_start..].iter().chain(std::iter::once(&next));
            let mut union = std::collections::HashSet::new();
            let mut intersection = eligible_providers.clone();
            for state in cycle {
                union.extend(state.iter().cloned());
                intersection.retain(|path| state.contains(path));
            }
            let toggling: Vec<PathBuf> = union.difference(&intersection).cloned().collect();
            for path in toggling {
                eligible_providers.remove(&path);
                cycle_rejections.insert(path, "compiledAgainst provider cycle has no stable candidate contract set; keeping live generations".to_string());
            }
            accepted_providers = eligible_providers.clone();
            history.clear();
            continue;
        }
        accepted_providers = next;
    }

    let mut hashes = HashMap::new();
    for (path, manifest) in &ordered {
        if !accepted_providers.contains(path) { continue; }
        for (name, publish) in &manifest.publishes {
            hashes.insert(name.clone(), publish.types_sha256.clone());
        }
    }
    let mut rejected = cycle_rejections;
    for (path, manifest) in ordered {
        if initially_rejected.contains(&path) || rejected.contains_key(&path) { continue; }
        if let Err(reason) = verify_compiled_against_with(manifest, &hashes, live_hash) {
            rejected.insert(path, reason);
        }
    }
    rejected
}

fn stamp_time(stamp: FileStamp) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_nanos(stamp.modified_ns.min(u64::MAX as u128) as u64)
}

fn begin_load_prepared(prepared: &PreparedPlugin, cfg: &str, path: &Path) {
    let manifest = &prepared.manifest;
    crate::v8host::set_plugin_imports(&manifest.id, imports_from_manifest(manifest));
    crate::v8host::set_plugin_publishes(&manifest.id, manifest.publishes.clone());
    if let Some(gd) = prepared.gamedata.as_deref() {
        crate::gamedata_calls::register_plugin(&manifest.id, gd);
        crate::gamedata_hooks::register_plugin(&manifest.id, gd);
    }
    start_load(manifest, &prepared.js, cfg);
    crate::v8host::store_config_decls(&manifest.id, manifest.config.clone());
    WATCH_STATE.with(|w| { w.borrow_mut().insert(path.to_path_buf(), WatchedPlugin {
        mtime: stamp_time(prepared.stamp), id: manifest.id.clone(),
    }); });
    FILE_STAMPS.with(|s| { s.borrow_mut().insert(path.to_path_buf(), prepared.stamp); });
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
fn imports_from_manifest(m: &Manifest) -> Vec<crate::interfaces::ImportSpec> {
    let mut out = Vec::new();
    for (name, range) in &m.plugin_dependencies {
        out.push(crate::interfaces::ImportSpec {
            name: name.clone(), range: range.clone(), kind: crate::interfaces::Kind::Hard,
            compiled_types_sha256: m.compiled_against.get(name).cloned(),
        });
    }
    for (name, range) in &m.optional_plugin_dependencies {
        out.push(crate::interfaces::ImportSpec {
            name: name.clone(), range: range.clone(), kind: crate::interfaces::Kind::Optional,
            compiled_types_sha256: m.compiled_against.get(name).cloned(),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// read_s2sp
// ---------------------------------------------------------------------------

/// Unzip a `.s2sp` archive from raw bytes and extract `(Manifest, plugin_js, gamedata_json)`.
///
/// `gamedata_json` is the RAW text of the optional `gamedata.json` member (`s2s build` packs the
/// plugin's parsed, platform-filtered gamedata there — spec §7). `None` when the archive has no such
/// member, which is every pre-slice `.s2sp`: absence is normal, never an error. Core does not parse
/// it here; the call registry owns that.
///
/// Returns `Err(named_reason)` when:
/// - `bytes` is not a valid zip archive
/// - `manifest.json` is absent or fails JSON parsing into `Manifest`
/// - `manifest.id` claims a RESERVED owner id (see below)
/// - `plugin.js` is absent or contains invalid UTF-8
pub fn read_s2sp(bytes: &[u8]) -> Result<(Manifest, String, Option<String>), String> {
    crate::loader_worker::parse_s2sp(bytes, crate::loader_worker::ParseLimits::default())
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
    /// Strong path identities paired with WATCH_STATE. They include inode/device/ctime on Unix,
    /// catching atomic replacement even when a writer preserves length and mtime.
    static FILE_STAMPS: std::cell::RefCell<HashMap<PathBuf, FileStamp>> =
        std::cell::RefCell::new(HashMap::new());
    /// Counts how many times `poll_plugins` has been called (throttle counter).
    static DRAIN_COUNT: Cell<u64> = Cell::new(0);
    /// Slice 6.12 (`sm plugins`): pending load/unload/reload requested from a command. Drained at the
    /// start of `poll_plugins` (the frame drain, OUTSIDE any command's isolate borrow) so the loader
    /// never runs re-entrantly. The natives only enqueue.
    static PENDING_OPS: std::cell::RefCell<HashMap<String, PendingOp>> = std::cell::RefCell::new(HashMap::new());
    /// Paths manually unloaded via `sm plugins unload` (path → id). `poll_plugins` must NOT auto-reload
    /// a suppressed file; `sm plugins load` un-suppresses it so the next scan loads it fresh.
    static SUPPRESSED: std::cell::RefCell<HashMap<PathBuf, String>> = std::cell::RefCell::new(HashMap::new());
    /// L1 lifecycle v2: plugins parked because a hard dependency was not yet published (id → parsed
    /// load). Re-checked every frame by `start_unblocked_waiters`: queued for budgeted application
    /// once every hard-dep interface is published, or after `LOAD_TIMEOUT_FRAMES` (resolved decision
    /// #3 — an unmet hard dep loads lazily; the proxy throws `InterfaceUnavailable` at call).
    static WAITING: std::cell::RefCell<HashMap<String, WaitingLoad>> = std::cell::RefCell::new(HashMap::new());
}

/// True if any plugin preparation or lifecycle application remains pending.
pub(crate) fn has_waiting() -> bool {
    WAITING.with(|w| !w.borrow().is_empty())
        || ACTIVE_BATCH.with(|b| b.borrow().is_some())
        || READY_APPLY.with(|q| !q.borrow().is_empty())
}

/// A parsed load parked pending its hard dependencies (L1 lifecycle v2).
struct WaitingLoad {
    row: PreparedLoad,
    since_frame: u64,
}

/// A command-requested plugin lifecycle op (Slice 6.12), keyed by plugin id.
#[derive(Clone, Copy)]
enum PendingOp { Unload, Reload, Load }

fn enqueue_intent(id: &str, op: PendingOp) -> bool {
    PENDING_OPS.with(|q| {
        let mut q = q.borrow_mut();
        let (max_items, max_bytes) = POLICY.with(|p| (p.request_items, p.request_bytes));
        if !q.contains_key(id) && q.len() >= max_items { return false; }
        let old_bytes = q.get(id).map_or(0, |_| id.len().saturating_add(32));
        let used_bytes: usize = q.keys().map(|key| key.len().saturating_add(32)).sum();
        if used_bytes.saturating_sub(old_bytes).saturating_add(id.len()).saturating_add(32) > max_bytes {
            return false;
        }
        q.insert(id.to_string(), op);
        true
    })
}

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
    enqueue_intent(id, PendingOp::Unload)
}
/// Enqueue a reload of a loaded plugin (or a re-load of a suppressed one). False if the id is unknown.
pub(crate) fn request_reload(id: &str) -> bool {
    let known = path_of_loaded(id).is_some()
        || SUPPRESSED.with(|s| s.borrow().values().any(|v| v == id));
    known && enqueue_intent(id, PendingOp::Reload)
}
/// L1 lifecycle v2: enqueue parked loads whose hard-dependency wait window has cleared. Called at
/// the tail of `v8host::finalize_loading_plugins` (a producer reaching Active may unblock consumers,
/// and every frame's drain re-checks the timeout). Actual validation and lifecycle work remain in
/// `poll_plugins`, where they consume the loader's per-frame budget.
pub(crate) fn start_unblocked_waiters() {
    let frame = crate::v8host::current_frame();
    let ready: Vec<(String, bool)> = WAITING.with(|w| {
        w.borrow()
            .iter()
            .filter_map(|(id, wl)| {
                let unblocked = wl.row.prepared.manifest.plugin_dependencies.keys().all(|n| crate::v8host::iface_published(n));
                let expired = frame.saturating_sub(wl.since_frame) > crate::v8host::LOAD_TIMEOUT_FRAMES;
                (unblocked || expired).then(|| (id.clone(), expired))
            })
            .collect()
    });
    for (id, expired) in ready {
        let Some(wl) = WAITING.with(|w| w.borrow_mut().remove(&id)) else { continue };
        READY_APPLY.with(|queue| queue.borrow_mut().push_back(ApplyItem {
            row: wl.row,
            allow_unmet_dependencies: expired,
        }));
    }
}

/// Enqueue a load of a suppressed (previously `sm plugins unload`ed) plugin. False if not suppressed.
pub(crate) fn request_load(id: &str) -> bool {
    let suppressed = SUPPRESSED.with(|s| s.borrow().values().any(|v| v == id));
    suppressed && enqueue_intent(id, PendingOp::Load)
}

/// Number of Post-drain calls between each real directory scan.
/// At ~64 Hz (CS2 default tick rate), `64` ≈ 1 second between scans.
const POLL_THROTTLE: u64 = 64;

/// Stop watching a plugin's config file (called from `unload_plugin` teardown).
pub(crate) fn unwatch_config_for(id: &str) {
    CONFIG_SEEDED.with(|s| { s.borrow_mut().remove(id); });
    let path = CONFIG_PATHS.with(|paths| paths.borrow_mut().remove(id));
    let Some(path) = path else { return };
    if CONFIG_PATHS.with(|paths| paths.borrow().values().any(|other| *other == path)) { return; }
    let generation = CONFIG_WATCH_GENERATIONS.with(|generations| generations.borrow_mut().remove(&path));
    if let Some(generation) = generation {
        WORKER.with(|worker| {
            if let Some(worker) = worker.borrow().as_ref() {
                worker.retire_config_watch(path, generation);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Off-thread loader coordinator
// ---------------------------------------------------------------------------

/// Loader apply has its own soft budget. It runs after the general async drain and therefore is
/// deliberately outside that drain's 2 ms soft budget.
pub(crate) const CONFIG_PATH_RESOLVER_ABI_V1: u32 = 1;
pub(crate) type ConfigPathResolver = extern "C" fn(*const c_char) -> *const c_char;

#[derive(Clone)]
enum ConfigConsumer {
    Plugin { path: PathBuf, revision: u64 },
    Watch { id: String, generation: u64 },
    Permissions,
}

struct PendingConfig { revision: u64, consumers: Vec<ConfigConsumer>, bytes: usize }

#[derive(Clone, Copy, Default)]
struct MainMetrics {
    pending_rejected_items: u64,
    pending_rejected_bytes: u64,
    pending_high_water_items: usize,
    pending_high_water_bytes: usize,
}

#[derive(Clone, Copy, Default)]
struct RetainedUsage {
    items: usize,
    bytes: usize,
    rejected_items: u64,
    rejected_bytes: u64,
    high_water_items: usize,
    high_water_bytes: usize,
}

#[derive(Clone)]
struct RetainedLedger {
    usage: Rc<Cell<RetainedUsage>>,
    max_items: usize,
    max_bytes: usize,
}

impl RetainedLedger {
    fn new(max_items: usize, max_bytes: usize) -> Self {
        Self { usage: Rc::new(Cell::new(RetainedUsage::default())), max_items, max_bytes }
    }

    fn try_acquire(&self, bytes: usize) -> Option<RetainedLease> {
        let mut usage = self.usage.get();
        let items_rejected = usage.items >= self.max_items;
        let bytes_rejected = usage.bytes.saturating_add(bytes) > self.max_bytes;
        if items_rejected || bytes_rejected {
            usage.rejected_items = usage.rejected_items.saturating_add(u64::from(items_rejected));
            usage.rejected_bytes = usage.rejected_bytes.saturating_add(u64::from(bytes_rejected));
            self.usage.set(usage);
            return None;
        }
        usage.items += 1;
        usage.bytes += bytes;
        usage.high_water_items = usage.high_water_items.max(usage.items);
        usage.high_water_bytes = usage.high_water_bytes.max(usage.bytes);
        self.usage.set(usage);
        Some(RetainedLease { ledger: self.clone(), bytes })
    }

    fn metrics(&self) -> RetainedUsage { self.usage.get() }

    #[cfg(test)]
    fn usage(&self) -> (usize, usize) {
        let usage = self.usage.get();
        (usage.items, usage.bytes)
    }
}

struct RetainedLease { ledger: RetainedLedger, bytes: usize }

impl RetainedLease {
    fn try_grow(&mut self, bytes: usize) -> bool {
        let mut usage = self.ledger.usage.get();
        if usage.bytes.saturating_add(bytes) > self.ledger.max_bytes {
            usage.rejected_bytes = usage.rejected_bytes.saturating_add(1);
            self.ledger.usage.set(usage);
            return false;
        }
        usage.bytes += bytes;
        usage.high_water_bytes = usage.high_water_bytes.max(usage.bytes);
        self.ledger.usage.set(usage);
        self.bytes += bytes;
        true
    }
}

impl Drop for RetainedLease {
    fn drop(&mut self) {
        let usage = self.ledger.usage.get();
        self.ledger.usage.set(RetainedUsage {
            items: usage.items.saturating_sub(1),
            bytes: usage.bytes.saturating_sub(self.bytes),
            ..usage
        });
    }
}

struct LoaderDrainBudget {
    max_items: usize,
    max_bytes: usize,
    deadline: Instant,
    items: usize,
    bytes: usize,
}

impl LoaderDrainBudget {
    fn new(max_items: usize, max_bytes: usize, max_time: Duration) -> Self {
        Self {
            max_items,
            max_bytes,
            deadline: Instant::now() + max_time,
            items: 0,
            bytes: 0,
        }
    }

    fn try_admit(&mut self, bytes: usize) -> bool {
        if self.items >= self.max_items
            || (self.items > 0
                && (self.bytes.saturating_add(bytes) > self.max_bytes
                    || Instant::now() >= self.deadline))
        {
            return false;
        }
        self.items += 1;
        self.bytes = self.bytes.saturating_add(bytes);
        true
    }

    fn can_poll_unknown(&self) -> bool {
        self.items < self.max_items
            && (self.items == 0
                || (self.bytes < self.max_bytes && Instant::now() < self.deadline))
    }

    fn charge_polled(&mut self, bytes: usize) {
        self.items += 1;
        self.bytes = self.bytes.saturating_add(bytes);
    }

    #[cfg(test)]
    fn items(&self) -> usize { self.items }
}

struct PreparedLoad {
    path: PathBuf,
    old_id: Option<String>,
    prepared: PreparedPlugin,
    config: Option<ConfigSnapshot>,
    lease: RetainedLease,
}

struct ActiveBatch {
    pending: HashMap<PathBuf, u64>,
    prepared: HashMap<PathBuf, PreparedLoad>,
}

struct ApplyItem { row: PreparedLoad, allow_unmet_dependencies: bool }

struct ApplyingGuard;

impl ApplyingGuard {
    fn new() -> Self {
        APPLYING.with(|count| count.set(count.get().saturating_add(1)));
        Self
    }
}

impl Drop for ApplyingGuard {
    fn drop(&mut self) {
        APPLYING.with(|count| count.set(count.get().saturating_sub(1)));
    }
}

thread_local! {
    static WORKER: std::cell::RefCell<Option<LoaderWorker>> = std::cell::RefCell::new(None);
    static POLICY: LoaderPolicy = crate::async_limits::policy().loader.clone();
    static SCAN_REVISION: Cell<u64> = const { Cell::new(0) };
    static SCAN_IN_FLIGHT: Cell<bool> = const { Cell::new(false) };
    // Expected revisions live only in ACTIVE_BATCH.pending. Never retain old path names
    // just to mint a distinct revision after cancellation or delete/recreate.
    static NEXT_PATH_REVISION: Cell<u64> = const { Cell::new(0) };
    static ACTIVE_BATCH: std::cell::RefCell<Option<ActiveBatch>> = const { std::cell::RefCell::new(None) };
    static READY_APPLY: std::cell::RefCell<VecDeque<ApplyItem>> = const { std::cell::RefCell::new(VecDeque::new()) };
    static CONFIG_PENDING: std::cell::RefCell<HashMap<PathBuf, PendingConfig>> = std::cell::RefCell::new(HashMap::new());
    static CONFIG_PATHS: std::cell::RefCell<HashMap<String, PathBuf>> = std::cell::RefCell::new(HashMap::new());
    static CONFIG_WATCH_GENERATIONS: std::cell::RefCell<HashMap<PathBuf, u64>> = std::cell::RefCell::new(HashMap::new());
    static NEXT_CONFIG_WATCH_GENERATION: Cell<u64> = const { Cell::new(0) };
    static CONFIG_SEEDED: std::cell::RefCell<std::collections::HashSet<String>> = std::cell::RefCell::new(std::collections::HashSet::new());
    static PERMISSIONS_SCAN_PENDING: Cell<bool> = const { Cell::new(false) };
    static CONFIG_RESOLVER: Cell<Option<ConfigPathResolver>> = const { Cell::new(None) };
    static RETAINED_LEDGER: std::cell::RefCell<RetainedLedger> = {
        let policy = crate::async_limits::policy().loader.clone();
        std::cell::RefCell::new(RetainedLedger::new(policy.prepared_items, policy.prepared_bytes))
    };
    static MAIN_METRICS: Cell<MainMetrics> = Cell::new(MainMetrics::default());
    static APPLYING: Cell<usize> = const { Cell::new(0) };
}

static LIFECYCLE_EPOCH: AtomicU64 = AtomicU64::new(0);

fn current_epoch() -> u64 { LIFECYCLE_EPOCH.load(Ordering::Acquire) }

pub(crate) fn set_config_path_resolver(version: u32, resolver: Option<ConfigPathResolver>) -> bool {
    if version != CONFIG_PATH_RESOLVER_ABI_V1 { return false; }
    CONFIG_RESOLVER.with(|r| r.set(resolver));
    true
}

fn resolve_config_path(id: &str) -> Result<PathBuf, String> {
    let resolver = CONFIG_RESOLVER.with(Cell::get).ok_or_else(|| format!(
        "config('{}'): shim did not register config-path resolver ABI v{}", id, CONFIG_PATH_RESOLVER_ABI_V1
    ))?;
    let id_c = CString::new(id).map_err(|_| format!("config({id:?}): id contains NUL"))?;
    let ptr = resolver(id_c.as_ptr());
    if ptr.is_null() { return Err(format!("config('{}'): config-path resolver returned null", id)); }
    let path = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    if path.is_empty() { return Err(format!("config('{}'): config-path resolver returned an empty path", id)); }
    Ok(PathBuf::from(path))
}

pub(crate) fn set_plugins_dir(path: &str) {
    PLUGINS_DIR.with(|d| *d.borrow_mut() = Some(PathBuf::from(path)));
    let started = WORKER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() { return true; }
        LIFECYCLE_EPOCH.fetch_add(1, Ordering::AcqRel);
        let policy = POLICY.with(Clone::clone);
        RETAINED_LEDGER.with(|ledger| {
            *ledger.borrow_mut() = RetainedLedger::new(policy.prepared_items, policy.prepared_bytes);
        });
        MAIN_METRICS.with(|metrics| metrics.set(MainMetrics::default()));
        match LoaderWorker::start(policy) {
            Ok(worker) => { *slot = Some(worker); true }
            Err(reason) => { crate::v8host::log_warn(&format!("WARN: plugin loader disabled: {reason}")); false }
        }
    });
    if !started { PLUGINS_DIR.with(|d| { d.borrow_mut().take(); }); }
    crate::v8host::refresh_detour();
}

pub(crate) fn is_watching() -> bool {
    PLUGINS_DIR.with(|d| d.borrow().is_some()) && WORKER.with(|w| w.borrow().is_some())
}

fn drain_command_intents() {
    let mut ops: Vec<(String, PendingOp)> = PENDING_OPS.with(|q| q.borrow_mut().drain().collect());
    ops.sort_by(|a, b| a.0.cmp(&b.0));
    for (id, op) in ops {
        match op {
            PendingOp::Unload => {
                // The detached candidate stays charged through onUnload. Pair its locator with
                // the row so tuple drop order releases the payload before the applying guard.
                let waiting = WAITING.with(|w| w.borrow_mut().remove(&id))
                    .map(|item| (item, ApplyingGuard::new()));
                let waiting_retained_old = waiting.as_ref().is_some_and(|(item, _)| item.row.old_id.is_some());
                if let Some(path) = path_of_loaded(&id) {
                    cancel_path_work(&path);
                    if waiting.is_none() || waiting_retained_old { crate::v8host::unload_plugin(&id); }
                    crate::v8host::clear_pending_handoff(&id);
                    WATCH_STATE.with(|w| { w.borrow_mut().remove(&path); });
                    FILE_STAMPS.with(|s| { s.borrow_mut().remove(&path); });
                    SUPPRESSED.with(|s| { s.borrow_mut().insert(path, id.clone()); });
                    crate::v8host::log_warn(&format!("[plugins] unloaded '{}' (sm plugins unload)", id));
                }
            }
            PendingOp::Reload => {
                if crate::v8host::is_loading(&id) {
                    crate::v8host::queue_pending_reload(&id);
                    crate::v8host::log_warn(&format!("[plugins] reload '{}' queued (still loading)", id));
                    continue;
                }
                let path = path_of_loaded(&id).or_else(|| SUPPRESSED.with(|s| {
                    s.borrow().iter().find(|(_, v)| **v == id).map(|(p, _)| p.clone())
                }));
                if let Some(path) = path {
                    cancel_path_work(&path);
                    SUPPRESSED.with(|s| { s.borrow_mut().remove(&path); });
                    FILE_STAMPS.with(|s| { s.borrow_mut().remove(&path); });
                }
            }
            PendingOp::Load => {
                let path = SUPPRESSED.with(|s| s.borrow().iter().find(|(_, v)| **v == id).map(|(p, _)| p.clone()));
                if let Some(path) = path { cancel_path_work(&path); }
                SUPPRESSED.with(|s| { s.borrow_mut().retain(|_, v| *v != id); });
            }
        }
    }
}

fn cancel_path_work(path: &Path) {
    ACTIVE_BATCH.with(|batch| {
        if let Some(batch) = batch.borrow_mut().as_mut() {
            batch.pending.remove(path);
            batch.prepared.remove(path);
        }
    });
    READY_APPLY.with(|ready| ready.borrow_mut().retain(|item| item.row.path != path));
    WAITING.with(|waiting| waiting.borrow_mut().retain(|_, item| item.row.path != path));
    finish_batch_if_ready();
}

fn remove_batch_path(path: &Path) -> Option<PreparedLoad> {
    ACTIVE_BATCH.with(|batch| {
        let mut batch = batch.borrow_mut();
        let batch = batch.as_mut()?;
        batch.pending.remove(path);
        batch.prepared.remove(path)
    })
}

pub(crate) fn watch_config_for(id: &str) {
    if CONFIG_PATHS.with(|paths| paths.borrow().contains_key(id)) { return; }
    match resolve_config_path(id) {
        Ok(path) => {
            let generation = CONFIG_WATCH_GENERATIONS.with(|generations| {
                if let Some(generation) = generations.borrow().get(&path).copied() {
                    return generation;
                }
                let generation = NEXT_CONFIG_WATCH_GENERATION.with(|next| {
                    let generation = next.get().wrapping_add(1);
                    next.set(generation);
                    generation
                });
                generations.borrow_mut().insert(path.clone(), generation);
                generation
            });
            CONFIG_PATHS.with(|p| { p.borrow_mut().insert(id.to_string(), path.clone()); });
            let _ = queue_config(path, ConfigConsumer::Watch { id: id.to_string(), generation });
        }
        Err(reason) => crate::v8host::log_warn(&format!("WARN: {reason}")),
    }
}

fn next_path_revision() -> u64 {
    NEXT_PATH_REVISION.with(|revision| {
        let next = revision.get().checked_add(1).expect("loader revision exhausted");
        revision.set(next);
        next
    })
}

fn queue_config(path: PathBuf, consumer: ConfigConsumer) -> bool {
    fn same_consumer(a: &ConfigConsumer, b: &ConfigConsumer) -> bool {
        match (a, b) {
            (ConfigConsumer::Permissions, ConfigConsumer::Permissions) => true,
            (ConfigConsumer::Watch { id: a, .. }, ConfigConsumer::Watch { id: b, .. }) => a == b,
            (ConfigConsumer::Plugin { path: a, .. }, ConfigConsumer::Plugin { path: b, .. }) => a == b,
            _ => false,
        }
    }
    fn consumer_bytes(consumer: &ConfigConsumer) -> usize {
        match consumer {
            ConfigConsumer::Permissions => 64,
            ConfigConsumer::Watch { id, .. } => id.len().saturating_add(72),
            ConfigConsumer::Plugin { path, .. } => path.as_os_str().to_string_lossy().len().saturating_add(64),
        }
    }
    let new_bytes = consumer_bytes(&consumer);
    let (can_admit_consumer, items_rejected, bytes_rejected) = CONFIG_PENDING.with(|p| {
        let p = p.borrow();
        let used_items: usize = p.values().map(|row| row.consumers.len()).sum();
        let used_bytes: usize = p.values().map(|row| row.bytes).sum();
        let duplicate_bytes = p.get(&path).and_then(|row| row.consumers.iter()
            .find(|existing| same_consumer(existing, &consumer))).map_or(0, consumer_bytes);
        let (max_items, max_bytes) = POLICY.with(|policy| (policy.request_items, policy.request_bytes));
        let items_rejected = duplicate_bytes == 0 && used_items >= max_items;
        let bytes_rejected = used_bytes.saturating_sub(duplicate_bytes).saturating_add(new_bytes) > max_bytes;
        (!items_rejected && !bytes_rejected, items_rejected, bytes_rejected)
    });
    if !can_admit_consumer {
        MAIN_METRICS.with(|metrics| {
            let mut value = metrics.get();
            value.pending_rejected_items = value.pending_rejected_items.saturating_add(u64::from(items_rejected));
            value.pending_rejected_bytes = value.pending_rejected_bytes.saturating_add(u64::from(bytes_rejected));
            metrics.set(value);
        });
        crate::v8host::log_warn(&format!("WARN: loader config consumer {:?} deferred by bounded coalescing table", path));
        return false;
    }
    // Pending consumers can outlive final unwatch. A new load/permissions consumer sharing the
    // path must not turn that stale delivery intent back into a persistent worker registration.
    let current_watch_generation = |consumer: &ConfigConsumer| match consumer {
        ConfigConsumer::Watch { id, generation }
            if CONFIG_PATHS.with(|paths| paths.borrow().get(id) == Some(&path))
                && CONFIG_WATCH_GENERATIONS.with(|generations| generations.borrow().get(&path).copied() == Some(*generation)) => Some(*generation),
        _ => None,
    };
    let mut watch_generation = current_watch_generation(&consumer);
    CONFIG_PENDING.with(|pending| {
        if let Some(row) = pending.borrow().get(&path) {
            for existing in &row.consumers {
                if let Some(generation) = current_watch_generation(existing) {
                    watch_generation = Some(watch_generation.map_or(generation, |old| old.max(generation)));
                }
            }
        }
    });
    let revision = CONFIG_PENDING.with(|p| p.borrow().get(&path).map_or(1, |row| row.revision.wrapping_add(1)));
    let epoch = current_epoch();
    let submit = WORKER.with(|w| w.borrow().as_ref().map(|w| w.try_read_config(epoch, revision, path.clone(), watch_generation)).unwrap_or(Submit::Stopped));
    if !matches!(submit, Submit::Accepted | Submit::Coalesced) {
        crate::v8host::log_warn(&format!("WARN: loader config read {:?} deferred by bounded queue ({submit:?})", path));
        return false;
    }
    CONFIG_PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let row = p.entry(path).or_insert(PendingConfig { revision, consumers: Vec::new(), bytes: 0 });
        row.revision = revision;
        if let Some(index) = row.consumers.iter().position(|existing| same_consumer(existing, &consumer)) {
            row.bytes = row.bytes.saturating_sub(consumer_bytes(&row.consumers[index]));
            row.consumers.remove(index);
        }
        row.bytes = row.bytes.saturating_add(new_bytes);
        row.consumers.push(consumer);
    });
    let (pending_items, pending_bytes) = pending_usage();
    MAIN_METRICS.with(|metrics| {
        let mut value = metrics.get();
        value.pending_high_water_items = value.pending_high_water_items.max(pending_items);
        value.pending_high_water_bytes = value.pending_high_water_bytes.max(pending_bytes);
        metrics.set(value);
    });
    true
}

fn pending_usage() -> (usize, usize) {
    CONFIG_PENDING.with(|pending| {
        let pending = pending.borrow();
        (
            pending.values().map(|row| row.consumers.len()).sum(),
            pending.values().map(|row| row.bytes).sum(),
        )
    })
}

fn commit_path(path: &Path, stamp: FileStamp, id: &str) {
    FILE_STAMPS.with(|s| { s.borrow_mut().insert(path.to_path_buf(), stamp); });
    WATCH_STATE.with(|w| { w.borrow_mut().insert(path.to_path_buf(), WatchedPlugin {
        mtime: stamp_time(stamp), id: id.to_string(),
    }); });
}

fn handle_scan(revision: u64, entries: Result<Vec<(PathBuf, FileStamp)>, String>) {
    SCAN_IN_FLIGHT.with(|s| s.set(false));
    if revision != SCAN_REVISION.with(Cell::get) { return; }
    let entries = match entries {
        Ok(entries) => entries,
        Err(reason) => { crate::v8host::log_warn(&format!("WARN: poll_plugins: {reason}; keeping prior state")); return; }
    };
    let current: HashMap<PathBuf, FileStamp> = entries.into_iter().collect();
    FILE_STAMPS.with(|stamps| stamps.borrow_mut().retain(|path, _| current.contains_key(path)));
    let vanished: Vec<(PathBuf, String)> = WATCH_STATE.with(|w| w.borrow().iter()
        .filter(|(path, _)| !current.contains_key(*path))
        .map(|(path, row)| (path.clone(), row.id.clone())).collect());
    for (path, id) in vanished {
        let retained_old_was_running = WAITING.with(|waiting| waiting.borrow().values()
            .find(|item| item.row.path == path).map(|item| item.row.old_id.is_some()))
            .or_else(|| READY_APPLY.with(|ready| ready.borrow().iter()
                .find(|item| item.row.path == path).map(|item| item.row.old_id.is_some())));
        cancel_path_work(&path);
        if retained_old_was_running != Some(false) { crate::v8host::unload_plugin(&id); }
        crate::v8host::clear_pending_handoff(&id);
        crate::v8host::clear_failed(&id);
        WATCH_STATE.with(|w| { w.borrow_mut().remove(&path); });
        FILE_STAMPS.with(|s| { s.borrow_mut().remove(&path); });
    }
    let mut batch = ActiveBatch { pending: HashMap::new(), prepared: HashMap::new() };
    for (path, stamp) in current {
        if SUPPRESSED.with(|s| s.borrow().contains_key(&path)) { continue; }
        if FILE_STAMPS.with(|s| s.borrow().get(&path).copied()) == Some(stamp) { continue; }
        WAITING.with(|waiting| waiting.borrow_mut().retain(|_, item| item.row.path != path));
        READY_APPLY.with(|ready| ready.borrow_mut().retain(|item| item.row.path != path));
        let revision = next_path_revision();
        let epoch = current_epoch();
        let submit = WORKER.with(|w| w.borrow().as_ref().map(|w| w.try_prepare(epoch, revision, path.clone())).unwrap_or(Submit::Stopped));
        if matches!(submit, Submit::Accepted | Submit::Coalesced) {
            batch.pending.insert(path, revision);
        } else {
            crate::v8host::log_warn(&format!("WARN: poll_plugins: {:?} deferred by bounded loader queue ({submit:?})", path));
        }
    }
    if !batch.pending.is_empty() { ACTIVE_BATCH.with(|b| *b.borrow_mut() = Some(batch)); }
}

fn refuse_prepared(path: &Path, prepared: &PreparedPlugin, reason: &str, old_id: Option<&str>) {
    crate::v8host::log_warn(&format!("WARN: poll_plugins: refusing {:?}: {}{}", path, reason,
        if old_id.is_some() { " - keeping the running version" } else { "" }));
    if old_id.is_none() { crate::v8host::set_failed(&prepared.manifest.id, reason); }
    commit_path(path, prepared.stamp, old_id.unwrap_or(&prepared.manifest.id));
}

fn handle_plugin(revision: u64, path: PathBuf, result: Result<PreparedPlugin, String>) {
    let expected = ACTIVE_BATCH.with(|b| b.borrow().as_ref().and_then(|b| b.pending.get(&path).copied()));
    if expected != Some(revision) { return; }
    let old_id = WATCH_STATE.with(|w| w.borrow().get(&path).map(|r| r.id.clone()));
    let prepared = match result {
        Ok(prepared) => prepared,
        Err(reason) => {
            crate::v8host::log_warn(&format!("WARN: poll_plugins: failed to prepare {:?}: {reason}", path));
            ACTIVE_BATCH.with(|b| { if let Some(b) = b.borrow_mut().as_mut() { b.pending.remove(&path); } });
            finish_batch_if_ready();
            return;
        }
    };
    let validation = if !api_version_compatible(&prepared.manifest.api_version) {
        Err(format!("apiVersion {:?} incompatible with host major {} (rebuild with a matching @s2script/sdk)", prepared.manifest.api_version, HOST_API_VERSION_MAJOR))
    } else { Ok(()) };
    if let Err(reason) = validation {
        refuse_prepared(&path, &prepared, &reason, old_id.as_deref());
        ACTIVE_BATCH.with(|b| { if let Some(b) = b.borrow_mut().as_mut() { b.pending.remove(&path); } });
        finish_batch_if_ready();
        return;
    }
    let weight = prepared.bytes();
    let lease = RETAINED_LEDGER.with(|ledger| ledger.borrow().try_acquire(weight));
    let Some(lease) = lease else {
        crate::v8host::log_warn(&format!("WARN: poll_plugins: {:?} retryable retained-payload pressure; baseline unchanged", path));
        ACTIVE_BATCH.with(|b| { if let Some(b) = b.borrow_mut().as_mut() { b.pending.remove(&path); } });
        finish_batch_if_ready();
        return;
    };
    ACTIVE_BATCH.with(|b| {
        let mut b = b.borrow_mut(); let b = b.as_mut().unwrap();
        b.prepared.insert(path.clone(), PreparedLoad { path: path.clone(), old_id, prepared, config: None, lease });
    });
    let needs_config = ACTIVE_BATCH.with(|b| b.borrow().as_ref().and_then(|b| b.prepared.get(&path))
        .is_some_and(|p| !p.prepared.manifest.config.is_empty()));
    if needs_config {
        let id = ACTIVE_BATCH.with(|b| b.borrow().as_ref().unwrap().prepared[&path].prepared.manifest.id.clone());
        match resolve_config_path(&id) {
            Ok(config_path) if queue_config(config_path.clone(), ConfigConsumer::Plugin { path: path.clone(), revision }) => {}
            Ok(_) => {
                let _ = remove_batch_path(&path);
                finish_batch_if_ready();
            }
            Err(reason) => {
                let row = remove_batch_path(&path);
                if let Some(row) = row { refuse_prepared(&path, &row.prepared, &reason, row.old_id.as_deref()); }
                finish_batch_if_ready();
            }
        }
    } else {
        ACTIVE_BATCH.with(|b| { if let Some(b) = b.borrow_mut().as_mut() { b.pending.remove(&path); } });
        finish_batch_if_ready();
    }
}

fn handle_config(
    path: PathBuf,
    revision: u64,
    watch_generation: Option<u64>,
    result: Result<ConfigSnapshot, String>,
) {
    let has_proposal = result.as_ref().is_ok_and(|snapshot| {
        matches!(snapshot.watch, WatchDelta::Seed | WatchDelta::Changed)
    });
    let pending = CONFIG_PENDING.with(|p| {
        if p.borrow().get(&path).is_some_and(|row| row.revision == revision) { p.borrow_mut().remove(&path) } else { None }
    });
    let Some(pending) = pending else {
        if let Some(generation) = watch_generation.filter(|_| has_proposal) {
            WORKER.with(|worker| {
                if let Some(worker) = worker.borrow().as_ref() {
                    worker.resolve_config_watch(path, generation, revision, false);
                }
            });
        }
        return;
    };
    if let Ok(snapshot) = &result {
        debug_assert_eq!(snapshot.content.is_some(), snapshot.stamp.is_some());
    }
    let mut permissions_resolved = false;
    let mut current_watchers = 0usize;
    for consumer in pending.consumers {
        match consumer {
            ConfigConsumer::Plugin { path: plugin_path, revision: plugin_revision } => {
                let valid = ACTIVE_BATCH.with(|b| b.borrow().as_ref().and_then(|b| b.pending.get(&plugin_path).copied())) == Some(plugin_revision);
                if !valid { continue; }
                match &result {
                    Ok(snapshot) => {
                        let attached = ACTIVE_BATCH.with(|b| {
                            if let Some(b) = b.borrow_mut().as_mut() {
                                if let Some(row) = b.prepared.get_mut(&plugin_path) {
                                    if row.lease.try_grow(snapshot.bytes()) {
                                        row.config = Some(snapshot.clone());
                                        b.pending.remove(&plugin_path);
                                        return true;
                                    }
                                }
                            }
                            false
                        });
                        if !attached {
                            crate::v8host::log_warn(&format!("WARN: poll_plugins: config for {:?} deferred by retained-payload pressure; baseline unchanged", plugin_path));
                            let _ = remove_batch_path(&plugin_path);
                        }
                    }
                    Err(reason) => {
                        crate::v8host::log_warn(&format!("WARN: poll_plugins: config read for {:?} failed: {reason}; baseline unchanged", plugin_path));
                        let _ = remove_batch_path(&plugin_path);
                    }
                }
            }
            ConfigConsumer::Watch { id, generation } => if let Ok(snapshot) = &result {
                let current = Some(generation) == watch_generation
                    && CONFIG_PATHS.with(|paths| paths.borrow().get(&id) == Some(&path))
                    && CONFIG_WATCH_GENERATIONS.with(|generations| generations.borrow().get(&path).copied() == Some(generation));
                if !current { continue; }
                current_watchers += 1;
                if snapshot.watch == WatchDelta::Pressure {
                    crate::v8host::log_warn(&format!("WARN: config watch {:?} deferred by bounded worker baseline; keeping prior values", path));
                    continue;
                }
                let seeded = CONFIG_SEEDED.with(|s| !s.borrow_mut().insert(id.clone()));
                if !seeded {
                    // A queued first read (including a shared-path Unchanged result) may
                    // already differ from this plugin's applied initial values.
                    crate::v8host::reconcile_initial_config_snapshot(&id, snapshot.content.as_deref());
                } else if snapshot.watch == WatchDelta::Changed {
                    crate::v8host::re_materialize_config_snapshot(&id, snapshot.content.as_deref());
                }
            },
            ConfigConsumer::Permissions => {
                permissions_resolved = true;
                match &result {
                    Ok(snapshot) => if let Some(content) = snapshot.content.as_deref() {
                        match load_permissions_from_str(content) {
                            Ok(()) => PERMISSIONS_WARNED.store(false, std::sync::atomic::Ordering::Relaxed),
                            Err(reason) if !PERMISSIONS_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) => crate::v8host::log_warn(&format!(
                                "WARN: {reason} - ignoring the operator allow-list (every gated capability stays denied)")),
                            Err(_) => {}
                        }
                    },
                    Err(reason) => crate::v8host::log_warn(&format!("WARN: permissions config read failed: {reason}; keeping prior allow-list")),
                }
            }
        }
    }
    let watch_handled = current_watchers > 0 && result.as_ref().is_ok_and(|snapshot| {
        matches!(snapshot.watch, WatchDelta::Seed | WatchDelta::Changed | WatchDelta::Unchanged)
    });
    if let Some(generation) = watch_generation.filter(|_| has_proposal) {
        WORKER.with(|worker| {
            if let Some(worker) = worker.borrow().as_ref() {
                worker.resolve_config_watch(path.clone(), generation, revision, watch_handled);
            }
        });
    }
    finish_batch_if_ready();
    if permissions_resolved {
        PERMISSIONS_SCAN_PENDING.with(|pending| pending.set(false));
        schedule_scan();
    }
}

fn finish_batch_if_ready() {
    if !ACTIVE_BATCH.with(|b| b.borrow().as_ref().is_some_and(|b| b.pending.is_empty())) { return; }
    let batch = ACTIVE_BATCH.with(|b| b.borrow_mut().take()).unwrap();
    let mut by_id: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for row in batch.prepared.values() { by_id.entry(row.prepared.manifest.id.clone()).or_default().push(row.path.clone()); }
    let mut rejected = std::collections::HashSet::new();
    for (id, paths) in &by_id {
        let live_elsewhere = WATCH_STATE.with(|w| w.borrow().iter().any(|(path, row)| row.id == *id && !paths.contains(path)));
        if paths.len() > 1 || live_elsewhere {
            crate::v8host::log_warn(&format!("WARN: poll_plugins: duplicate manifest id {:?} at {:?}; keeping the running version", id, paths));
            rejected.extend(paths.iter().cloned());
        }
    }
    let candidates: Vec<(PathBuf, &Manifest)> = batch.prepared.values()
        .map(|row| (row.path.clone(), &row.prepared.manifest)).collect();
    let drift_rejected = resolve_contract_rejections(&candidates, &rejected, |name| {
        crate::v8host::iface_published_types_sha256(name)
    });
    for (path, reason) in &drift_rejected {
        if let Some(row) = batch.prepared.get(path) {
            refuse_prepared(path, &row.prepared, reason, row.old_id.as_deref());
        }
    }
    rejected.extend(drift_rejected.keys().cloned());
    let mut loads: HashMap<String, PreparedLoad> = HashMap::new();
    let mut order_input = Vec::new();
    for (_, row) in batch.prepared {
        if rejected.contains(&row.path) {
            FILE_STAMPS.with(|s| { s.borrow_mut().insert(row.path.clone(), row.prepared.stamp); });
            WATCH_STATE.with(|w| {
                if let Some(existing) = w.borrow_mut().get_mut(&row.path) {
                    existing.mtime = stamp_time(row.prepared.stamp);
                }
            });
            continue;
        }
        let id = row.prepared.manifest.id.clone();
        order_input.push((id.clone(), row.prepared.manifest.plugin_dependencies.keys().cloned().collect(), row.prepared.manifest.publishes.keys().cloned().collect()));
        loads.insert(id, row);
    }
    let mut ready = VecDeque::new();
    for id in topo_order(&order_input) {
        let Some(row) = loads.remove(&id) else { continue };
        ready.push_back(ApplyItem { row, allow_unmet_dependencies: false });
    }
    READY_APPLY.with(|queue| queue.borrow_mut().extend(ready));
}

fn duplicate_manifest_id(path: &Path, id: &str) -> bool {
    WATCH_STATE.with(|watch| watch.borrow().iter().any(|(other, row)| other != path && row.id == id))
}

fn apply_prepared(item: ApplyItem) {
    let _applying = ApplyingGuard::new();
    let ApplyItem { row, allow_unmet_dependencies } = item;
    let id = row.prepared.manifest.id.clone();
    if !api_version_compatible(&row.prepared.manifest.api_version) {
        let reason = format!("apiVersion {:?} incompatible with host major {} (rebuild with a matching @s2script/sdk)", row.prepared.manifest.api_version, HOST_API_VERSION_MAJOR);
        refuse_prepared(&row.path, &row.prepared, &reason, row.old_id.as_deref());
        return;
    }
    if duplicate_manifest_id(&row.path, &id) {
        crate::v8host::log_warn(&format!("WARN: poll_plugins: duplicate manifest id {:?} at {:?}; keeping the running version", id, row.path));
        FILE_STAMPS.with(|stamps| { stamps.borrow_mut().insert(row.path.clone(), row.prepared.stamp); });
        if let Some(old_id) = row.old_id.as_deref() {
            WATCH_STATE.with(|watch| {
                if let Some(existing) = watch.borrow_mut().get_mut(&row.path) {
                    existing.id = old_id.to_string();
                    existing.mtime = stamp_time(row.prepared.stamp);
                }
            });
        }
        return;
    }
    if let Err(reason) = verify_compiled_against_batch(&row.prepared.manifest, &HashMap::new()) {
        refuse_prepared(&row.path, &row.prepared, &reason, row.old_id.as_deref());
        return;
    }
    let dependencies_ready = deps_satisfied(&row.prepared.manifest);
    if !dependencies_ready && !allow_unmet_dependencies {
        let watch_id = row.old_id.as_deref().unwrap_or(&id);
        commit_path(&row.path, row.prepared.stamp, watch_id);
        let frame = crate::v8host::current_frame();
        WAITING.with(|waiting| {
            waiting.borrow_mut().insert(id, WaitingLoad { row, since_frame: frame });
        });
        return;
    }
    if !dependencies_ready {
        crate::v8host::log_warn(&format!(
            "WARN: '{}': hard dependency producer not Active after ~30s - loading anyway (calls will throw InterfaceUnavailable until it appears)",
            id
        ));
    }
    if let Some(old_id) = row.old_id.as_deref() {
        if crate::v8host::is_loading(old_id) {
            crate::v8host::queue_pending_reload(old_id);
            commit_path(&row.path, row.prepared.stamp, old_id);
            crate::v8host::log_warn(&format!("[plugins] reload '{}' queued (still loading)", old_id));
            return;
        }
    }
    let override_json = row.config.as_ref().and_then(|snapshot| snapshot.content.as_deref());
    let cfg = crate::v8host::materialize_for_load_snapshot(
        &id,
        &row.prepared.manifest.config,
        override_json,
    );
    if let Some(old_id) = row.old_id.as_deref() { crate::v8host::unload_plugin(old_id); }
    begin_load_prepared(&row.prepared, &cfg, &row.path);
}

fn drain_ready(budget: &mut LoaderDrainBudget) {
    loop {
        let Some(item) = take_ready(budget) else { return };
        apply_prepared(item);
    }
}

fn take_ready(budget: &mut LoaderDrainBudget) -> Option<ApplyItem> {
    let weight = READY_APPLY.with(|queue| queue.borrow().front().map(|item| item.row.lease.bytes))?;
    if !budget.try_admit(weight) { return None; }
    READY_APPLY.with(|queue| queue.borrow_mut().pop_front())
}

fn schedule_scan() {
    if SCAN_IN_FLIGHT.with(Cell::get)
        || ACTIVE_BATCH.with(|b| b.borrow().is_some())
        || READY_APPLY.with(|queue| !queue.borrow().is_empty())
    { return; }
    let watched: Vec<(String, PathBuf, u64)> = CONFIG_PATHS.with(|paths| paths.borrow().iter().filter_map(|(id, path)| {
        CONFIG_WATCH_GENERATIONS.with(|generations| generations.borrow().get(path).copied())
            .map(|generation| (id.clone(), path.clone(), generation))
    }).collect());
    for (id, path, generation) in watched {
        let _ = queue_config(path, ConfigConsumer::Watch { id, generation });
    }
    let Some(dir) = PLUGINS_DIR.with(|d| d.borrow().clone()) else { return };
    let revision = SCAN_REVISION.with(|r| { let n = r.get().wrapping_add(1); r.set(n); n });
    let epoch = current_epoch();
    let submit = WORKER.with(|w| w.borrow().as_ref().map(|w| w.try_scan(epoch, revision, dir)).unwrap_or(Submit::Stopped));
    if matches!(submit, Submit::Accepted | Submit::Coalesced) { SCAN_IN_FLIGHT.with(|s| s.set(true)); }
}

fn schedule_periodic() {
    if SCAN_IN_FLIGHT.with(Cell::get)
        || ACTIVE_BATCH.with(|b| b.borrow().is_some())
        || READY_APPLY.with(|queue| !queue.borrow().is_empty())
        || PERMISSIONS_SCAN_PENDING.with(Cell::get)
    { return; }
    if let Ok(path) = resolve_config_path(PERMISSIONS_CONFIG_ID) {
        // Do not merely enqueue this read ahead of the scan: a coalesced in-flight read can put its
        // rerun behind a later scan. Admit the scan only after this exact result was applied.
        if queue_config(path, ConfigConsumer::Permissions) {
            PERMISSIONS_SCAN_PENDING.with(|pending| pending.set(true));
        }
        return;
    }
    schedule_scan();
}

pub(crate) fn poll_plugins() {
    drain_command_intents();
    let (max_items, max_bytes, max_time) = POLICY.with(|p| (p.drain_items, p.drain_bytes, Duration::from_micros(p.drain_micros)));
    let mut budget = LoaderDrainBudget::new(max_items, max_bytes, max_time);
    let epoch = current_epoch();
    while budget.can_poll_unknown() {
        let result = WORKER.with(|w| w.borrow().as_ref().and_then(LoaderWorker::try_result));
        let Some(result) = result else { break };
        budget.charge_polled(result.weight());
        match result {
            WorkerResult::Scan { epoch: e, revision, entries } if e == epoch => handle_scan(revision, entries),
            WorkerResult::Plugin { epoch: e, revision, path, prepared } if e == epoch => handle_plugin(revision, path, prepared),
            WorkerResult::Config { epoch: e, revision, path, watch_generation, snapshot } if e == epoch => {
                handle_config(path, revision, watch_generation, snapshot)
            }
            WorkerResult::Config {
                revision,
                path,
                watch_generation: Some(generation),
                snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Seed | WatchDelta::Changed, .. }),
                ..
            } => {
                WORKER.with(|worker| {
                    if let Some(worker) = worker.borrow().as_ref() {
                        worker.resolve_config_watch(path, generation, revision, false);
                    }
                })
            }
            _ => {}
        }
    }
    drain_ready(&mut budget);
    let count = DRAIN_COUNT.with(|c| { let n = c.get(); c.set(n.wrapping_add(1)); n });
    if count % POLL_THROTTLE == 0 { schedule_periodic(); }
}

/// Loader ownership is sampled from the worker and main thread independently. Queue/result state
/// locates the shared obligation rows; those counts are not additional retained ownership.
pub(crate) fn metrics() -> serde_json::Value {
    let worker = WORKER.with(|slot| slot.borrow().as_ref().map(LoaderWorker::metrics).unwrap_or_default());
    let running = WORKER.with(|slot| slot.borrow().is_some());
    let (pending_items, pending_bytes) = pending_usage();
    let active = ACTIVE_BATCH.with(|batch| batch.borrow().as_ref().map_or(0, |batch| batch.prepared.len()));
    let ready = READY_APPLY.with(|queue| queue.borrow().len());
    let waiting = WAITING.with(|rows| rows.borrow().len());
    let applying = APPLYING.with(Cell::get);
    let retained = RETAINED_LEDGER.with(|ledger| ledger.borrow().metrics());
    let main = MAIN_METRICS.with(Cell::get);
    let limits = &crate::async_limits::policy().loader;
    serde_json::json!({
        "running": running,
        "worker": {
            "obligations": { "items": worker.obligations_items, "requestBytes": worker.request_bytes, "resultBytes": worker.result_bytes },
            "queued": worker.queued, "inFlight": worker.in_flight, "results": worker.results,
            "controls": { "items": worker.control_items, "bytes": worker.control_bytes, "pending": worker.controls_pending },
            "config": { "paths": worker.config_paths, "bytes": worker.config_bytes },
            "baselines": { "items": worker.baseline_items, "bytes": worker.baseline_bytes },
            "proposals": { "items": worker.proposal_items, "bytes": worker.proposal_bytes },
        },
        "main": {
            "pending": { "items": pending_items, "bytes": pending_bytes },
            "active": active, "ready": ready, "waiting": waiting, "applying": applying,
            "retained": { "items": retained.items, "bytes": retained.bytes },
        },
        "rejected": {
            "requestItems": worker.rejected.request_items, "requestBytes": worker.rejected.request_bytes,
            "resultItems": worker.rejected.result_items, "resultBytes": worker.rejected.result_bytes,
            "controlItems": worker.rejected.control_items, "controlBytes": worker.rejected.control_bytes,
            "configPaths": worker.rejected.config_paths, "configBytes": worker.rejected.config_bytes,
            "pendingItems": main.pending_rejected_items, "pendingBytes": main.pending_rejected_bytes,
            "retainedItems": retained.rejected_items, "retainedBytes": retained.rejected_bytes,
        },
        "highWater": {
            "obligations": worker.high_water.obligations, "requestBytes": worker.high_water.request_bytes,
            "resultBytes": worker.high_water.result_bytes, "queued": worker.high_water.queued,
            "inFlight": worker.high_water.in_flight, "results": worker.high_water.results,
            "controlItems": worker.high_water.control_items, "controlBytes": worker.high_water.control_bytes,
            "configPaths": worker.high_water.config_paths, "configBytes": worker.high_water.config_bytes,
            "baselineItems": worker.high_water.baseline_items, "baselineBytes": worker.high_water.baseline_bytes,
            "proposalItems": worker.high_water.proposal_items, "proposalBytes": worker.high_water.proposal_bytes,
            "pendingItems": main.pending_high_water_items, "pendingBytes": main.pending_high_water_bytes,
            "retainedItems": retained.high_water_items, "retainedBytes": retained.high_water_bytes,
        },
        "limits": {
            "requestItems": limits.request_items, "requestBytes": limits.request_bytes,
            "resultItems": limits.result_items, "resultBytes": limits.result_bytes,
            "preparedItems": limits.prepared_items, "preparedBytes": limits.prepared_bytes,
            "scanEntries": limits.scan_entries, "scanCandidates": limits.scan_candidates,
            "pathBytes": limits.path_bytes, "archiveBytes": limits.archive_bytes,
            "configBytes": limits.config_bytes, "configBaselineItems": limits.config_baseline_items,
            "configBaselineBytes": limits.config_baseline_bytes,
            "parse": {
                "zipEntries": limits.parse.zip_entries, "memberNameBytes": limits.parse.member_name_bytes,
                "manifestBytes": limits.parse.manifest_bytes, "pluginJsBytes": limits.parse.plugin_js_bytes,
                "gamedataBytes": limits.parse.gamedata_bytes,
            },
            "drainItems": limits.drain_items, "drainBytes": limits.drain_bytes, "drainMicros": limits.drain_micros,
        },
    })
}

pub(crate) fn shutdown_worker() {
    LIFECYCLE_EPOCH.fetch_add(1, Ordering::AcqRel);
    PLUGINS_DIR.with(|d| { d.borrow_mut().take(); });
    CONFIG_RESOLVER.with(|r| r.set(None));
    let worker = WORKER.with(|w| w.borrow_mut().take());
    if let Some(worker) = worker { worker.shutdown(); }
    SCAN_IN_FLIGHT.with(|s| s.set(false));
    ACTIVE_BATCH.with(|b| { b.borrow_mut().take(); });
    READY_APPLY.with(|queue| queue.borrow_mut().clear());
    CONFIG_PENDING.with(|p| p.borrow_mut().clear());
    CONFIG_PATHS.with(|p| p.borrow_mut().clear());
    CONFIG_WATCH_GENERATIONS.with(|generations| generations.borrow_mut().clear());
    NEXT_CONFIG_WATCH_GENERATION.with(|next| next.set(0));
    CONFIG_SEEDED.with(|s| s.borrow_mut().clear());
    PERMISSIONS_SCAN_PENDING.with(|pending| pending.set(false));
    FILE_STAMPS.with(|s| s.borrow_mut().clear());
    WATCH_STATE.with(|w| w.borrow_mut().clear());
    SUPPRESSED.with(|s| s.borrow_mut().clear());
    PENDING_OPS.with(|p| p.borrow_mut().clear());
    WAITING.with(|w| w.borrow_mut().clear());
    SCAN_REVISION.with(|r| r.set(0));
    DRAIN_COUNT.with(|c| c.set(0));
    if let Ok(mut permissions) = PERMISSIONS.write() { *permissions = None; }
    PERMISSIONS_WARNED.store(false, std::sync::atomic::Ordering::Relaxed);
    let policy = &crate::async_limits::policy().loader;
    RETAINED_LEDGER.with(|ledger| {
        *ledger.borrow_mut() = RetainedLedger::new(policy.prepared_items, policy.prepared_bytes);
    });
    MAIN_METRICS.with(|metrics| metrics.set(MainMetrics::default()));
    APPLYING.with(|count| count.set(0));
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn delayed_coalesced_first_watch_and_joining_watcher_apply_unseen_edit() {
        first_watch_scenario(true);
    }

    #[test]
    fn delayed_first_watch_and_joining_watcher_suppress_unchanged_defaults() {
        first_watch_scenario(false);
    }

    fn first_watch_scenario(edit_after_registration: bool) {
        use crate::v8host::{self, frame_tests::*};
        use std::sync::{Arc, Condvar, Mutex};
        extern "C" fn path_resolver(_: *const c_char) -> *const c_char {
            b"/tmp/s2-final-review-config-seed.json\0".as_ptr().cast()
        }
        shutdown_worker();
        v8host::init(dummy_logger()).unwrap();
        let path = PathBuf::from("/tmp/s2-final-review-config-seed.json");
        let blocker = PathBuf::from("/tmp/s2-final-review-config-blocker.json");
        std::fs::write(&path, br#"{"greeting":"A"}"#).unwrap();
        std::fs::write(&blocker, b"{}").unwrap();
        let gate = Arc::new((Mutex::new((false, false)), Condvar::new()));
        *crate::loader_worker::TEST_READ_GATE.lock().unwrap() = Some(gate.clone());
        WORKER.with(|slot| {
            *slot.borrow_mut() = Some(LoaderWorker::start(LoaderPolicy::default()).unwrap())
        });
        WORKER.with(|slot| {
            assert_eq!(
                slot.borrow().as_ref().unwrap().try_read_config(
                    current_epoch(),
                    1,
                    blocker.clone(),
                    None
                ),
                Submit::Accepted
            )
        });
        {
            let state = gate.0.lock().unwrap();
            let (state, timeout) = gate
                .1
                .wait_timeout_while(state, Duration::from_secs(2), |s| !s.0)
                .unwrap();
            assert!(!timeout.timed_out() && state.0);
        }
        assert!(set_config_path_resolver(
            CONFIG_PATH_RESOLVER_ABI_V1,
            Some(path_resolver)
        ));
        let decls = HashMap::from([
            (
                "greeting".into(),
                crate::config::ConfigEntry::Decl(crate::config::ConfigDecl {
                    r#type: "string".into(),
                    default: serde_json::json!("A"),
                    ..Default::default()
                }),
            ),
            (
                "scale".into(),
                crate::config::ConfigEntry::Decl(crate::config::ConfigDecl {
                    r#type: "float".into(),
                    default: serde_json::json!(1.0),
                    ..Default::default()
                }),
            ),
            (
                "escaped".into(),
                crate::config::ConfigEntry::Decl(crate::config::ConfigDecl {
                    r#type: "string".into(),
                    default: serde_json::json!("a\n雪"),
                    ..Default::default()
                }),
            ),
        ]);
        let register = |id: &str| {
            v8host::create_plugin_context(id);
            v8host::store_config_decls(id, decls.clone());
            v8host::eval_in_context(
                id,
                r#"
                // Different property order, JSON escapes, and 1 versus materialized 1.0.
                globalThis.__s2pkg_config_values = {scale:1, greeting:'A', escaped:'a\n\u96ea'};
                globalThis.seen = [];
                __s2pkg_config.config.onChange(cfg => seen.push(cfg.greeting));
            "#,
            )
            .unwrap();
        };
        register("seed-review");
        // Both watchers share a path and coalesce while the unrelated read blocks the worker.
        register("seed-coalesced");
        assert_eq!(
            CONFIG_PENDING.with(|p| p.borrow()[&path].consumers.len()),
            2
        );
        assert!(CONFIG_PATHS.with(|paths| paths.borrow().contains_key("seed-review")));
        // This edit is AFTER the synchronous registration boundary, before its worker read.
        if edit_after_registration {
            std::fs::write(&path, br#"{"greeting":"B"}"#).unwrap();
        } else {
            // Same applied defaults, now in the auto-generated JSONC representation.
            std::fs::write(&path, crate::config::generate_default_jsonc(&decls)).unwrap();
        }
        {
            let mut s = gate.0.lock().unwrap();
            s.1 = true;
            gate.1.notify_all();
        }
        let next = || {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(r) = WORKER.with(|slot| slot.borrow().as_ref().unwrap().try_result()) {
                    break r;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        };
        let _ = next(); // blocker
        let mut deltas = Vec::new();
        let result = next();
        if let WorkerResult::Config {
            path,
            revision,
            watch_generation,
            snapshot,
            ..
        } = result
        {
            deltas.push(format!("{:?}", snapshot.as_ref().unwrap().watch));
            handle_config(path, revision, watch_generation, snapshot);
        } else {
            panic!("expected config");
        }
        // A normal subsequent poll sees the acknowledged baseline as unchanged.
        let generation = CONFIG_WATCH_GENERATIONS.with(|g| g.borrow()[&path]);
        assert!(queue_config(
            path.clone(),
            ConfigConsumer::Watch {
                id: "seed-review".into(),
                generation
            }
        ));
        if let WorkerResult::Config {
            path,
            revision,
            watch_generation,
            snapshot,
            ..
        } = next()
        {
            deltas.push(format!("{:?}", snapshot.as_ref().unwrap().watch));
            handle_config(path, revision, watch_generation, snapshot);
        } else {
            panic!("expected config");
        }
        register("seed-joined");
        if let WorkerResult::Config {
            path,
            revision,
            watch_generation,
            snapshot,
            ..
        } = next()
        {
            assert_eq!(snapshot.as_ref().unwrap().watch, WatchDelta::Unchanged);
            deltas.push(format!("{:?}", snapshot.as_ref().unwrap().watch));
            handle_config(path, revision, watch_generation, snapshot);
        } else {
            panic!("expected joined config");
        }
        let observed: Vec<_> = ["seed-review", "seed-coalesced", "seed-joined"]
            .iter()
            .map(|id| {
                eval_in_context_string(
                    id,
                    "JSON.stringify([__s2pkg_config.config.getString('greeting'), seen])",
                )
            })
            .collect();
        shutdown_worker();
        *crate::loader_worker::TEST_READ_GATE.lock().unwrap() = None;
        v8host::shutdown();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(blocker).unwrap();
        let expected = if edit_after_registration {
            r#"["B",["B"]]"#
        } else {
            r#"["A",[]]"#
        };
        assert_eq!(observed, vec![expected; 3], "worker deltas: {deltas:?}");
    }

    #[test]
    fn removed_plugin_paths_do_not_accumulate_revision_tombstones() {
        shutdown_worker();
        let root =
            std::env::temp_dir().join(format!("s2-final-review-churn-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let policy = LoaderPolicy {
            request_items: 1,
            result_items: 1,
            prepared_items: 1,
            ..LoaderPolicy::default()
        };
        WORKER.with(|slot| *slot.borrow_mut() = Some(LoaderWorker::start(policy.clone()).unwrap()));
        let mut revisions = Vec::new();
        for i in 0..64 {
            let path = root.join(format!("plugin-{i}.s2sp"));
            std::fs::write(&path, b"not a zip").unwrap();
            let entries = crate::loader_worker::scan_plugins(&root, &policy).unwrap();
            assert_eq!(entries.len(), 1);
            SCAN_REVISION.with(|r| r.set(i * 2 + 1));
            handle_scan(i * 2 + 1, Ok(entries));
            let deadline = Instant::now() + Duration::from_secs(2);
            let result = loop {
                if let Some(r) = WORKER.with(|slot| slot.borrow().as_ref().unwrap().try_result()) {
                    break r;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            };
            if let WorkerResult::Plugin {
                revision,
                path,
                prepared,
                ..
            } = result
            {
                assert!(prepared.is_err());
                revisions.push(revision);
                handle_plugin(revision, path, prepared);
            } else {
                panic!("expected plugin result");
            }
            std::fs::remove_file(&path).unwrap();
            SCAN_REVISION.with(|r| r.set(i * 2 + 2));
            handle_scan(
                i * 2 + 2,
                Ok(crate::loader_worker::scan_plugins(&root, &policy).unwrap()),
            );
        }
        // Inspect coordinator path ownership too: zero payload gauges alone missed the old
        // revision tombstones. Revisions are now scalar, not an additional path-name table.
        let historical_paths = WATCH_STATE.with(|s| s.borrow().len())
            + FILE_STAMPS.with(|s| s.borrow().len())
            + SUPPRESSED.with(|s| s.borrow().len())
            + CONFIG_PENDING.with(|s| s.borrow().len())
            + CONFIG_PATHS.with(|s| s.borrow().len())
            + CONFIG_WATCH_GENERATIONS.with(|s| s.borrow().len());
        assert!(ACTIVE_BATCH.with(|s| s.borrow().is_none()));
        assert!(READY_APPLY.with(|s| s.borrow().is_empty()));
        assert!(WAITING.with(|s| s.borrow().is_empty()));
        let live = (
            WATCH_STATE.with(|s| s.borrow().len()),
            FILE_STAMPS.with(|s| s.borrow().len()),
        );
        let m = metrics();
        shutdown_worker();
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(live, (0, 0));
        assert_eq!(m["worker"]["obligations"]["items"], 0);
        assert!(
            revisions.windows(2).all(|pair| pair[1] > pair[0]),
            "revisions must be unique across retired paths: {revisions:?}"
        );
        assert_eq!(
            historical_paths, 0,
            "historical paths remain while idle: {m}"
        );
        assert_eq!(m["main"]["active"], 0);
        assert_eq!(m["main"]["ready"], 0);
        assert_eq!(m["main"]["waiting"], 0);
        assert_eq!(m["main"]["retained"]["items"], 0);
    }

    #[test]
    fn cancelled_and_deleted_recreated_paths_reject_late_worker_results() {
        shutdown_worker();
        let root = std::env::temp_dir().join(format!("s2-final-recreate-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("plugin.s2sp");
        let policy = LoaderPolicy::default();
        WORKER.with(|slot| *slot.borrow_mut() = Some(LoaderWorker::start(policy.clone()).unwrap()));
        let write_plugin = |version: &str| {
            let manifest =
                format!(r#"{{"id":"recreated","version":"{version}","apiVersion":"2.x"}}"#);
            std::fs::write(&path, make_test_s2sp(&manifest, "module.exports = {};")).unwrap();
        };
        let scan = |revision| {
            SCAN_REVISION.with(|r| r.set(revision));
            handle_scan(
                revision,
                Ok(crate::loader_worker::scan_plugins(&root, &policy).unwrap()),
            );
        };
        let next_plugin = || {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(WorkerResult::Plugin {
                    revision, prepared, ..
                }) = WORKER.with(|slot| slot.borrow().as_ref().unwrap().try_result())
                {
                    break (revision, prepared.unwrap());
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        };
        write_plugin("1");
        scan(1);
        let (old_revision, old_prepared) = next_plugin();
        cancel_path_work(&path);
        handle_plugin(old_revision, path.clone(), Ok(old_prepared.clone()));
        assert!(READY_APPLY.with(|ready| ready.borrow().is_empty()));
        std::fs::remove_file(&path).unwrap();
        scan(2);
        write_plugin("2");
        scan(3);
        let (new_revision, new_prepared) = next_plugin();
        assert!(new_revision > old_revision);
        // Deliver a valid but cancelled result after the replacement was admitted.
        handle_plugin(old_revision, path.clone(), Ok(old_prepared));
        assert_eq!(
            ACTIVE_BATCH.with(|b| b.borrow().as_ref().unwrap().pending[&path]),
            new_revision
        );
        assert!(READY_APPLY.with(|ready| ready.borrow().is_empty()));
        handle_plugin(new_revision, path.clone(), Ok(new_prepared));
        assert!(ACTIVE_BATCH.with(|b| b.borrow().is_none()));
        READY_APPLY.with(|ready| {
            let ready = ready.borrow();
            assert_eq!(ready.len(), 1);
            assert_eq!(ready[0].row.prepared.manifest.version, "2");
        });
        shutdown_worker();
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(metrics()["main"]["retained"]["items"], 0);
    }

    extern "C" fn test_config_path(_id: *const c_char) -> *const c_char {
        b"/tmp/s2script-config.json\0".as_ptr().cast()
    }

    fn prepared_row(
        manifest: Manifest,
        old_id: Option<&str>,
        bytes: usize,
        ledger: &RetainedLedger,
    ) -> PreparedLoad {
        let prepared = PreparedPlugin::for_test(
            manifest,
            "module.exports.OnPluginStart = function() {};",
            bytes,
        );
        PreparedLoad {
            path: PathBuf::from(format!("{}.s2sp", prepared.manifest.id)),
            old_id: old_id.map(str::to_string),
            prepared,
            config: None,
            lease: ledger.try_acquire(bytes).expect("test payload fits"),
        }
    }

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

    /// Build an in-memory `.s2sp` zip with a third `gamedata.json` member (what `s2s build` packs).
    fn make_test_s2sp_with_gamedata(manifest_json: &str, plugin_js: &str, gamedata_json: &str) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let opts = || zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

        writer.start_file("manifest.json", opts()).expect("start manifest.json");
        writer.write_all(manifest_json.as_bytes()).expect("write manifest.json");
        writer.start_file("plugin.js", opts()).expect("start plugin.js");
        writer.write_all(plugin_js.as_bytes()).expect("write plugin.js");
        writer.start_file("gamedata.json", opts()).expect("start gamedata.json");
        writer.write_all(gamedata_json.as_bytes()).expect("write gamedata.json");

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
        let (m, js, _gd) = read_s2sp(&bytes).expect("valid s2sp");
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

    /// A `.s2sp` may not claim a RESERVED owner id (A5b). That namespace is permission-exempt for
    /// declared engine calls and is keyed identically in the ledger, so a plugin holding one would
    /// both inherit the exemption and tear the runtime's descriptors down on its own unload.
    /// `read_s2sp` is the single door every load/reload path goes through.
    #[test]
    fn read_s2sp_refuses_a_manifest_claiming_a_reserved_owner_id() {
        let spoofed = crate::gamedata_calls::reserved_owner_id("@s2script/cs2");
        let bytes = make_test_s2sp(
            &format!(r#"{{"id":"{spoofed}","version":"9.9.9","apiVersion":"2.x"}}"#),
            "module.exports.default={__s2plugin:1,factory:function(ctx){}};",
        );
        let err = read_s2sp(&bytes).expect_err("a reserved owner id must be refused");
        assert!(err.contains("reserved"), "the refusal must NAME why: {err}");
        // And the game package's own npm name is NOT the reserved id, so a legitimately-named
        // first-party plugin under the @s2script scope still loads.
        let ok = make_test_s2sp(
            r#"{"id":"@s2script/zones","version":"1.0.0","apiVersion":"2.x"}"#,
            "module.exports.default={__s2plugin:1,factory:function(ctx){}};",
        );
        assert!(read_s2sp(&ok).is_ok(), "an @s2script-scoped plugin id is still legal");
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
        let (m, _js, _gd) = read_s2sp(&bytes).expect("valid s2sp");
        assert_eq!(m.plugin_dependencies.get("@demo/greeter").map(String::as_str), Some("^1.0.0"));
        assert_eq!(m.optional_plugin_dependencies.get("@demo/extra").map(String::as_str), Some("^1.0.0"));
    }

    #[test]
    fn manifest_defaults_missing_dependency_maps_to_empty() {
        let bytes = make_test_s2sp(
            r#"{"id":"@demo/x","version":"0.1.0","apiVersion":"2.x"}"#,
            "module.exports.default={__s2plugin:1,factory:function(ctx){}};",
        );
        let (m, _js, _gd) = read_s2sp(&bytes).expect("valid s2sp");
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
        let (m, _js, _gd) = read_s2sp(&bytes).expect("legacy manifest parses");
        let imports = imports_from_manifest(&m);
        // Builtins are no longer skipped — they become phantom Hard deps (lazy, never called).
        assert_eq!(imports.len(), 2, "both builtin deps flow through post-deletion");
        assert!(imports.iter().all(|i| matches!(i.kind, crate::interfaces::Kind::Hard)));
        assert!(imports.iter().any(|i| i.name == "@s2script/entity"));
    }

    #[test]
    fn manifest_parses_compiled_against_and_flows_into_imports() {
        let json = r#"{"id":"@demo/c","version":"0.1.0","apiVersion":"2.x",
            "pluginDependencies":{"@x/if":"^1.0.0","@x/other":"^1.0.0"},
            "compiledAgainst":{"@x/if":"deadbeef"}}"#;
        let m: Manifest = serde_json::from_str(json).expect("parse");
        assert_eq!(m.compiled_against.get("@x/if").map(String::as_str), Some("deadbeef"));
        let imports = imports_from_manifest(&m);
        let with = imports.iter().find(|i| i.name == "@x/if").expect("present");
        assert_eq!(with.compiled_types_sha256.as_deref(), Some("deadbeef"));
        let without = imports.iter().find(|i| i.name == "@x/other").expect("present");
        assert_eq!(without.compiled_types_sha256, None);
    }

    #[test]
    fn manifest_without_compiled_against_defaults_empty() {
        let json = r#"{"id":"@demo/x","version":"0.1.0","apiVersion":"2.x"}"#;
        let m: Manifest = serde_json::from_str(json).expect("parse");
        assert!(m.compiled_against.is_empty());
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
    fn config_path_resolver_is_versioned_and_copies_the_returned_path() {
        CONFIG_RESOLVER.with(|r| r.set(None));
        assert!(!set_config_path_resolver(99, Some(test_config_path)));
        assert!(resolve_config_path("@demo/a").unwrap_err().contains("ABI v1"));
        assert!(set_config_path_resolver(CONFIG_PATH_RESOLVER_ABI_V1, Some(test_config_path)));
        assert_eq!(resolve_config_path("@demo/a").unwrap(), PathBuf::from("/tmp/s2script-config.json"));
        assert!(set_config_path_resolver(CONFIG_PATH_RESOLVER_ABI_V1, None));
    }

    #[test]
    fn compiled_against_prefers_the_prevalidated_in_batch_producer_hash() {
        let mut consumer: Manifest = serde_json::from_str(
            r#"{"id":"consumer","version":"1","apiVersion":"2.x","compiledAgainst":{"iface":"new-hash"}}"#,
        ).unwrap();
        consumer.plugin_dependencies.insert("iface".into(), "1.x".into());
        let batch = HashMap::from([("iface".to_string(), "new-hash".to_string())]);
        assert!(verify_compiled_against_batch(&consumer, &batch).is_ok());
        let stale = HashMap::from([("iface".to_string(), "old-hash".to_string())]);
        assert!(verify_compiled_against_batch(&consumer, &stale).unwrap_err().contains("contract drift"));
    }

    #[test]
    fn retained_payload_lease_stays_charged_across_moves_and_config_growth() {
        let ledger = RetainedLedger::new(2, 12);
        let mut lease = ledger.try_acquire(7).expect("first payload fits");
        assert_eq!(ledger.usage(), (1, 7));
        assert!(lease.try_grow(5));
        assert_eq!(ledger.usage(), (1, 12));
        assert!(!lease.try_grow(1));
        let metrics = ledger.metrics();
        assert_eq!((metrics.items, metrics.bytes), (1, 12));
        assert_eq!((metrics.rejected_items, metrics.rejected_bytes), (0, 1));
        assert_eq!((metrics.high_water_items, metrics.high_water_bytes), (1, 12));
        let moved = Some(lease);
        assert_eq!(ledger.usage(), (1, 12), "moving ownership must retain the charge");
        drop(moved);
        assert_eq!(ledger.usage(), (0, 0), "dropping ownership releases the charge");
    }

    #[test]
    fn loader_metrics_schema_reports_charged_ownership_and_resets_after_join() {
        shutdown_worker();
        let policy = crate::async_limits::policy().loader.clone();
        WORKER.with(|slot| *slot.borrow_mut() = Some(LoaderWorker::start(policy.clone()).unwrap()));
        let submit = WORKER.with(|slot| slot.borrow().as_ref().unwrap()
            .try_prepare(1, 1, PathBuf::from("metrics-owned.s2sp")));
        assert_eq!(submit, Submit::Accepted);
        let retained_lease = RETAINED_LEDGER.with(|ledger| ledger.borrow().try_acquire(73).unwrap());
        let live = metrics();
        assert_eq!(live["running"], true);
        assert_eq!(live["worker"]["obligations"]["items"], 1);
        assert_eq!(live["main"]["retained"]["items"], 1);
        assert_eq!(live["main"]["retained"]["bytes"], 73);
        assert_eq!(live["limits"]["requestItems"], policy.request_items);
        assert_eq!(live["limits"]["parse"]["zipEntries"], policy.parse.zip_entries);
        assert!(live["worker"]["config"]["paths"].is_number());
        assert!(live["rejected"]["configBytes"].is_number());
        assert!(live["highWater"]["proposalBytes"].is_number());
        drop(retained_lease);
        shutdown_worker();
        let stopped = metrics();
        assert_eq!(stopped["running"], false);
        assert_eq!(stopped["worker"]["obligations"]["items"], 0);
        assert_eq!(stopped["main"]["retained"]["items"], 0);
        assert_eq!(stopped["highWater"]["retainedItems"], 0);
    }

    #[test]
    fn loader_metrics_follow_retained_ownership_across_active_ready_and_waiting() {
        shutdown_worker();
        let ledger = RETAINED_LEDGER.with(|ledger| ledger.borrow().clone());
        let manifest: Manifest = serde_json::from_str(
            r#"{"id":"metrics-retained","version":"1","apiVersion":"2.x"}"#,
        ).unwrap();
        let row = prepared_row(manifest, None, 64, &ledger);
        ACTIVE_BATCH.with(|batch| *batch.borrow_mut() = Some(ActiveBatch {
            pending: HashMap::new(), prepared: HashMap::from([(row.path.clone(), row)]),
        }));
        let active = metrics();
        assert_eq!(active["main"]["active"], 1);
        assert_eq!(active["main"]["retained"]["items"], 1);
        let row = ACTIVE_BATCH.with(|batch| batch.borrow_mut().take().unwrap()
            .prepared.into_values().next().unwrap());
        READY_APPLY.with(|ready| ready.borrow_mut().push_back(ApplyItem { row, allow_unmet_dependencies: false }));
        let ready = metrics();
        assert_eq!(ready["main"]["ready"], 1);
        assert_eq!(ready["main"]["retained"]["items"], 1);
        let item = READY_APPLY.with(|ready| ready.borrow_mut().pop_front().unwrap());
        let applying_guard = ApplyingGuard::new();
        let applying = metrics();
        assert_eq!(applying["main"]["applying"], 1);
        assert_eq!(applying["main"]["retained"]["items"], 1);
        drop(applying_guard);
        WAITING.with(|waiting| waiting.borrow_mut().insert("metrics-retained".into(), WaitingLoad {
            row: item.row, since_frame: 0,
        }));
        let waiting = metrics();
        assert_eq!(waiting["main"]["waiting"], 1);
        assert_eq!(waiting["main"]["retained"]["items"], 1);
        shutdown_worker();
        assert_eq!(metrics()["main"]["retained"]["items"], 0);
    }

    #[test]
    fn pending_metrics_track_admission_and_dimension_rejections() {
        shutdown_worker();
        let policy = crate::async_limits::policy().loader.clone();
        WORKER.with(|slot| *slot.borrow_mut() = Some(LoaderWorker::start(policy.clone()).unwrap()));
        assert!(queue_config(PathBuf::from("metrics-pending.json"), ConfigConsumer::Permissions));
        let admitted = metrics();
        assert_eq!(admitted["main"]["pending"]["items"], 1);
        assert_eq!(admitted["highWater"]["pendingItems"], 1);
        CONFIG_PENDING.with(|pending| {
            let mut pending = pending.borrow_mut();
            pending.clear();
            pending.insert(PathBuf::from("held.json"), PendingConfig {
                revision: 1,
                consumers: vec![ConfigConsumer::Permissions; policy.request_items],
                bytes: policy.request_bytes,
            });
        });
        assert!(!queue_config(PathBuf::from("metrics-rejected.json"), ConfigConsumer::Permissions));
        let rejected = metrics();
        assert_eq!(rejected["rejected"]["pendingItems"], 1);
        assert_eq!(rejected["rejected"]["pendingBytes"], 1);
        shutdown_worker();
    }

    #[test]
    fn contract_fixed_point_removes_consumers_of_a_refused_candidate_producer() {
        let producer: Manifest = serde_json::from_str(
            r#"{
                "id":"producer","version":"2","apiVersion":"2.x",
                "publishes":{"iface":{"version":"2","typesSha256":"new"}},
                "compiledAgainst":{"gate":"wrong"}
            }"#,
        ).unwrap();
        let consumer: Manifest = serde_json::from_str(
            r#"{
                "id":"consumer","version":"2","apiVersion":"2.x",
                "compiledAgainst":{"iface":"new"}
            }"#,
        ).unwrap();
        let candidates = vec![
            (PathBuf::from("producer.s2sp"), &producer),
            (PathBuf::from("consumer.s2sp"), &consumer),
        ];
        let rejected = resolve_contract_rejections(&candidates, &std::collections::HashSet::new(), |name| {
            match name {
                "gate" => Some("current".to_string()),
                "iface" => Some("old".to_string()),
                _ => None,
            }
        });
        assert!(rejected.contains_key(Path::new("producer.s2sp")));
        assert!(rejected.contains_key(Path::new("consumer.s2sp")));
    }

    #[test]
    fn contract_resolution_reconsiders_a_consumer_against_the_retained_live_producer() {
        let producer: Manifest = serde_json::from_str(
            r#"{
                "id":"producer","version":"2","apiVersion":"2.x",
                "publishes":{"iface":{"version":"2","typesSha256":"new"}},
                "compiledAgainst":{"gate":"wrong"}
            }"#,
        ).unwrap();
        let consumer: Manifest = serde_json::from_str(
            r#"{
                "id":"consumer","version":"2","apiVersion":"2.x",
                "compiledAgainst":{"iface":"old"}
            }"#,
        ).unwrap();
        let candidates = vec![
            (PathBuf::from("producer.s2sp"), &producer),
            (PathBuf::from("consumer.s2sp"), &consumer),
        ];
        let rejected = resolve_contract_rejections(&candidates, &std::collections::HashSet::new(), |name| {
            match name {
                "gate" => Some("current".to_string()),
                "iface" => Some("old".to_string()),
                _ => None,
            }
        });
        assert!(rejected.contains_key(Path::new("producer.s2sp")));
        assert!(!rejected.contains_key(Path::new("consumer.s2sp")));
    }

    #[test]
    fn oscillating_provider_contract_is_refused_instead_of_selecting_an_arbitrary_state() {
        let provider: Manifest = serde_json::from_str(
            r#"{
                "id":"provider","version":"2","apiVersion":"2.x",
                "publishes":{"iface":{"version":"2","typesSha256":"new"}},
                "compiledAgainst":{"iface":"old"}
            }"#,
        ).unwrap();
        let path = PathBuf::from("provider.s2sp");
        let rejected = resolve_contract_rejections(
            &[(path.clone(), &provider)],
            &std::collections::HashSet::new(),
            |name| (name == "iface").then(|| "old".to_string()),
        );
        assert!(rejected[&path].contains("cycle"));
    }

    #[test]
    fn drain_budget_defers_a_second_large_apply_instead_of_draining_the_batch() {
        let mut budget = LoaderDrainBudget::new(8, 10, Duration::from_secs(1));
        assert!(budget.try_admit(7));
        assert!(!budget.try_admit(7));
        assert_eq!(budget.items(), 1);
    }

    #[test]
    fn ready_apply_queue_keeps_the_second_payload_charged_for_a_later_frame() {
        READY_APPLY.with(|queue| queue.borrow_mut().clear());
        let ledger = RetainedLedger::new(2, 20);
        let manifest_a: Manifest = serde_json::from_str(
            r#"{"id":"a","version":"1","apiVersion":"2.x"}"#,
        ).unwrap();
        let manifest_b: Manifest = serde_json::from_str(
            r#"{"id":"b","version":"1","apiVersion":"2.x"}"#,
        ).unwrap();
        READY_APPLY.with(|queue| {
            queue.borrow_mut().extend([
                ApplyItem { row: prepared_row(manifest_a, None, 7, &ledger), allow_unmet_dependencies: false },
                ApplyItem { row: prepared_row(manifest_b, None, 7, &ledger), allow_unmet_dependencies: false },
            ]);
        });
        let mut budget = LoaderDrainBudget::new(8, 10, Duration::from_secs(1));
        let first = take_ready(&mut budget).expect("first apply is admitted");
        assert!(take_ready(&mut budget).is_none(), "second apply must remain queued");
        assert_eq!(READY_APPLY.with(|queue| queue.borrow().len()), 1);
        assert_eq!(ledger.usage(), (2, 14));
        drop(first);
        READY_APPLY.with(|queue| queue.borrow_mut().clear());
        assert_eq!(ledger.usage(), (0, 0));
    }

    #[test]
    fn periodic_apply_queues_reload_instead_of_tearing_down_a_loading_factory() {
        crate::v8host::init(crate::v8host::frame_tests::dummy_logger()).unwrap();
        crate::v8host::load_plugin_js(
            "slow",
            "module.exports.default={__s2plugin:1,factory:function(){return new Promise(function(){});}};",
            "{}",
        );
        assert!(crate::v8host::is_loading("slow"));
        let ledger = RetainedLedger::new(1, 1024);
        let manifest: Manifest = serde_json::from_str(
            r#"{"id":"slow","version":"2","apiVersion":"2.x"}"#,
        ).unwrap();
        apply_prepared(ApplyItem {
            row: prepared_row(manifest, Some("slow"), 64, &ledger),
            allow_unmet_dependencies: false,
        });
        assert!(crate::v8host::is_loading("slow"), "the original loading generation must remain");
        assert_eq!(ledger.usage(), (0, 0));
        crate::v8host::shutdown();
    }

    #[test]
    fn waiting_release_revalidates_contract_against_the_now_live_producer() {
        crate::v8host::init(crate::v8host::frame_tests::dummy_logger()).unwrap();
        crate::v8host::set_plugin_publishes(
            "producer",
            HashMap::from([("iface".to_string(), PublishDecl {
                version: "1".to_string(),
                types_sha256: "old".to_string(),
            })]),
        );
        crate::v8host::frame_tests::load_body(
            "producer",
            r#"const {publishInterface}=require("@s2script/interfaces"); publishInterface("iface", {});"#,
            "{}",
        );
        assert_eq!(crate::v8host::iface_published_types_sha256("iface").as_deref(), Some("old"));

        let ledger = RetainedLedger::new(1, 1024);
        let manifest: Manifest = serde_json::from_str(
            r#"{
                "id":"consumer","version":"1","apiVersion":"2.x",
                "pluginDependencies":{"iface":"1.x"},
                "compiledAgainst":{"iface":"new"}
            }"#,
        ).unwrap();
        WAITING.with(|waiting| {
            waiting.borrow_mut().insert("consumer".to_string(), WaitingLoad {
                row: prepared_row(manifest, None, 64, &ledger),
                since_frame: crate::v8host::current_frame(),
            });
        });
        assert_eq!(ledger.usage(), (1, 64), "WAITING must retain the payload charge");
        start_unblocked_waiters();
        let mut budget = LoaderDrainBudget::new(8, 1024, Duration::from_secs(1));
        drain_ready(&mut budget);
        assert!(crate::v8host::is_failed("consumer"));
        assert_eq!(ledger.usage(), (0, 0));
        crate::v8host::shutdown();
    }

    #[test]
    fn invalid_waiting_reload_keeps_the_running_generation() {
        crate::v8host::init(crate::v8host::frame_tests::dummy_logger()).unwrap();
        crate::v8host::frame_tests::load_body("old", "", "{}");
        crate::v8host::set_plugin_publishes(
            "producer",
            HashMap::from([("iface".to_string(), PublishDecl {
                version: "1".to_string(),
                types_sha256: "old".to_string(),
            })]),
        );
        crate::v8host::frame_tests::load_body(
            "producer",
            r#"const {publishInterface}=require("@s2script/interfaces"); publishInterface("iface", {});"#,
            "{}",
        );

        let ledger = RetainedLedger::new(1, 1024);
        let manifest: Manifest = serde_json::from_str(
            r#"{
                "id":"replacement","version":"2","apiVersion":"2.x",
                "pluginDependencies":{"iface":"1.x"},
                "compiledAgainst":{"iface":"new"}
            }"#,
        ).unwrap();
        WAITING.with(|waiting| {
            waiting.borrow_mut().insert("replacement".to_string(), WaitingLoad {
                row: prepared_row(manifest, Some("old"), 64, &ledger),
                since_frame: crate::v8host::current_frame(),
            });
        });
        start_unblocked_waiters();
        let mut budget = LoaderDrainBudget::new(8, 1024, Duration::from_secs(1));
        drain_ready(&mut budget);

        assert_eq!(crate::v8host::plugin_phase("old"), Some(crate::plugin::Phase::Active));
        assert!(crate::v8host::plugin_phase("replacement").is_none());
        assert_eq!(ledger.usage(), (0, 0));
        crate::v8host::shutdown();
    }

    #[test]
    fn unload_of_a_same_id_waiting_reload_unloads_the_retained_old_generation() {
        shutdown_worker();
        crate::v8host::frame_tests::LOG.lock().unwrap().clear();
        crate::v8host::init(crate::v8host::frame_tests::logger).unwrap();
        crate::v8host::frame_tests::load_body("waiting-unload", r#"
            return { onUnload() {
                console.log('WAITING_UNLOAD_METRICS:' +
                    JSON.stringify(JSON.parse(__s2_async_stats()).loader.main));
            } };
        "#, "{}");
        let ledger = RETAINED_LEDGER.with(|ledger| ledger.borrow().clone());
        let manifest: Manifest = serde_json::from_str(
            r#"{"id":"waiting-unload","version":"2","apiVersion":"2.x","pluginDependencies":{"missing":"1.x"}}"#,
        ).unwrap();
        let row = prepared_row(manifest, Some("waiting-unload"), 64, &ledger);
        let path = row.path.clone();
        commit_path(&path, row.prepared.stamp, "waiting-unload");
        WAITING.with(|waiting| {
            waiting.borrow_mut().insert("waiting-unload".to_string(), WaitingLoad {
                row,
                since_frame: crate::v8host::current_frame(),
            });
        });

        assert!(request_unload("waiting-unload"));
        drain_command_intents();
        assert!(crate::v8host::plugin_phase("waiting-unload").is_none());
        assert!(WAITING.with(|waiting| waiting.borrow().is_empty()));
        assert!(READY_APPLY.with(|ready| ready.borrow().is_empty()));
        assert_eq!(ledger.usage(), (0, 0));
        assert!(SUPPRESSED.with(|suppressed| suppressed.borrow().contains_key(&path)));
        let after_unload = metrics();
        let logs = crate::v8host::frame_tests::LOG.lock().unwrap().clone();
        shutdown_worker();
        crate::v8host::shutdown();

        let raw = logs.iter().find_map(|line| line.split_once("WAITING_UNLOAD_METRICS:")
            .map(|(_, text)| text)).expect("onUnload emitted loader metrics");
        let main: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(main["retained"], serde_json::json!({ "items": 1, "bytes": 64 }),
            "the payload must remain charged throughout onUnload");
        let located: u64 = ["active", "ready", "waiting", "applying"].iter()
            .map(|key| main[key].as_u64().unwrap()).sum();
        assert_eq!(located, 1, "onUnload must locate its retained payload: {main}");
        assert_eq!(main["applying"], 1);
        assert_eq!(after_unload["main"]["retained"], serde_json::json!({ "items": 0, "bytes": 0 }));
        assert_eq!(after_unload["main"]["applying"], 0);
    }

    #[test]
    fn vanished_path_cancels_ready_candidate_and_cannot_resurrect_it() {
        crate::v8host::init(crate::v8host::frame_tests::dummy_logger()).unwrap();
        let ledger = RetainedLedger::new(1, 1024);
        let manifest: Manifest = serde_json::from_str(
            r#"{"id":"deleted-ready","version":"1","apiVersion":"2.x"}"#,
        ).unwrap();
        let row = prepared_row(manifest, None, 64, &ledger);
        let path = row.path.clone();
        commit_path(&path, row.prepared.stamp, "deleted-ready");
        READY_APPLY.with(|ready| ready.borrow_mut().push_back(ApplyItem {
            row,
            allow_unmet_dependencies: false,
        }));
        SCAN_REVISION.with(|revision| revision.set(7));

        handle_scan(7, Ok(Vec::new()));
        assert!(READY_APPLY.with(|ready| ready.borrow().is_empty()));
        assert_eq!(ledger.usage(), (0, 0));
        let mut budget = LoaderDrainBudget::new(8, 1024, Duration::from_secs(1));
        drain_ready(&mut budget);
        assert!(crate::v8host::plugin_phase("deleted-ready").is_none());
        assert!(!WATCH_STATE.with(|watch| watch.borrow().contains_key(&path)));
        shutdown_worker();
        crate::v8host::shutdown();
    }

    #[test]
    fn retired_pending_watch_does_not_reregister_for_a_permissions_consumer() {
        shutdown_worker();
        let root = std::env::temp_dir().join(format!("s2-loader-retired-pending-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("shared.json");
        std::fs::write(&path, b"{}").unwrap();
        let policy = LoaderPolicy { request_items: 1, result_items: 1, ..LoaderPolicy::default() };
        WORKER.with(|slot| *slot.borrow_mut() = Some(LoaderWorker::start(policy).unwrap()));
        CONFIG_PATHS.with(|paths| paths.borrow_mut().insert("old".into(), path.clone()));
        CONFIG_WATCH_GENERATIONS.with(|generations| generations.borrow_mut().insert(path.clone(), 1));
        assert!(queue_config(path.clone(), ConfigConsumer::Watch { id: "old".into(), generation: 1 }));
        let next_result = || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(result) = WORKER.with(|slot| slot.borrow().as_ref().unwrap().try_result()) {
                    break result;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
        };
        // The old watch consumer is still pending when a load sharing this config path arrives.
        let _undelivered = next_result();
        unwatch_config_for("old");
        assert!(queue_config(path.clone(), ConfigConsumer::Permissions));
        let next = next_result();
        let WorkerResult::Config { revision, watch_generation, snapshot, .. } = next else {
            panic!("expected config result");
        };
        handle_config(path, revision, watch_generation, snapshot);
        let admission = WORKER.with(|slot| slot.borrow().as_ref().unwrap()
            .try_read_config(current_epoch(), 1, root.join("second.json"), Some(2)));
        shutdown_worker();
        *PERMISSIONS.write().unwrap() = None;
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(watch_generation, None, "stale pending consumer revived retired watch intent");
        assert_eq!(admission, Submit::Accepted, "consumerless control reservation leaked capacity");
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

    /// B1: a refused (apiVersion-incompatible) .s2sp is remembered by path+mtime — one WARN,
    /// a `failed` state, and NO re-processing on later scans until the file changes.
    #[test]
    fn refused_load_is_remembered_by_path_and_mtime() {
        let dir = std::env::temp_dir().join(format!("s2s-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mk tempdir");
        let bytes = make_test_s2sp(
            r#"{"id":"@old/one","version":"0.1.0","apiVersion":"1.x"}"#,
            "module.exports={};",
        );
        let p = dir.join("old.s2sp");
        std::fs::write(&p, &bytes).expect("write s2sp");
        set_plugins_dir(dir.to_str().unwrap());
        for _ in 0..10_000 {
            poll_plugins();
            if WATCH_STATE.with(|ws| ws.borrow().contains_key(&p)) { break; }
            std::thread::yield_now();
        }

        assert!(
            WATCH_STATE.with(|ws| ws.borrow().contains_key(&p)),
            "refusal must be remembered as a WATCH_STATE row (path+mtime) — no rescan-and-rewarn"
        );
        assert!(crate::v8host::is_failed("@old/one"), "refusal is operator-visible as `failed`");

        // Unchanged file ⇒ the diff yields NO action (this IS warn-once, structurally).
        let mtime_before = WATCH_STATE.with(|ws| ws.borrow().get(&p).map(|w| w.mtime));
        for _ in 0..(POLL_THROTTLE as usize + 1) { poll_plugins(); std::thread::yield_now(); }
        let mtime_after = WATCH_STATE.with(|ws| ws.borrow().get(&p).map(|w| w.mtime));
        assert_eq!(mtime_before, mtime_after, "second scan must not re-process the refused file");

        // Cleanup so later tests on this (single) test thread see no leftovers.
        WATCH_STATE.with(|ws| { ws.borrow_mut().remove(&p); });
        crate::v8host::clear_failed("@old/one");
        shutdown_worker();
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir(&dir);
    }

    // -------------------------------------------------------------------
    // Plugin gamedata: manifest fields + the operator allow-list (spec §6/§7)
    // -------------------------------------------------------------------

    #[test]
    fn manifest_parses_permissions_and_gamedata() {
        let bytes = make_test_s2sp(
            r#"{"id":"@demo/gd","version":"0.1.0","apiVersion":"2.x","permissions":["engine:calls"]}"#,
            "module.exports.default={__s2plugin:1};",
        );
        let (m, _js, gd) = read_s2sp(&bytes).expect("valid s2sp");
        assert_eq!(m.permissions, vec!["engine:calls".to_string()]);
        assert!(gd.is_none(), "no gamedata.json member in this archive");
    }

    /// The packed member is returned VERBATIM (core never parses it here) and the manifest's
    /// author-side `gamedata` path parses alongside it.
    #[test]
    fn read_s2sp_returns_the_packed_gamedata_member() {
        let gd = r#"{"signatures":{},"calls":{"ignite":{"receiver":{"kind":"entity"}}}}"#;
        let bytes = make_test_s2sp_with_gamedata(
            r#"{"id":"@demo/gd","version":"0.1.0","apiVersion":"2.x",
                "permissions":["engine:calls"],"gamedata":"gamedata/gd.gamedata.jsonc"}"#,
            "module.exports.default={__s2plugin:1};",
            gd,
        );
        let (m, _js, packed) = read_s2sp(&bytes).expect("valid s2sp");
        assert_eq!(packed.as_deref(), Some(gd), "raw gamedata text crosses unaltered");
        assert_eq!(m.gamedata.as_deref(), Some("gamedata/gd.gamedata.jsonc"));
    }

    #[test]
    fn manifest_without_permissions_defaults_empty() {
        let bytes = make_test_s2sp(
            r#"{"id":"@demo/p","version":"0.1.0","apiVersion":"2.x"}"#,
            "module.exports.default={__s2plugin:1};",
        );
        let (m, _js, _gd) = read_s2sp(&bytes).expect("valid s2sp");
        assert!(m.permissions.is_empty());
    }

    #[test]
    fn permission_is_default_deny() {
        // PERMISSIONS is host-global and libtest orders tests by NAME, so
        // `permission_allowed_after_allow_list_load` (and any other module's test that loads an
        // allow-list) runs BEFORE this one on the single test thread. Reset at entry — the same
        // shared-global discipline the frame tests' capture buffers use (see .cargo/config.toml).
        *PERMISSIONS.write().expect("permissions lock") = None;
        // With no allow-list loaded, nothing is permitted.
        assert!(!permission_allowed("@demo/gd", "engine:calls"));
    }

    #[test]
    fn permission_allowed_after_allow_list_load() {
        load_permissions_from_str(r#"{"engine:calls":["@demo/gd"]}"#).expect("parses");
        assert!(permission_allowed("@demo/gd", "engine:calls"));
        assert!(!permission_allowed("@other/x", "engine:calls"));
        assert!(!permission_allowed("@demo/gd", "engine:other"));
    }
}
