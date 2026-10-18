//! Startup plugin update from the server's `plugins.json` + `plugins-lock.json`.
//!
//! When `addons/s2script/plugins.json` exists with `autoUpdate` on (the default), the runtime does what `s2s update` does before the first
//! plugin scan: resolve each `dependencies` range through the registry's install plan, download
//! what changed, verify every sha256, swap the files into `plugins/`, and rewrite the lock. The
//! steps and the lockfile bytes are the SDK's (packages/sdk/src/server/{sync,lock}.ts); both are
//! pinned to packages/sdk/test/fixtures/server-lock.
//!
//! Degrade, never block: the work runs on the shared tokio runtime while Load returns at once.
//! The loader holds its FIRST scan until the update finishes or its deadline passes. Every
//! download is staged first and the commit (renames, prune, lock write) happens only after all
//! of them verified, so a failure, a timeout or an offline registry leaves plugins/ exactly as it
//! was and the server loads what it has. Only files the lock owns are ever pruned, and only when
//! they still hold the bytes the lock recorded.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::ser::{SerializeMap, Serializer};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const DEFAULT_REGISTRY: &str = "https://www.s2script.com";
/// The registry's own .s2sp cap; a larger body is not a plugin.
const MAX_S2SP_BYTES: usize = 32 * 1024 * 1024;
const MAX_PLAN_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const LOCKFILE_VERSION: u64 = 3;
const PLUGINS_SUBDIR: &str = "plugins";
const STAGE_SUFFIX: &str = ".s2s-tmp";
/// Where a disabled plugin's file lives: the loader's scan is non-recursive, so it never loads it.
const DISABLED_SUBDIR: &str = "disabled";
/// Not `package.json`: npm must never treat the addon folder as a Node project.
const MANIFEST_FILE: &str = "plugins.json";
const LOCK_FILE: &str = "plugins-lock.json";

// ---------------------------------------------------------------------------------------------
// plugins.json
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ServerManifest {
    pub name: String,
    pub dependencies: BTreeMap<String, String>,
    pub with: BTreeMap<String, Vec<String>>,
    /// Dependencies kept in plugins/disabled/: installed and updated, never loaded.
    pub disabled: BTreeSet<String>,
    pub auto_update: bool,
    pub reviewed_only: bool,
    pub registry: Option<String>,
    pub timeout: Duration,
}

/// The server manifest in `text` (`plugins.json`: npm-style `dependencies` plus settings).
pub(crate) fn parse_manifest(text: &str, fallback_name: &str) -> Result<ServerManifest, String> {
    let j: Value = serde_json::from_str(text).map_err(|e| format!("plugins.json: {e}"))?;
    if !j.is_object() {
        return Err("plugins.json: must be a JSON object".into());
    }
    let s2 = &j;
    let mut dependencies = BTreeMap::new();
    match j.get("dependencies") {
        None => {}
        Some(Value::Object(m)) => {
            for (name, range) in m {
                let r = range.as_str().ok_or_else(|| format!("plugins.json: dependency {name} must be a range string"))?;
                dependencies.insert(name.clone(), r.to_string());
            }
        }
        Some(_) => return Err("plugins.json: \"dependencies\" must be an object".into()),
    }
    let mut with = BTreeMap::new();
    if let Some(w) = s2.get("with") {
        let m = w.as_object().ok_or("plugins.json: \"with\" must be an object")?;
        for (bundle, members) in m {
            let arr = members.as_array().ok_or_else(|| format!("plugins.json: with[{bundle}] must be an array"))?;
            let names = arr
                .iter()
                .map(|v| v.as_str().map(str::to_string).ok_or_else(|| format!("plugins.json: with[{bundle}] must hold names")))
                .collect::<Result<Vec<_>, _>>()?;
            with.insert(bundle.clone(), names);
        }
    }
    let mut disabled = BTreeSet::new();
    if let Some(d) = s2.get("disabled") {
        for v in d.as_array().ok_or("plugins.json: \"disabled\" must be an array of plugin names")? {
            let name = v.as_str().ok_or("plugins.json: \"disabled\" must hold names")?;
            if !dependencies.contains_key(name) {
                return Err(format!("plugins.json: \"disabled\" names {name}, which is not in dependencies"));
            }
            disabled.insert(name.to_string());
        }
    }
    let timeout_secs = s2
        .get("updateTimeoutSeconds")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(5, 600);
    Ok(ServerManifest {
        name: j.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(fallback_name).to_string(),
        dependencies,
        with,
        disabled,
        auto_update: s2.get("autoUpdate").and_then(Value::as_bool) != Some(false),
        reviewed_only: s2.get("reviewedOnly").and_then(Value::as_bool) == Some(true),
        registry: s2.get("registry").and_then(Value::as_str).map(str::to_string),
        timeout: Duration::from_secs(timeout_secs),
    })
}

// ---------------------------------------------------------------------------------------------
// plugins-lock.json
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct LockedPlugin {
    pub name: String,
    pub version: String,
    pub resolved: String,
    pub integrity: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Lock {
    pub name: String,
    pub root_name: String,
    pub dependencies: BTreeMap<String, String>,
    pub with: BTreeMap<String, Vec<String>>,
    pub disabled: BTreeSet<String>,
    /// Install path (`plugins/<file>` or `plugins/disabled/<file>`) → plugin.
    pub plugins: BTreeMap<String, LockedPlugin>,
}

pub(crate) fn parse_lock(text: &str) -> Result<Lock, String> {
    let j: Value = serde_json::from_str(text).map_err(|e| format!("plugins-lock.json: {e}"))?;
    if j.get("lockfileVersion").and_then(Value::as_u64) != Some(LOCKFILE_VERSION) {
        return Err(format!("plugins-lock.json: lockfileVersion must be {LOCKFILE_VERSION}"));
    }
    let packages = j.get("packages").and_then(Value::as_object).ok_or("plugins-lock.json: no \"packages\"")?;
    let mut lock = Lock { name: j.get("name").and_then(Value::as_str).unwrap_or("").to_string(), ..Lock::default() };
    for (key, raw) in packages {
        if key.is_empty() {
            lock.root_name = raw.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            if let Some(d) = raw.get("dependencies").and_then(Value::as_object) {
                for (k, v) in d {
                    lock.dependencies.insert(k.clone(), v.as_str().unwrap_or("").to_string());
                }
            }
            if let Some(w) = raw.get("with").and_then(Value::as_object) {
                for (k, v) in w {
                    let names = v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                    lock.with.insert(k.clone(), names);
                }
            }
            if let Some(d) = raw.get("disabled").and_then(Value::as_array) {
                lock.disabled = d.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            }
            continue;
        }
        let field = |f: &str| raw.get(f).and_then(Value::as_str).map(str::to_string);
        let file = key.strip_prefix("plugins/").map(|f| f.strip_prefix("disabled/").unwrap_or(f));
        match (file, field("name"), field("version"), field("resolved"), field("integrity")) {
            (Some(file), Some(name), Some(version), Some(resolved), Some(integrity)) if safe_filename(file) => {
                lock.plugins.insert(key.clone(), LockedPlugin { name, version, resolved, integrity });
            }
            _ => return Err(format!("plugins-lock.json: malformed entry {key:?}")),
        }
    }
    Ok(lock)
}

struct Packages<'a>(&'a Lock);

