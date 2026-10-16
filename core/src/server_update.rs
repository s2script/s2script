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

use std::collections::BTreeMap;
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
    let timeout_secs = s2
        .get("updateTimeoutSeconds")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(5, 600);
    Ok(ServerManifest {
        name: j.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(fallback_name).to_string(),
        dependencies,
        with,
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
    /// Install path (`plugins/<file>`) → plugin.
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
            continue;
        }
        let field = |f: &str| raw.get(f).and_then(Value::as_str).map(str::to_string);
        match (key.strip_prefix("plugins/"), field("name"), field("version"), field("resolved"), field("integrity")) {
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
        }
        let lock = self.0;
        let mut m = s.serialize_map(Some(lock.plugins.len() + 1))?;
        m.serialize_entry(
            "",
            &Root {
                name: &lock.root_name,
                dependencies: &lock.dependencies,
                with: (!lock.with.is_empty()).then_some(&lock.with),
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
            if let Some((prior, _)) = chosen.get(&n) {
                if prior.version != version {
                    return Err(format!("version conflict for {n}: {} vs {version}", prior.version));
                }
                continue;
            }
            chosen.insert(n.clone(), (Target { name: n, version, filename, url, sha256 }, review));
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

pub(crate) struct Staged {
    root: PathBuf,
    /// (tmp, dest, "name@version → file")
    renames: Vec<(PathBuf, PathBuf, String)>,
    pub lock: Lock,
    pub kept: usize,
}

impl Staged {
    fn discard(&self) {
        for (tmp, _, _) in &self.renames {
            let _ = std::fs::remove_file(tmp);
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
            plugins: BTreeMap::new(),
        },
        kept: 0,
    };
    let result: Result<(), String> = async {
        for t in targets {
            let integrity = integrity_from_hex(&t.sha256).ok_or_else(|| format!("{}: bad sha256 {}", t.name, t.sha256))?;
            staged.lock.plugins.insert(
                format!("{PLUGINS_SUBDIR}/{}", t.filename),
                LockedPlugin { name: t.name.clone(), version: t.version.clone(), resolved: t.url.clone(), integrity },
            );
            let dest = dir.join(&t.filename);
            if file_sha(&dest).as_deref() == Some(t.sha256.as_str()) {
                staged.kept += 1;
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
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let tmp = dir.join(format!("{}{STAGE_SUFFIX}", t.filename));
            std::fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
            staged.renames.push((tmp, dest, format!("{}@{} -> {}", t.name, t.version, t.filename)));
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
    for (tmp, dest, what) in &staged.renames {
        match std::fs::rename(tmp, dest) {
            Ok(()) => {
                installed += 1;
                log.push(format!("[update] installed {what}"));
            }
            Err(e) => log.push(format!("WARN: [update] could not install {what}: {e}")),
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
    log.push(format!("[update] plugins: {installed} installed, {} up to date, {removed} removed", staged.kept));
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
        stage(&cfg.root, &cfg.manifest, &targets, &get, host.as_deref()).await
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
                    url.split(['?', '&']).find_map(|p| p.strip_prefix(&format!("{k}="))).map(|v| v.replace("%40", "@").replace("%2F", "/").replace("%5E", "^"))
                };
                if url.contains("/api/v1/plan") {
                    let name = q("name").unwrap();
                    let range = q("range").unwrap();
                    let Some(vs) = fake.published.get(name.as_str()) else {
                        return Ok((404, br#"{"message":"package not found"}"#.to_vec()));
                    };
                    // Only the shapes the tests use: ^X.Y.Z means "same major".
                    let major = range.trim_start_matches('^').split('.').next().unwrap().to_string();
                    let v = vs.iter().rev().find(|v| v.split('.').next().unwrap() == major);
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
        };
        let staged = rt().block_on(stage(&root, &m, &[t], &get, Some("reg.test"))).unwrap();
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

    #[test]
    fn plan_url_encodes_scoped_names_and_with() {
        assert_eq!(
            plan_url("https://r.test/", "@edge/jb", "^2.0.0", Some(&vec!["@edge/a".into(), "@edge/b".into()])),
            "https://r.test/api/v1/plan?name=%40edge%2Fjb&range=%5E2.0.0&with=%40edge%2Fa%2C%40edge%2Fb"
        );
    }
}