impl Serialize for Packages<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Root<'a> {
            name: &'a str,
            dependencies: &'a BTreeMap<String, String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            with: Option<&'a BTreeMap<String, Vec<String>>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            disabled: Option<&'a BTreeSet<String>>,
        }
        let lock = self.0;
        let mut m = s.serialize_map(Some(lock.plugins.len() + 1))?;
        m.serialize_entry(
            "",
            &Root {
                name: &lock.root_name,
                dependencies: &lock.dependencies,
                with: (!lock.with.is_empty()).then_some(&lock.with),
                disabled: (!lock.disabled.is_empty()).then_some(&lock.disabled),
            },
        )?;
        for (k, v) in &lock.plugins {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// The lockfile's exact bytes — identical to the SDK's `serializeLock`.
pub(crate) fn serialize_lock(lock: &Lock) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct File<'a> {
        name: &'a str,
        lockfile_version: u64,
        requires: bool,
        packages: Packages<'a>,
    }
    let file = File { name: &lock.name, lockfile_version: LOCKFILE_VERSION, requires: true, packages: Packages(lock) };
    let mut out = serde_json::to_string_pretty(&file).expect("lock serializes");
    out.push('\n');
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn unbase64(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        acc = acc << 6 | B64.iter().position(|&b| b == c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

pub(crate) fn integrity_from_hex(hex: &str) -> Option<String> {
    Some(format!("sha256-{}", base64(&hex_to_bytes(hex)?)))
}

pub(crate) fn hex_from_integrity(integrity: &str) -> Option<String> {
    let bytes = unbase64(integrity.strip_prefix("sha256-")?)?;
    (bytes.len() == 32).then(|| bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The SDK's SAFE_FILENAME rule plus the loader's own: a bare `*.s2sp` name, nothing else.
fn safe_filename(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains("..")
        && name.ends_with(".s2sp")
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

// ---------------------------------------------------------------------------------------------
// Resolve
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Target {
    pub name: String,
    pub version: String,
    pub filename: String,
    pub url: String,
    pub sha256: String,
    /// Every dependency that pulls this plugin in is disabled, so its file goes to disabled/.
    pub disabled: bool,
}

impl Target {
    /// The file's path under plugins/.
    fn rel(&self) -> String {
        if self.disabled { format!("{DISABLED_SUBDIR}/{}", self.filename) } else { self.filename.clone() }
    }
}

/// GET `url` (with the registry token when `auth`) → (status, body). Injectable for tests.
pub(crate) type Get = Arc<dyn Fn(String, bool) -> Pin<Box<dyn Future<Output = Result<(u16, Vec<u8>), String>> + Send>> + Send + Sync>;

fn plan_url(registry: &str, name: &str, range: &str, with: Option<&Vec<String>>) -> String {
    let enc = |s: &str| {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect::<String>()
    };
    let mut url = format!("{}/api/v1/plan?name={}&range={}", registry.trim_end_matches('/'), enc(name), enc(range));
    if let Some(w) = with.filter(|w| !w.is_empty()) {
        url.push_str("&with=");
        url.push_str(&enc(&w.join(",")));
    }
    url
}

fn error_detail(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("message").or_else(|| v.get("error")).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| String::from_utf8_lossy(&body[..body.len().min(200)]).into_owned())
}

/// Resolve every dependency through the registry's install plan, as `s2s update` does.
pub(crate) async fn resolve_targets(manifest: &ServerManifest, registry: &str, get: &Get) -> Result<Vec<Target>, String> {
    let mut chosen: BTreeMap<String, (Target, String)> = BTreeMap::new();
    for (name, range) in &manifest.dependencies {
        let root_disabled = manifest.disabled.contains(name);
        let (status, body) = get(plan_url(registry, name, range, manifest.with.get(name)), true).await?;
        if status != 200 {
            return Err(format!("plan {name}@{range}: HTTP {status}: {}", error_detail(&body)));
        }
        let plan: Value = serde_json::from_slice(&body).map_err(|e| format!("plan {name}: {e}"))?;
        if let Some(errs) = plan.get("errors").and_then(Value::as_array).filter(|e| !e.is_empty()) {
            let msgs: Vec<_> = errs.iter().filter_map(Value::as_str).collect();
            return Err(format!("plan {name}@{range}: {}", msgs.join("; ")));
        }
        for e in plan.get("install").and_then(Value::as_array).into_iter().flatten() {
            let s = |f: &str| e.get(f).and_then(Value::as_str).map(str::to_string);
            let (Some(n), Some(version), Some(filename), Some(url)) = (s("name"), s("version"), s("filename"), s("url")) else {
                return Err(format!("plan {name}: malformed install entry"));
            };
            let sha256 = s("sha256").ok_or_else(|| format!("refusing {n}@{version}: registry did not provide a sha256"))?;
            if !safe_filename(&filename) {
                return Err(format!("refusing {n}@{version}: unsafe filename {filename:?}"));
            }
            let review = s("reviewState").unwrap_or_default();
            if manifest.reviewed_only && review != "reviewed" {
                return Err(format!("reviewedOnly: refusing unreviewed plugin {n}@{version}"));
            }
            if let Some((prior, _)) = chosen.get_mut(&n) {
                if prior.version != version {
                    return Err(format!("version conflict for {n}: {} vs {version}", prior.version));
                }
                // A plugin is enabled when ANY dependency that needs it is.
                prior.disabled &= root_disabled;
                continue;
            }
            chosen.insert(n.clone(), (Target { name: n, version, filename, url, sha256, disabled: root_disabled }, review));
        }
    }
    let mut out: Vec<Target> = chosen.into_values().map(|(t, _)| t).collect();
    out.sort_by(|a, b| a.filename.cmp(&b.filename));
    for w in out.windows(2) {
        if w[0].filename == w[1].filename {
            return Err(format!("{} and {} resolve to the same file {}", w[0].name, w[1].name, w[0].filename));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Stage / commit
// ---------------------------------------------------------------------------------------------

/// One rename the commit performs: a verified download into place, or an installed file moved
/// between plugins/ and plugins/disabled/ (enable / disable — no download).
struct Rename {
    from: PathBuf,
    to: PathBuf,
    /// "installed …" / "enabled …" / "disabled …"
    what: String,
    /// `from` is a staged temp file (removed on discard), not an installed plugin.
    staged: bool,
}

pub(crate) struct Staged {
    root: PathBuf,
    renames: Vec<Rename>,
    pub lock: Lock,
    pub kept: usize,
}

impl Staged {
    fn discard(&self) {
        for r in self.renames.iter().filter(|r| r.staged) {
            let _ = std::fs::remove_file(&r.from);
        }
    }
}

fn file_sha(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|b| sha256_hex(&b))
}

/// Download and verify every changed target into `<file>.s2s-tmp`. Nothing visible changes.
pub(crate) async fn stage(
    root: &Path,
    manifest: &ServerManifest,
    targets: &[Target],
    get: &Get,
    registry_host: Option<&str>,
    old: Option<&Lock>,
) -> Result<Staged, String> {
    let dir = root.join(PLUGINS_SUBDIR);
    let mut staged = Staged {
        root: root.to_path_buf(),
        renames: Vec::new(),
        lock: Lock {
            name: manifest.name.clone(),
            root_name: manifest.name.clone(),
            dependencies: manifest.dependencies.clone(),
            with: manifest.with.clone(),
            disabled: manifest.disabled.clone(),
            plugins: BTreeMap::new(),
        },
        kept: 0,
    };
    let result: Result<(), String> = async {
        for t in targets {
            let integrity = integrity_from_hex(&t.sha256).ok_or_else(|| format!("{}: bad sha256 {}", t.name, t.sha256))?;
            let key = format!("{PLUGINS_SUBDIR}/{}", t.rel());
            staged.lock.plugins.insert(
                key.clone(),
                LockedPlugin { name: t.name.clone(), version: t.version.clone(), resolved: t.url.clone(), integrity },
            );
            let dest = dir.join(t.rel());
            if file_sha(&dest).as_deref() == Some(t.sha256.as_str()) {
                staged.kept += 1;
                continue;
            }
            // Enabled or disabled since the last lock: move the installed file, no download.
            let moved_from = old.into_iter().flat_map(|l| &l.plugins).find_map(|(k, p)| {
                let from = root.join(k);
                (k != &key && p.name == t.name && file_sha(&from).as_deref() == Some(t.sha256.as_str())).then_some(from)
            });
            if let Some(from) = moved_from {
                let verb = if t.disabled { "disabled" } else { "enabled" };
                staged.renames.push(Rename { from, to: dest, what: format!("{verb} {}@{}", t.name, t.version), staged: false });
                continue;
            }
            // The token goes only to the registry's own host; a redirect to the CDN drops it.
            let auth = registry_host.is_some_and(|h| url_host(&t.url).as_deref() == Some(h));
            let (status, bytes) = get(t.url.clone(), auth).await?;
            if status != 200 {
                return Err(format!("download {}@{}: HTTP {status}: {}", t.name, t.version, error_detail(&bytes)));
            }
            if bytes.len() > MAX_S2SP_BYTES {
                return Err(format!("download {}@{}: larger than {MAX_S2SP_BYTES} bytes", t.name, t.version));
            }
            let got = sha256_hex(&bytes);
            if got != t.sha256 {
                return Err(format!("sha256 mismatch for {}@{}: expected {}, got {got}", t.name, t.version, t.sha256));
            }
            let parent = dest.parent().unwrap_or(&dir).to_path_buf();
            std::fs::create_dir_all(&parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            let tmp = parent.join(format!("{}{STAGE_SUFFIX}", t.filename));
            std::fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
            staged.renames.push(Rename {
                from: tmp,
                to: dest,
                what: format!("installed {}@{} -> {}", t.name, t.version, t.rel()),
                staged: true,
            });
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Ok(staged),
        Err(e) => {
            staged.discard();
            Err(e)
        }
    }
}

/// Swap the staged files in, prune what the old lock owned and nothing wants, write the lock.
pub(crate) fn commit(staged: Staged, old: Option<&Lock>) -> Vec<String> {
    let mut log = Vec::new();
    let mut installed = 0;
    let mut moved = 0;
    for r in &staged.renames {
        if let Some(parent) = r.to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::rename(&r.from, &r.to) {
            Ok(()) => {
                if r.staged {
                    installed += 1;
                } else {
                    moved += 1;
                }
                log.push(format!("[update] {}", r.what));
            }
            Err(e) => log.push(format!("WARN: [update] could not finish \"{}\": {e}", r.what)),
        }
    }
    let mut removed = 0;
    for (key, p) in old.map(|l| &l.plugins).into_iter().flatten() {
        if staged.lock.plugins.contains_key(key) {
            continue;
        }
        let file = staged.root.join(key);
        let Some(have) = file_sha(&file) else { continue };
        if Some(have) == hex_from_integrity(&p.integrity) {
            if std::fs::remove_file(&file).is_ok() {
                removed += 1;
                log.push(format!("[update] removed {}@{} ({key})", p.name, p.version));
            }
        } else {
            log.push(format!("WARN: [update] left {key} in place — it no longer matches the lock, so it was changed by hand"));
        }
    }
    let lock_path = staged.root.join(LOCK_FILE);
    let text = serialize_lock(&staged.lock);
    if std::fs::read_to_string(&lock_path).ok().as_deref() != Some(text.as_str()) {
        let tmp = staged.root.join(format!("{LOCK_FILE}{STAGE_SUFFIX}"));
        if let Err(e) = std::fs::write(&tmp, &text).and_then(|_| std::fs::rename(&tmp, &lock_path)) {
            log.push(format!("WARN: [update] could not write {LOCK_FILE}: {e}"));
        }
    }
    let moved = if moved > 0 { format!(", {moved} enabled/disabled") } else { String::new() };
    log.push(format!("[update] plugins: {installed} installed, {} up to date, {removed} removed{moved}", staged.kept));
    log
}

fn url_host(url: &str) -> Option<String> {
    reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string))
}

// ---------------------------------------------------------------------------------------------
// The startup gate (main thread)
// ---------------------------------------------------------------------------------------------

enum Phase {
    Running,
    Committing,
    Done(Vec<String>),
    Abandoned,
}

struct Gate {
    phase: Arc<Mutex<Phase>>,
    deadline: Instant,
}

static GATE: Mutex<Option<Gate>> = Mutex::new(None);

pub(crate) struct UpdateConfig {
    pub root: PathBuf,
    pub manifest: ServerManifest,
    pub lock: Option<Lock>,
    pub registry: String,
}

/// What `start` will do for this addon root, read on the main thread. None = no update.
pub(crate) fn load_config(root: &Path, env_registry: Option<String>) -> Result<Option<UpdateConfig>, String> {
    let Ok(text) = std::fs::read_to_string(root.join(MANIFEST_FILE)) else { return Ok(None) };
    let fallback = root.file_name().and_then(|n| n.to_str()).unwrap_or("server");
    let manifest = parse_manifest(&text, fallback)?;
    if !manifest.auto_update || manifest.dependencies.is_empty() {
        return Ok(None);
    }
    let lock = match std::fs::read_to_string(root.join(LOCK_FILE)) {
        Ok(t) => Some(parse_lock(&t)?),
        Err(_) => None,
    };
    let registry = env_registry.or_else(|| manifest.registry.clone()).unwrap_or_else(|| DEFAULT_REGISTRY.to_string());
    Ok(Some(UpdateConfig { root: root.to_path_buf(), manifest, lock, registry }))
}

/// The registry token: S2SCRIPT_TOKEN, else `configs/registry.json` `{ "token": "s2s_…" }`.
fn registry_token() -> Option<String> {
    if let Ok(t) = std::env::var("S2SCRIPT_TOKEN") {
        if !t.is_empty() {
            return Some(t);
        }
    }
    let text = crate::v8host::read_engine_config("registry")?;
    let j: Value = serde_json::from_str(&crate::config::strip_line_comments(&text)).ok()?;
    j.get("token").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string)
}

fn http_get(token: Option<String>) -> Get {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().expect("reqwest client");
    Arc::new(move |url: String, auth: bool| {
        let mut rb = client.get(url);
        if let (true, Some(t)) = (auth, token.as_ref()) {
            rb = rb.bearer_auth(t);
        }
        Box::pin(async move {
            let resp = rb.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
            if bytes.len() > MAX_S2SP_BYTES.max(MAX_PLAN_BYTES) {
                return Err(format!("response larger than {} bytes", MAX_S2SP_BYTES));
            }
            Ok((status, bytes.to_vec()))
        })
    })
}

/// The whole update, as one future; `phase` decides at the commit point whether it may proceed.
async fn run(cfg: UpdateConfig, get: Get, phase: Arc<Mutex<Phase>>) {
    let host = url_host(&cfg.registry);
    let outcome = async {
        let targets = resolve_targets(&cfg.manifest, &cfg.registry, &get).await?;
        stage(&cfg.root, &cfg.manifest, &targets, &get, host.as_deref(), cfg.lock.as_ref()).await
    }
    .await;
    let staged = match outcome {
        Ok(s) => s,
        Err(e) => {
            let mut p = phase.lock().unwrap();
            if matches!(*p, Phase::Running) {
                *p = Phase::Done(vec![format!("WARN: [update] plugins not updated: {e} — loading the installed ones")]);
            }
            return;
        }
    };
    {
        let mut p = phase.lock().unwrap();
        if !matches!(*p, Phase::Running) {
            drop(p);
            staged.discard();
            return;
        }
        *p = Phase::Committing;
    }
    let log = commit(staged, cfg.lock.as_ref());
    *phase.lock().unwrap() = Phase::Done(log);
}

/// Begin the startup update for the addon root holding `plugins_dir`. Called on the main thread
/// before the loader starts; returns at once.
pub(crate) fn start(plugins_dir: &Path) {
    let Some(root) = plugins_dir.parent() else { return };
    let cfg = match load_config(root, std::env::var("S2SCRIPT_REGISTRY_URL").ok().filter(|s| !s.is_empty())) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return,
        Err(e) => {
            crate::v8host::log_warn(&format!("WARN: [update] {e} — plugins not updated"));
            return;
        }
    };
    let deadline = Instant::now() + cfg.manifest.timeout;
    crate::v8host::log_warn(&format!(
        "[update] checking {} plugin(s) from {MANIFEST_FILE} against {}",
        cfg.manifest.dependencies.len(),
        cfg.registry
    ));
    let phase = Arc::new(Mutex::new(Phase::Running));
    let get = http_get(registry_token());
    let task_phase = phase.clone();
    if !crate::http::try_spawn(run(cfg, get, task_phase)) {
        crate::v8host::log_warn("WARN: [update] HTTP engine unavailable — plugins not updated");
        return;
    }
    *GATE.lock().unwrap() = Some(Gate { phase, deadline });
}

/// True while the loader must hold its first scan. Logs the update's outcome when it ends.
pub(crate) fn holds_first_scan() -> bool {
    let mut gate = GATE.lock().unwrap();
    let Some(g) = gate.as_ref() else { return false };
    let mut phase = g.phase.lock().unwrap();
    let release = match &mut *phase {
        Phase::Running if Instant::now() < g.deadline => return true,
        Phase::Running => {
            *phase = Phase::Abandoned;
            vec!["WARN: [update] timed out — loading the installed plugins; nothing was changed".to_string()]
        }
        // Renames and one small write: wait for them rather than scan a half-swapped directory.
        Phase::Committing => return true,
        Phase::Done(log) => std::mem::take(log),
        Phase::Abandoned => Vec::new(),
    };
    drop(phase);
    *gate = None;
    drop(gate);
    for line in release {
        crate::v8host::log_warn(&line);
    }
    false
}

// ---------------------------------------------------------------------------------------------
// Console operations (`sm plugins install|update|remove|outdated`)
// ---------------------------------------------------------------------------------------------

/// One console request, as basecommands sends it: `{"op":"install","names":[...],"with":[...]}`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Op {
    /// Add (or re-range) `names` (`name` or `name@range`); `with` = optional bundle members;
    /// `disabled` = install into plugins/disabled/ so they never load until enabled.
    Install { names: Vec<String>, with: Vec<String>, disabled: bool },
    /// Move installed plugins out of / into plugins/disabled/ (and record it in plugins.json).
    Enable { names: Vec<String> },
    Disable { names: Vec<String> },
    /// Move `names` (all when empty) to the newest version their range allows.
    Update { names: Vec<String> },
    /// Drop `names` from plugins.json; their files are pruned.
    Remove { names: Vec<String> },
    /// Report current / wanted / latest for every dependency.
    Outdated,
}

pub(crate) fn parse_op(json: &str) -> Result<Op, String> {
    let j: Value = serde_json::from_str(json).map_err(|e| format!("bad request: {e}"))?;
    let list = |k: &str| -> Vec<String> {
        j.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
    };
    match j.get("op").and_then(Value::as_str) {
        Some("install") => Ok(Op::Install {
            names: list("names"),
            with: list("with"),
            disabled: j.get("disabled").and_then(Value::as_bool) == Some(true),
        }),
        Some("enable") => Ok(Op::Enable { names: list("names") }),
        Some("disable") => Ok(Op::Disable { names: list("names") }),
        Some("update") => Ok(Op::Update { names: list("names") }),
        Some("remove") => Ok(Op::Remove { names: list("names") }),
        Some("outdated") => Ok(Op::Outdated),
        other => Err(format!("unknown plugins operation {other:?}")),
    }
}

/// `name@range` → (name, Some(range)); a scoped name's leading @ is not a separator.
fn split_spec(spec: &str) -> (String, Option<String>) {
    match spec.rfind('@') {
        Some(at) if at > 0 => (spec[..at].to_string(), Some(spec[at + 1..].to_string()).filter(|r| !r.is_empty())),
        _ => (spec.to_string(), None),
    }
}

/// A plain `x.y.z` (optionally with a prerelease) gets npm's caret; anything else is kept as is.
fn caret(version: &str) -> String {
    let core = version.split(['-', '+']).next().unwrap_or("");
    if core.split('.').count() == 3 && core.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
        format!("^{version}")
    } else {
        version.to_string()
    }
}

/// The ranges to resolve: names in `unpinned` at their range, every other dependency pinned to the
/// version the lock has for it — as long as its range has not changed since that lock was written.
fn pinned(manifest: &ServerManifest, lock: Option<&Lock>, unpinned: &dyn Fn(&str) -> bool) -> ServerManifest {
    let mut m = manifest.clone();
    for (name, range) in m.dependencies.iter_mut() {
        if unpinned(name) {
            continue;
        }
        let Some(lock) = lock else { continue };
        if lock.dependencies.get(name) != Some(range) {
            continue;
        }
        if let Some(p) = lock.plugins.values().find(|p| &p.name == name) {
            *range = p.version.clone();
        }
    }
    m
}

/// Rewrite plugins.json with `manifest`'s dependencies and `with`, keeping every other field.
fn write_manifest(root: &Path, manifest: &ServerManifest) -> Result<(), String> {
    let path = root.join(MANIFEST_FILE);
    let mut raw: serde_json::Map<String, Value> = match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{MANIFEST_FILE}: {e}"))?,
        Err(_) => serde_json::Map::new(),
    };
    raw.insert("name".into(), Value::String(manifest.name.clone()));
    raw.insert("dependencies".into(), serde_json::to_value(&manifest.dependencies).unwrap());
    if manifest.with.is_empty() {
        raw.remove("with");
    } else {
        raw.insert("with".into(), serde_json::to_value(&manifest.with).unwrap());
    }
    if manifest.disabled.is_empty() {
        raw.remove("disabled");
    } else {
        raw.insert("disabled".into(), serde_json::to_value(&manifest.disabled).unwrap());
    }
    raw.entry("autoUpdate").or_insert(Value::Bool(true));
    // Known keys first, in the order the docs show them; anything else after, as found.
    let order = ["name", "dependencies", "with", "disabled", "autoUpdate", "reviewedOnly", "updateTimeoutSeconds", "registry"];
    let mut out = String::from("{\n");
    let mut entries: Vec<(&String, &Value)> = order.iter().filter_map(|k| raw.get_key_value(*k)).collect();
    entries.extend(raw.iter().filter(|(k, _)| !order.contains(&k.as_str())));
    for (i, (k, v)) in entries.iter().enumerate() {
        let value = serde_json::to_string_pretty(v).unwrap().replace('\n', "\n  ");
        out.push_str(&format!("  {}: {}{}\n", serde_json::to_string(k).unwrap(), value, if i + 1 < entries.len() { "," } else { "" }));
    }
    out.push_str("}\n");
    let tmp = root.join(format!("{MANIFEST_FILE}{STAGE_SUFFIX}"));
    std::fs::write(&tmp, out).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("{MANIFEST_FILE}: {e}"))
}

async fn resolve_version(registry: &str, name: &str, range: &str, get: &Get) -> Result<String, String> {
    let url = plan_url(registry, name, range, None).replace("/api/v1/plan?", "/api/v1/resolve?");
    let (status, body) = get(url, true).await?;
    if status != 200 {
        return Err(error_detail(&body));
    }
    let v: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    v.get("version").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "no version".into())
}

/// Run one console operation against the server at `root`, returning the lines to print.
pub(crate) async fn run_op(root: &Path, op: Op, env_registry: Option<String>, get: &Get) -> Result<Vec<String>, String> {
    let manifest_path = root.join(MANIFEST_FILE);
    let fallback = root.file_name().and_then(|n| n.to_str()).unwrap_or("server");
    let mut manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => parse_manifest(&text, fallback)?,
        // Like `npm install`, the first install creates the manifest.
        Err(_) if matches!(op, Op::Install { .. }) => parse_manifest("{}", "s2script-server")?,
        Err(_) => return Err(format!("no {MANIFEST_FILE} in {} — add a plugin with `sm plugins install <name>` first", root.display())),
    };
    let lock = match std::fs::read_to_string(root.join(LOCK_FILE)) {
        Ok(t) => Some(parse_lock(&t)?),
        Err(_) => None,
    };
    let registry = env_registry.or_else(|| manifest.registry.clone()).unwrap_or_else(|| DEFAULT_REGISTRY.to_string());

    if op == Op::Outdated {
        let mut lines = vec![format!("{:<32} {:<12} {:<12} {}", "Plugin", "Current", "Wanted", "Latest")];
        for (name, range) in &manifest.dependencies {
            let current = lock.as_ref().and_then(|l| l.plugins.values().find(|p| &p.name == name)).map(|p| p.version.clone()).unwrap_or_else(|| "missing".into());
            let wanted = resolve_version(&registry, name, range, get).await.unwrap_or_else(|e| format!("({e})"));
            let latest = resolve_version(&registry, name, "*", get).await.unwrap_or_else(|_| "-".into());
            let mark = if current != wanted || wanted != latest { "" } else { "  (up to date)" };
            lines.push(format!("{name:<32} {current:<12} {wanted:<12} {latest}{mark}"));
        }
        if manifest.dependencies.is_empty() {
            lines.push(format!("no plugins in {MANIFEST_FILE}"));
        }
        return Ok(lines);
    }

    let mut bare = Vec::new();
    let changed: Vec<String> = match &op {
        Op::Install { names, with, disabled } => {
            if names.is_empty() {
                return Err("usage: sm plugins install <name>[@range] [...]".into());
            }
            if !with.is_empty() && names.len() != 1 {
                return Err("--with applies to exactly one bundle".into());
            }
            let mut out = Vec::new();
            for spec in names {
                let (name, range) = split_spec(spec);
                if range.is_none() {
                    bare.push(name.clone());
                }
                manifest.dependencies.insert(name.clone(), range.unwrap_or_else(|| "*".into()));
                if !with.is_empty() {
                    manifest.with.insert(name.clone(), with.clone());
                }
                // --disabled puts it in plugins/disabled/; a plain install keeps whatever it was.
                if *disabled {
                    manifest.disabled.insert(name.clone());
                }
                out.push(name);
            }
            out
        }
        Op::Update { names } => {
            for n in names {
                if !manifest.dependencies.contains_key(n) {
                    return Err(format!("{n} is not in {MANIFEST_FILE}"));
                }
            }
            if names.is_empty() { manifest.dependencies.keys().cloned().collect() } else { names.clone() }
        }
        Op::Enable { names } | Op::Disable { names } => {
            if names.is_empty() {
                return Err(format!("usage: sm plugins {} <name> [...]", if matches!(op, Op::Enable { .. }) { "enable" } else { "disable" }));
            }
            for n in names {
                if !manifest.dependencies.contains_key(n) {
                    return Err(format!("{n} is not in {MANIFEST_FILE}"));
                }
                if matches!(op, Op::Enable { .. }) {
                    manifest.disabled.remove(n);
                } else {
                    manifest.disabled.insert(n.clone());
                }
            }
            // Everything stays at its locked version; only the folders change.
            Vec::new()
        }
        Op::Remove { names } => {
            if names.is_empty() {
                return Err("usage: sm plugins remove <name> [...]".into());
            }
            for n in names {
                if manifest.dependencies.remove(n).is_none() {
                    return Err(format!("{n} is not in {MANIFEST_FILE}"));
                }
                manifest.with.remove(n);
                manifest.disabled.remove(n);
            }
            Vec::new()
        }
        Op::Outdated => unreachable!(),
    };

    let resolve_with = pinned(&manifest, lock.as_ref(), &|n| changed.iter().any(|c| c == n));
    let targets = resolve_targets(&resolve_with, &registry, get).await?;
    // A bare `install name` records ^<the version it got>, as npm does — before staging, so the
    // lock's root `dependencies` carries the same range plugins.json will.
    for name in &bare {
        if let Some(t) = targets.iter().find(|t| &t.name == name) {
            manifest.dependencies.insert(name.clone(), caret(&t.version));
        }
    }
    let host = url_host(&registry);
    let staged = stage(root, &manifest, &targets, get, host.as_deref(), lock.as_ref()).await?;
    let mut log = commit(staged, lock.as_ref());
    if !matches!(op, Op::Update { .. }) {
        write_manifest(root, &manifest)?;
    }
    for l in log.iter_mut() {
        *l = l.trim_start_matches("WARN: ").trim_start_matches("[update] ").to_string();
    }
    Ok(log)
}

static OP_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static OP_RESULT: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// Claim the single console-operation slot. Two at once would race on plugins/ and the lock, and
/// the startup update owns them until it is done.
pub(crate) fn try_begin_op() -> Result<(), String> {
    if GATE.lock().unwrap().is_some() {
        return Err("the startup plugin update is still running — try again in a moment".into());
    }
    if OP_BUSY.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return Err("another `sm plugins` registry operation is still running".into());
    }
    *OP_RESULT.lock().unwrap() = None;
    Ok(())
}

/// The blocking body of a console operation, for a threadpool job. Releases the slot when done.
pub(crate) fn run_op_blocking(root: PathBuf, op: Op, token: Option<String>) -> Result<(), String> {
    let env_registry = std::env::var("S2SCRIPT_REGISTRY_URL").ok().filter(|s| !s.is_empty());
    let get = http_get(token);
    let out = crate::http::block_on(async move { run_op(&root, op, env_registry, &get).await })
        .unwrap_or_else(|| Err("HTTP engine unavailable".into()));
    OP_BUSY.store(false, std::sync::atomic::Ordering::Release);
    let lines = out?;
    *OP_RESULT.lock().unwrap() = Some(lines);
    Ok(())
}

/// Release the slot for an operation that never started (the threadpool refused the job).
pub(crate) fn abort_op() {
    OP_BUSY.store(false, std::sync::atomic::Ordering::Release);
}

/// The lines the last finished operation produced (taken once).
pub(crate) fn take_op_result() -> Vec<String> {
    OP_RESULT.lock().unwrap().take().unwrap_or_default()
}

/// The registry token for a console operation (main thread: reads configs/registry.json).
pub(crate) fn console_token() -> Option<String> {
    registry_token()
}

/// Lines a background `sm plugins …` operation produced, waiting for the main thread to print
/// them (the console logger is only reachable there). Drained by `take_console_report`.
static CONSOLE_REPORT: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Run a registry operation for the `sm` console command on its own thread. Its report (or the
/// reason it failed) is queued for `take_console_report`; the slot is released when it ends.
pub(crate) fn spawn_console_op(root: PathBuf, op: Op, token: Option<String>, label: &str) -> Result<(), String> {
    try_begin_op()?;
    let label = label.to_string();
    let spawned = std::thread::Builder::new()
        .name("s2s-sm-plugins".into())
        .spawn(move || {
            let lines = match run_op_blocking(root, op, token) {
                Ok(()) => take_op_result(),
                Err(e) => vec![format!("sm plugins {label} failed: {e}")],
            };
            let mut report = CONSOLE_REPORT.lock().unwrap();
            report.extend(lines);
            report.push(format!("sm plugins {label}: done"));
        });
    if let Err(e) = spawned {
        abort_op();
        return Err(format!("could not start the operation: {e}"));
    }
    Ok(())
}

/// The queued report lines of finished `sm` console operations (taken once, main thread).
pub(crate) fn take_console_report() -> Vec<String> {
    std::mem::take(&mut *CONSOLE_REPORT.lock().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const FIXTURE: &str = include_str!("../../packages/sdk/test/fixtures/server-lock/plugins-lock.json");

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("s2s-update-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join(PLUGINS_SUBDIR)).unwrap();
        d
    }

    /// A fake registry: name → (published versions newest last), serving plans and downloads.
    struct Fake {
        published: HashMap<&'static str, Vec<&'static str>>,
        bad_download: Option<&'static str>,
    }

    fn body(name: &str, version: &str) -> Vec<u8> {
        format!("s2sp:{name}@{version}").into_bytes()
    }
    fn file_of(name: &str) -> String {
        format!("{}.s2sp", name.replace(['@', '/'], "_"))
    }

    fn fake_get(fake: Fake, calls: Arc<Mutex<Vec<(String, bool)>>>) -> Get {
        let fake = Arc::new(fake);
        Arc::new(move |url: String, auth: bool| {
            calls.lock().unwrap().push((url.clone(), auth));
            let fake = fake.clone();
            Box::pin(async move {
                let q = |k: &str| {
                    url.split(['?', '&']).find_map(|p| p.strip_prefix(&format!("{k}="))).map(|v| v.replace("%40", "@").replace("%2F", "/").replace("%5E", "^").replace("%2A", "*"))
                };
                if url.contains("/api/v1/plan") {
                    let name = q("name").unwrap();
                    let range = q("range").unwrap();
                    let Some(vs) = fake.published.get(name.as_str()) else {
                        return Ok((404, br#"{"message":"package not found"}"#.to_vec()));
                    };
                    // Only the shapes the tests use: `*` is the newest, ^X.Y.Z means "same major", and
                    // a bare X.Y.Z is that exact version.
                    let major = range.trim_start_matches('^').split('.').next().unwrap().to_string();
                    let v = if range == "*" {
                        vs.last()
                    } else if !range.starts_with('^') {
                        vs.iter().find(|v| **v == range)
                    } else {
                        vs.iter().rev().find(|v| v.split('.').next().unwrap() == major)
                    };
                    let Some(v) = v else {
                        return Ok((200, serde_json::to_vec(&serde_json::json!({"install":[],"errors":[format!("no version of {name} satisfies {range}")]})).unwrap()));
                    };
                    let plan = serde_json::json!({ "install": [{
                        "name": name, "version": v, "filename": file_of(&name), "reviewState": "reviewed",
                        "url": format!("https://reg.test/api/v1/download/s2sp?name={name}&version={v}"),
                        "sha256": sha256_hex(&body(&name, v)),
                    }], "errors": [], "warnings": [] });
                    return Ok((200, serde_json::to_vec(&plan).unwrap()));
                }
                let name = q("name").unwrap();
                let version = q("version").unwrap();
                if fake.bad_download == Some(name.as_str()) {
                    return Ok((200, b"tampered".to_vec()));
                }
                Ok((200, body(&name, &version)))
            })
        })
    }

    fn manifest(deps: &[(&str, &str)]) -> ServerManifest {
        ServerManifest {
            name: "test-server".into(),
            dependencies: deps.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            with: BTreeMap::new(),
            disabled: BTreeSet::new(),
            auto_update: true,
            reviewed_only: false,
            registry: None,
            timeout: Duration::from_secs(60),
        }
    }

    fn update(root: &Path, m: &ServerManifest, fake: Fake) -> (Vec<String>, Vec<(String, bool)>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let get = fake_get(fake, calls.clone());
        let lock = std::fs::read_to_string(root.join(LOCK_FILE)).ok().map(|t| parse_lock(&t).unwrap());
        let cfg = UpdateConfig { root: root.to_path_buf(), manifest: m.clone(), lock, registry: "https://reg.test".into() };
        let phase = Arc::new(Mutex::new(Phase::Running));
        rt().block_on(run(cfg, get, phase.clone()));
        let log = match std::mem::replace(&mut *phase.lock().unwrap(), Phase::Abandoned) {
            Phase::Done(l) => l,
            _ => panic!("update did not finish"),
        };
        let calls = calls.lock().unwrap().clone();
        (log, calls)
    }

    fn locked_versions(root: &Path) -> BTreeMap<String, String> {
        let lock = parse_lock(&std::fs::read_to_string(root.join(LOCK_FILE)).unwrap()).unwrap();
        lock.plugins.into_values().map(|p| (p.name, p.version)).collect()
    }

    #[test]
    fn lock_round_trips_the_shared_fixture_byte_for_byte() {
        assert_eq!(serialize_lock(&parse_lock(FIXTURE).unwrap()), FIXTURE);
    }

    #[test]
    fn integrity_matches_the_sdk_sri_form() {
        let hex = sha256_hex(b"x");
        let sri = integrity_from_hex(&hex).unwrap();
        // node -e "crypto.createHash('sha256').update('x').digest('base64')"
        assert_eq!(sri, "sha256-LXEWQrcmsEQBYnyp+6wy9chTD7GQPMTbAiWHF5IaSIE=");
        assert_eq!(hex_from_integrity(&sri).as_deref(), Some(hex.as_str()));
        assert_eq!(hex_from_integrity("sha512-AAAA"), None);
        for n in 0..8 {
            let data: Vec<u8> = (0..n).collect();
            assert_eq!(unbase64(&base64(&data)).unwrap(), data);
        }
    }

    #[test]
    fn manifest_reads_flat_settings_and_honours_auto_update_off() {
        let m = parse_manifest(r#"{"name":"p","dependencies":{"a":"^1"},"with":{"a":["b"]},"reviewedOnly":true}"#, "x").unwrap();
        assert_eq!(m.name, "p");
        assert!(m.auto_update && m.reviewed_only);
        assert_eq!(m.with["a"], ["b"]);
        let m = parse_manifest(r#"{"dependencies":{"a":"^1"},"autoUpdate":false}"#, "fallback").unwrap();
        assert_eq!(m.name, "fallback");
        assert!(!m.auto_update);
        assert!(parse_manifest("not json", "x").is_err());
    }

    #[test]
    fn updates_within_range_writes_the_lock_and_prunes_what_it_owned() {
        let root = tmpdir("range");
        let m1 = manifest(&[("@edge/rtv", "^1.0.0"), ("@edge/old", "^1.0.0")]);
        let (_, _) = update(&root, &m1, Fake { published: HashMap::from([("@edge/rtv", vec!["1.0.0"]), ("@edge/old", vec!["1.0.0"])]), bad_download: None });
        std::fs::write(root.join("plugins/handmade.s2sp"), b"operator's own").unwrap();
        let m2 = manifest(&[("@edge/rtv", "^1.0.0")]);
        let (log, _) = update(&root, &m2, Fake { published: HashMap::from([("@edge/rtv", vec!["1.0.0", "1.4.0", "2.0.0"])]), bad_download: None });
        assert_eq!(locked_versions(&root), BTreeMap::from([("@edge/rtv".to_string(), "1.4.0".to_string())]));
        let mut files: Vec<_> = std::fs::read_dir(root.join("plugins")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        files.sort();
        assert_eq!(files, ["_edge_rtv.s2sp", "handmade.s2sp"]);
        assert_eq!(std::fs::read(root.join("plugins/_edge_rtv.s2sp")).unwrap(), body("@edge/rtv", "1.4.0"));
        assert!(log.last().unwrap().contains("1 installed, 0 up to date, 1 removed"), "{log:?}");
    }

    #[test]
    fn a_bad_download_changes_nothing() {
        let root = tmpdir("bad");
        let m = manifest(&[("@edge/a", "^1.0.0"), ("@edge/b", "^1.0.0")]);
        update(&root, &m, Fake { published: HashMap::from([("@edge/a", vec!["1.0.0"]), ("@edge/b", vec!["1.0.0"])]), bad_download: None });
        let before = std::fs::read_to_string(root.join(LOCK_FILE)).unwrap();
        let (log, _) = update(&root, &m, Fake { published: HashMap::from([("@edge/a", vec!["1.0.0", "1.1.0"]), ("@edge/b", vec!["1.0.0", "1.1.0"])]), bad_download: Some("@edge/b") });
        assert!(log[0].contains("sha256 mismatch for @edge/b@1.1.0"), "{log:?}");
        assert_eq!(std::fs::read_to_string(root.join(LOCK_FILE)).unwrap(), before);
        assert_eq!(std::fs::read(root.join("plugins/_edge_a.s2sp")).unwrap(), body("@edge/a", "1.0.0"));
        let mut files: Vec<_> = std::fs::read_dir(root.join("plugins")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        files.sort();
        assert_eq!(files, ["_edge_a.s2sp", "_edge_b.s2sp"], "no staged leftovers");
    }

    #[test]
    fn an_unreachable_registry_leaves_plugins_alone() {
        let root = tmpdir("offline");
        std::fs::write(root.join("plugins/keep.s2sp"), b"x").unwrap();
        let cfg = UpdateConfig { root: root.clone(), manifest: manifest(&[("@edge/rtv", "^1.0.0")]), lock: None, registry: "https://reg.test".into() };
        let get: Get = Arc::new(|_, _| Box::pin(async { Err("connection refused".to_string()) }));
        let phase = Arc::new(Mutex::new(Phase::Running));
        rt().block_on(run(cfg, get, phase.clone()));
        let Phase::Done(log) = &*phase.lock().unwrap() else { panic!() };
        assert!(log[0].contains("plugins not updated: connection refused"));
        assert!(!root.join(LOCK_FILE).exists());
        assert!(root.join("plugins/keep.s2sp").exists());
    }

    #[test]
    fn the_token_goes_to_the_registry_host_only() {
        let root = tmpdir("auth");
        let m = manifest(&[("@edge/rtv", "^1.0.0")]);
        let (_, calls) = update(&root, &m, Fake { published: HashMap::from([("@edge/rtv", vec!["1.0.0"])]), bad_download: None });
        assert!(calls.iter().all(|(_, auth)| *auth), "plan and same-host download carry the token: {calls:?}");
        let get: Get = Arc::new(|_, auth| Box::pin(async move { if auth { Err("token leaked".into()) } else { Ok((200, body("@edge/x", "1.0.0"))) } }));
        let t = Target {
            name: "@edge/x".into(),
            version: "1.0.0".into(),
            filename: "_edge_x.s2sp".into(),
            url: "https://cdn.other.test/x.s2sp".into(),
            sha256: sha256_hex(&body("@edge/x", "1.0.0")),
            disabled: false,
        };
        let staged = rt().block_on(stage(&root, &m, &[t], &get, Some("reg.test"), None)).unwrap();
        staged.discard();
    }

    #[test]
    fn unsafe_filenames_and_reviewed_only_are_refused() {
        assert!(!safe_filename("../x.s2sp"));
        assert!(!safe_filename("a/b.s2sp"));
        assert!(!safe_filename("x.js"));
        assert!(safe_filename("_edge_rtv.s2sp"));
        let mut m = manifest(&[("@edge/rtv", "^1.0.0")]);
        m.reviewed_only = true;
        let get: Get = Arc::new(|_, _| {
            Box::pin(async {
                Ok((200, br#"{"install":[{"name":"@edge/rtv","version":"1.0.0","filename":"_edge_rtv.s2sp","url":"u","sha256":"00","reviewState":"unreviewed"}],"errors":[]}"#.to_vec()))
            })
        });
        let err = rt().block_on(resolve_targets(&m, "https://reg.test", &get)).unwrap_err();
        assert!(err.contains("reviewedOnly: refusing unreviewed plugin @edge/rtv@1.0.0"), "{err}");
    }

    #[test]
    fn gate_holds_until_done_and_releases_on_deadline() {
        let phase = Arc::new(Mutex::new(Phase::Running));
        *GATE.lock().unwrap() = Some(Gate { phase: phase.clone(), deadline: Instant::now() + Duration::from_secs(60) });
        assert!(holds_first_scan());
        *phase.lock().unwrap() = Phase::Committing;
        assert!(holds_first_scan());
        *phase.lock().unwrap() = Phase::Done(vec![]);
        assert!(!holds_first_scan());
        assert!(!holds_first_scan(), "released for good");

        let phase = Arc::new(Mutex::new(Phase::Running));
        *GATE.lock().unwrap() = Some(Gate { phase: phase.clone(), deadline: Instant::now() });
        assert!(!holds_first_scan());
        assert!(matches!(*phase.lock().unwrap(), Phase::Abandoned), "a late task must not commit");
    }

    #[test]
    fn a_task_finishing_after_the_deadline_commits_nothing() {
        let root = tmpdir("late");
        let cfg = UpdateConfig { root: root.clone(), manifest: manifest(&[("@edge/rtv", "^1.0.0")]), lock: None, registry: "https://reg.test".into() };
        let get = fake_get(Fake { published: HashMap::from([("@edge/rtv", vec!["1.0.0"])]), bad_download: None }, Arc::new(Mutex::new(Vec::new())));
        let phase = Arc::new(Mutex::new(Phase::Abandoned));
        rt().block_on(run(cfg, get, phase));
        let files: Vec<_> = std::fs::read_dir(root.join("plugins")).unwrap().collect();
        assert!(files.is_empty(), "staged file discarded, nothing installed");
        assert!(!root.join(LOCK_FILE).exists());
    }

    /// Live check against a running registry: S2S_E2E_ROOT=<addon root> S2S_E2E_REGISTRY=<url>
    /// [S2SCRIPT_TOKEN=…] cargo test -p s2script-core server_update::tests::live -- --ignored
    #[test]
    #[ignore]
    fn live_update_against_a_registry() {
        let root = PathBuf::from(std::env::var("S2S_E2E_ROOT").expect("S2S_E2E_ROOT"));
        let cfg = load_config(&root, std::env::var("S2S_E2E_REGISTRY").ok()).unwrap().expect("a plugins.json with dependencies");
        let phase = Arc::new(Mutex::new(Phase::Running));
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(run(cfg, http_get(std::env::var("S2SCRIPT_TOKEN").ok()), phase.clone()));
        let Phase::Done(log) = &*phase.lock().unwrap() else { panic!("not done") };
        for line in log {
            println!("{line}");
        }
    }

    fn fake_with_resolve(published: HashMap<&'static str, Vec<&'static str>>) -> Get {
        let plan = fake_get(Fake { published: published.clone(), bad_download: None }, Arc::new(Mutex::new(Vec::new())));
        Arc::new(move |url: String, auth: bool| {
            if !url.contains("/api/v1/resolve") {
                return plan(url, auth);
            }
            let published = published.clone();
            Box::pin(async move {
                let q = |k: &str| url.split(['?', '&']).find_map(|p| p.strip_prefix(&format!("{k}="))).map(|v| v.replace("%40", "@").replace("%2F", "/").replace("%5E", "^").replace("%2A", "*"));
                let (name, range) = (q("name").unwrap(), q("range").unwrap());
                let vs = &published[name.as_str()];
                let v = if range == "*" {
                    vs.last()
                } else {
                    let major = range.trim_start_matches('^').split('.').next().unwrap().to_string();
                    vs.iter().rev().find(|v| v.split('.').next().unwrap() == major)
                };
                Ok((200, serde_json::to_vec(&serde_json::json!({ "version": v })).unwrap()))
            })
        })
    }

    fn op(root: &Path, op: Op, get: &Get) -> Result<Vec<String>, String> {
        rt().block_on(run_op(root, op, Some("https://reg.test".into()), get))
    }

    fn deps(root: &Path) -> BTreeMap<String, String> {
        parse_manifest(&std::fs::read_to_string(root.join(MANIFEST_FILE)).unwrap(), "x").unwrap().dependencies
    }

    #[test]
    fn console_install_creates_plugins_json_and_records_a_caret_range() {
        let root = tmpdir("c-install");
        let get = fake_with_resolve(HashMap::from([("@edge/rtv", vec!["1.0.0", "1.2.0"])]));
        let lines = op(&root, Op::Install { names: vec!["@edge/rtv".into()], with: vec![], disabled: false }, &get).unwrap();
        assert!(lines.iter().any(|l| l == "installed @edge/rtv@1.2.0 -> _edge_rtv.s2sp"), "{lines:?}");
        assert_eq!(deps(&root), BTreeMap::from([("@edge/rtv".into(), "^1.2.0".into())]));
        let lock = parse_lock(&std::fs::read_to_string(root.join(LOCK_FILE)).unwrap()).unwrap();
        assert_eq!(lock.dependencies, deps(&root), "the lock records the same range plugins.json does");
        assert_eq!(std::fs::read(root.join("plugins/_edge_rtv.s2sp")).unwrap(), body("@edge/rtv", "1.2.0"));
        let text = std::fs::read_to_string(root.join(MANIFEST_FILE)).unwrap();
        assert!(text.starts_with("{\n  \"name\": "), "name first, two-space indent:\n{text}");
        serde_json::from_str::<Value>(&text).expect("valid JSON");
    }

    #[test]
    fn console_install_keeps_the_other_plugins_at_their_locked_versions() {
        let root = tmpdir("c-pin");
        op(&root, Op::Install { names: vec!["@edge/rtv@^1.0.0".into()], with: vec![], disabled: false }, &fake_with_resolve(HashMap::from([("@edge/rtv", vec!["1.0.0"])]))).unwrap();
        let later = fake_with_resolve(HashMap::from([("@edge/rtv", vec!["1.0.0", "1.5.0"]), ("@edge/admin", vec!["3.0.0"])]));
        op(&root, Op::Install { names: vec!["@edge/admin".into()], with: vec![], disabled: false }, &later).unwrap();
        assert_eq!(locked_versions(&root), BTreeMap::from([("@edge/admin".into(), "3.0.0".into()), ("@edge/rtv".into(), "1.0.0".into())]));
        // update then moves rtv within its range.
        let lines = op(&root, Op::Update { names: vec![] }, &later).unwrap();
        assert!(lines.iter().any(|l| l.contains("installed @edge/rtv@1.5.0")), "{lines:?}");
        assert_eq!(deps(&root)["@edge/rtv"], "^1.0.0", "update never edits plugins.json");
    }

    #[test]
    fn console_remove_drops_the_dependency_and_prunes_its_file() {
        let root = tmpdir("c-remove");
        let get = fake_with_resolve(HashMap::from([("@edge/rtv", vec!["1.0.0"]), ("@edge/admin", vec!["1.0.0"])]));
        op(&root, Op::Install { names: vec!["@edge/rtv".into(), "@edge/admin".into()], with: vec![], disabled: false }, &get).unwrap();
        let lines = op(&root, Op::Remove { names: vec!["@edge/admin".into()] }, &get).unwrap();
        assert!(lines.iter().any(|l| l.starts_with("removed @edge/admin@1.0.0")), "{lines:?}");
        assert!(!root.join("plugins/_edge_admin.s2sp").exists());
        assert_eq!(deps(&root).keys().collect::<Vec<_>>(), ["@edge/rtv"]);
        assert!(op(&root, Op::Remove { names: vec!["@edge/nope".into()] }, &get).unwrap_err().contains("not in plugins.json"));
    }

    #[test]
    fn console_outdated_reports_current_wanted_latest() {
        let root = tmpdir("c-outdated");
        op(&root, Op::Install { names: vec!["@edge/rtv@^1.0.0".into()], with: vec![], disabled: false }, &fake_with_resolve(HashMap::from([("@edge/rtv", vec!["1.0.0"])]))).unwrap();
        let lines = op(&root, Op::Outdated, &fake_with_resolve(HashMap::from([("@edge/rtv", vec!["1.0.0", "1.3.0", "2.0.0"])]))).unwrap();
        assert!(lines[1].split_whitespace().eq(["@edge/rtv", "1.0.0", "1.3.0", "2.0.0"]), "{lines:?}");
        assert!(op(&tmpdir("c-none"), Op::Outdated, &fake_with_resolve(HashMap::new())).unwrap_err().contains("no plugins.json"));
    }

    fn ls(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).map(|r| r.map(|e| e.unwrap().file_name().into_string().unwrap()).collect()).unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn install_disabled_lands_in_the_disabled_folder_and_enable_moves_it_without_downloading() {
        let root = tmpdir("c-disabled");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let plain = fake_get(Fake { published: HashMap::from([("@edge/rtv", vec!["1.0.0"])]), bad_download: None }, calls.clone());
        let get: Get = {
            let resolve = fake_with_resolve(HashMap::from([("@edge/rtv", vec!["1.0.0"])]));
            Arc::new(move |url: String, auth: bool| if url.contains("/resolve") { resolve(url, auth) } else { plain(url, auth) })
        };
        let lines = op(&root, Op::Install { names: vec!["@edge/rtv".into()], with: vec![], disabled: true }, &get).unwrap();
        assert!(lines.iter().any(|l| l == "installed @edge/rtv@1.0.0 -> disabled/_edge_rtv.s2sp"), "{lines:?}");
        assert_eq!(ls(&root.join("plugins")), ["disabled"]);
        assert_eq!(ls(&root.join("plugins/disabled")), ["_edge_rtv.s2sp"]);
        let m = parse_manifest(&std::fs::read_to_string(root.join(MANIFEST_FILE)).unwrap(), "x").unwrap();
        assert!(m.disabled.contains("@edge/rtv"));
        let lock = parse_lock(&std::fs::read_to_string(root.join(LOCK_FILE)).unwrap()).unwrap();
        assert!(lock.plugins.contains_key("plugins/disabled/_edge_rtv.s2sp"));
        assert!(lock.disabled.contains("@edge/rtv"));

        let downloads = |c: &Arc<Mutex<Vec<(String, bool)>>>| c.lock().unwrap().iter().filter(|(u, _)| u.contains("/download/")).count();
        let before = downloads(&calls);
        let lines = op(&root, Op::Enable { names: vec!["@edge/rtv".into()] }, &get).unwrap();
        assert!(lines.iter().any(|l| l == "enabled @edge/rtv@1.0.0"), "{lines:?}");
        assert_eq!(downloads(&calls), before, "enable moves the installed file; it never downloads");
        assert_eq!(ls(&root.join("plugins")), ["_edge_rtv.s2sp", "disabled"]);
        assert!(ls(&root.join("plugins/disabled")).is_empty());

        let lines = op(&root, Op::Disable { names: vec!["@edge/rtv".into()] }, &get).unwrap();
        assert!(lines.iter().any(|l| l == "disabled @edge/rtv@1.0.0"), "{lines:?}");
        assert_eq!(ls(&root.join("plugins/disabled")), ["_edge_rtv.s2sp"]);
        assert!(op(&root, Op::Enable { names: vec!["@edge/nope".into()] }, &get).unwrap_err().contains("not in plugins.json"));
    }

    #[test]
    fn a_dependency_stays_enabled_while_any_enabled_plugin_needs_it() {
        let mut m = manifest(&[("@edge/a", "^1.0.0"), ("@edge/b", "^1.0.0")]);
        m.disabled.insert("@edge/a".into());
        // Both pull in @edge/core; only @edge/a is disabled.
        let get: Get = Arc::new(|url: String, _| {
            Box::pin(async move {
                let root = if url.contains("name=%40edge%2Fa") { "@edge/a" } else { "@edge/b" };
                let entry = |n: &str| serde_json::json!({"name": n, "version": "1.0.0", "filename": file_of(n), "url": "u", "sha256": sha256_hex(&body(n, "1.0.0")), "reviewState": "reviewed"});
                Ok((200, serde_json::to_vec(&serde_json::json!({"install": [entry(root), entry("@edge/core")], "errors": []})).unwrap()))
            })
        });
        let targets = rt().block_on(resolve_targets(&m, "https://reg.test", &get)).unwrap();
        let rel: Vec<_> = targets.iter().map(|t| t.rel()).collect();
        assert_eq!(rel, ["disabled/_edge_a.s2sp", "_edge_b.s2sp", "_edge_core.s2sp"]);
        m.disabled.insert("@edge/b".into());
        let targets = rt().block_on(resolve_targets(&m, "https://reg.test", &get)).unwrap();
        assert!(targets.iter().all(|t| t.disabled), "core follows once every plugin needing it is disabled");
    }

    #[test]
    fn the_startup_update_keeps_a_disabled_plugin_current_in_place() {
        let root = tmpdir("c-disabled-update");
        let get = fake_with_resolve(HashMap::from([("@edge/rtv", vec!["1.0.0"])]));
        op(&root, Op::Install { names: vec!["@edge/rtv@^1.0.0".into()], with: vec![], disabled: true }, &get).unwrap();
        let mut m = parse_manifest(&std::fs::read_to_string(root.join(MANIFEST_FILE)).unwrap(), "x").unwrap();
        m.timeout = Duration::from_secs(60);
        update(&root, &m, Fake { published: HashMap::from([("@edge/rtv", vec!["1.0.0", "1.1.0"])]), bad_download: None });
        assert_eq!(ls(&root.join("plugins")), ["disabled"]);
        assert_eq!(std::fs::read(root.join("plugins/disabled/_edge_rtv.s2sp")).unwrap(), body("@edge/rtv", "1.1.0"));
    }

    #[test]
    fn disabled_must_name_a_dependency() {
        let err = parse_manifest(r#"{"dependencies":{"a":"^1"},"disabled":["b"]}"#, "x").unwrap_err();
        assert!(err.contains("\"disabled\" names b"), "{err}");
    }

    #[test]
    fn console_ops_parse_and_run_one_at_a_time() {
        assert_eq!(parse_op(r#"{"op":"update","names":["a"]}"#).unwrap(), Op::Update { names: vec!["a".into()] });
        assert!(parse_op(r#"{"op":"format-c"}"#).is_err());
        assert_eq!(split_spec("@edge/rtv@^1.2.0"), ("@edge/rtv".into(), Some("^1.2.0".into())));
        assert_eq!(split_spec("@edge/rtv"), ("@edge/rtv".into(), None));
        assert_eq!(caret("1.2.3"), "^1.2.3");
        assert_eq!(caret("1.2.3-beta.1"), "^1.2.3-beta.1");
        *GATE.lock().unwrap() = None;
        try_begin_op().unwrap();
        assert!(try_begin_op().unwrap_err().contains("still running"));
        abort_op();
        try_begin_op().unwrap();
        abort_op();
    }

    #[test]
    fn plan_url_encodes_scoped_names_and_with() {
        assert_eq!(
            plan_url("https://r.test/", "@edge/jb", "^2.0.0", Some(&vec!["@edge/a".into(), "@edge/b".into()])),
            "https://r.test/api/v1/plan?name=%40edge%2Fjb&range=%5E2.0.0&with=%40edge%2Fa%2C%40edge%2Fb"
        );
    }
}
