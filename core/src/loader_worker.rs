//! Bounded filesystem and archive work for the plugin loader.
//!
//! This module deliberately contains no engine or V8 handles. Every payload is owned and
//! `Send + 'static`; the game thread remains the sole owner of lifecycle and callbacks.

use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::hash::Hash;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

pub(crate) type Epoch = u64;
pub(crate) type PathRevision = u64;

#[derive(Debug, Deserialize, Clone)]
pub struct PublishDecl {
    pub version: String,
    #[serde(rename = "typesSha256", default)]
    pub types_sha256: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Manifest {
    pub id: String,
    pub version: String,
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    #[serde(rename = "pluginDependencies", default)]
    pub plugin_dependencies: HashMap<String, String>,
    #[serde(rename = "optionalPluginDependencies", default)]
    pub optional_plugin_dependencies: HashMap<String, String>,
    #[serde(default)]
    pub publishes: HashMap<String, PublishDecl>,
    #[serde(rename = "compiledAgainst", default)]
    pub compiled_against: HashMap<String, String>,
    #[serde(default)]
    pub config: HashMap<String, crate::config::ConfigEntry>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub gamedata: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ParseLimits {
    pub zip_entries: usize,
    pub member_name_bytes: usize,
    pub manifest_bytes: usize,
    pub plugin_js_bytes: usize,
    pub gamedata_bytes: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            zip_entries: 256,
            member_name_bytes: 64 << 10,
            manifest_bytes: 1 << 20,
            plugin_js_bytes: 16 << 20,
            gamedata_bytes: 8 << 20,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LoaderPolicy {
    pub request_items: usize,
    pub request_bytes: usize,
    pub result_items: usize,
    pub result_bytes: usize,
    pub prepared_items: usize,
    pub prepared_bytes: usize,
    pub scan_entries: usize,
    pub scan_candidates: usize,
    pub path_bytes: usize,
    pub archive_bytes: usize,
    pub config_bytes: usize,
    pub config_baseline_items: usize,
    pub config_baseline_bytes: usize,
    pub parse: ParseLimits,
    pub drain_items: usize,
    pub drain_bytes: usize,
    pub drain_micros: u64,
}

impl Default for LoaderPolicy {
    fn default() -> Self {
        Self {
            request_items: 128,
            request_bytes: 32 << 20,
            result_items: 128,
            result_bytes: 64 << 20,
            prepared_items: 32,
            prepared_bytes: 64 << 20,
            scan_entries: 4096,
            scan_candidates: 1024,
            path_bytes: 256 << 10,
            archive_bytes: 32 << 20,
            config_bytes: 1 << 20,
            config_baseline_items: 128,
            config_baseline_bytes: 32 << 20,
            parse: ParseLimits::default(),
            drain_items: 8,
            drain_bytes: 16 << 20,
            drain_micros: 1_000,
        }
    }
}

impl LoaderPolicy {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.request_items == 0 || self.result_items < self.request_items {
            return Err(
                "loader policy: result_items must cover every request_items obligation".into(),
            );
        }
        if self.request_bytes == 0
            || self.result_bytes == 0
            || self.prepared_items == 0
            || self.prepared_bytes == 0
            || self.scan_entries == 0
            || self.scan_candidates == 0
            || self.path_bytes == 0
            || self.archive_bytes == 0
            || self.config_bytes == 0
            || self.config_baseline_items == 0
            || self.config_baseline_bytes == 0
            || self.drain_items == 0
            || self.drain_bytes == 0
        {
            return Err("loader policy: all count and byte limits must be nonzero".into());
        }
        if self.archive_bytes > self.result_bytes {
            return Err(
                "loader policy: archive_bytes exceeds result_bytes; an item could never complete"
                    .into(),
            );
        }
        if self.config_bytes.saturating_mul(3).saturating_add(256) > self.result_bytes {
            return Err(
                "loader policy: worst-case lossy config text exceeds result_bytes; an item could never complete"
                    .into(),
            );
        }
        if self.config_bytes.saturating_mul(6) > self.config_baseline_bytes {
            return Err(
                "loader policy: config baseline cannot retain one committed and one proposed worst-case lossy config".into(),
            );
        }
        if self.scan_result_reservation() > self.result_bytes {
            return Err("loader policy: scan result envelope exceeds result_bytes".into());
        }
        if self
            .parse
            .manifest_bytes
            .saturating_add(self.parse.plugin_js_bytes)
            .saturating_add(self.parse.gamedata_bytes)
            > self.result_bytes
        {
            return Err("loader policy: parsed archive members exceed result_bytes".into());
        }
        if self.prepared_bytes
            < self
                .parse
                .manifest_bytes
                .saturating_add(self.parse.plugin_js_bytes)
                .saturating_add(self.parse.gamedata_bytes)
                .saturating_add(self.config_bytes.saturating_mul(3))
        {
            return Err(
                "loader policy: prepared_bytes cannot hold one maximum parsed archive and decoded config"
                    .into(),
            );
        }
        Ok(())
    }

    fn scan_result_reservation(&self) -> usize {
        self.path_bytes
            .saturating_add(self.scan_candidates.saturating_mul(96))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileStamp {
    pub len: u64,
    pub modified_ns: u128,
    pub created_ns: u128,
    pub device: u64,
    pub inode: u64,
    pub changed_ns: i128,
}

impl FileStamp {
    fn from_metadata(meta: &std::fs::Metadata) -> Self {
        let modified_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let created_ns = meta
            .created()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                len: meta.len(),
                modified_ns,
                created_ns,
                device: meta.dev(),
                inode: meta.ino(),
                changed_ns: i128::from(meta.ctime())
                    .saturating_mul(1_000_000_000)
                    .saturating_add(i128::from(meta.ctime_nsec())),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                len: meta.len(),
                modified_ns,
                created_ns,
                device: 0,
                inode: 0,
                changed_ns: 0,
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedPlugin {
    pub manifest: Manifest,
    pub js: String,
    pub gamedata: Option<String>,
    pub stamp: FileStamp,
    resident_bytes: usize,
}

impl PreparedPlugin {
    pub(crate) fn bytes(&self) -> usize {
        self.resident_bytes
    }

    #[cfg(test)]
    pub(crate) fn for_test(manifest: Manifest, js: &str, resident_bytes: usize) -> Self {
        Self {
            manifest,
            js: js.to_string(),
            gamedata: None,
            stamp: FileStamp {
                len: resident_bytes as u64,
                modified_ns: 1,
                created_ns: 1,
                device: 1,
                inode: 1,
                changed_ns: 1,
            },
            resident_bytes,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ConfigSnapshot {
    pub content: Option<Arc<str>>,
    pub stamp: Option<FileStamp>,
    pub watch: WatchDelta,
}

impl ConfigSnapshot {
    pub(crate) fn bytes(&self) -> usize {
        self.content.as_ref().map_or(0, |content| content.len())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WatchDelta {
    NotRequested,
    Seed,
    Unchanged,
    Changed,
    Pressure,
}

#[derive(Clone, Debug)]
pub(crate) enum WorkerResult {
    Scan {
        epoch: Epoch,
        revision: PathRevision,
        entries: Result<Vec<(PathBuf, FileStamp)>, String>,
    },
    Plugin {
        epoch: Epoch,
        revision: PathRevision,
        path: PathBuf,
        prepared: Result<PreparedPlugin, String>,
    },
    Config {
        epoch: Epoch,
        revision: PathRevision,
        path: PathBuf,
        watch_generation: Option<u64>,
        snapshot: Result<ConfigSnapshot, String>,
    },
}

impl WorkerResult {
    pub(crate) fn weight(&self) -> usize {
        match self {
            Self::Scan { entries, .. } => entries
                .as_ref()
                .map(|v| v.iter().map(|(p, _)| path_len(p) + 96).sum())
                .unwrap_or(256),
            Self::Plugin { prepared, path, .. } => {
                path_len(path) + prepared.as_ref().map_or(256, PreparedPlugin::bytes)
            }
            Self::Config { snapshot, path, .. } => {
                path_len(path)
                    + snapshot
                        .as_ref()
                        .ok()
                        .and_then(|s| s.content.as_ref())
                        .map_or(128, |content| content.len())
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Request {
    Scan {
        epoch: Epoch,
        revision: PathRevision,
        dir: PathBuf,
    },
    Plugin {
        epoch: Epoch,
        revision: PathRevision,
        path: PathBuf,
    },
    Config {
        epoch: Epoch,
        revision: PathRevision,
        path: PathBuf,
        watch_generation: Option<u64>,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum WorkKey {
    Scan(PathBuf),
    Plugin(PathBuf),
    Config(PathBuf),
}

impl Request {
    fn coalesce_with(self, older: Self) -> Self {
        match (self, older) {
            (
                Self::Config {
                    epoch,
                    revision,
                    path,
                    watch_generation,
                },
                Self::Config {
                    watch_generation: older_generation,
                    ..
                },
            ) => Self::Config {
                epoch,
                revision,
                path,
                watch_generation: watch_generation.max(older_generation),
            },
            (latest, _) => latest,
        }
    }

    fn key(&self) -> WorkKey {
        match self {
            Self::Scan { dir, .. } => WorkKey::Scan(dir.clone()),
            Self::Plugin { path, .. } => WorkKey::Plugin(path.clone()),
            Self::Config { path, .. } => WorkKey::Config(path.clone()),
        }
    }
    fn request_weight(&self) -> usize {
        match self {
            Self::Scan { dir, .. } => path_len(dir) + 64,
            Self::Plugin { path, .. } | Self::Config { path, .. } => path_len(path) + 64,
        }
    }
    fn result_reservation(&self, p: &LoaderPolicy) -> usize {
        match self {
            Self::Scan { .. } => p.scan_result_reservation(),
            Self::Plugin { path, .. } => path_len(path)
                .saturating_add(p.parse.manifest_bytes)
                .saturating_add(p.parse.plugin_js_bytes)
                .saturating_add(p.parse.gamedata_bytes)
                .saturating_add(2048),
            Self::Config { path, .. } => path_len(path)
                .saturating_add(p.config_bytes.saturating_mul(3))
                .saturating_add(256),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Submit {
    Accepted,
    Coalesced,
    Full,
    Oversized,
    Stopped,
}

struct State {
    stopped: bool,
    order: VecDeque<WorkKey>,
    pending: HashMap<WorkKey, Request>,
    rerun: HashMap<WorkKey, Request>,
    in_flight: HashSet<WorkKey>,
    results: VecDeque<(WorkKey, WorkerResult)>,
    obligations: HashMap<WorkKey, (usize, usize)>,
    request_bytes: usize,
    reserved_result_bytes: usize,
    // Every admitted watched generation reserves its retirement/control row before worker-owned
    // content can exist. Controls therefore progress independently of request/result saturation.
    config_watches: HashMap<PathBuf, ConfigWatchState>,
    config_watch_bytes: usize,
}

#[derive(Default)]
struct ConfigControl {
    resolution: Option<(u64, PathRevision, bool)>,
    retire_generation: Option<u64>,
}

#[derive(Default)]
struct ConfigWatchState {
    registered_generation: Option<u64>,
    control: ConfigControl,
}

impl ConfigControl {
    fn is_empty(&self) -> bool {
        self.resolution.is_none() && self.retire_generation.is_none()
    }
}

struct Shared {
    state: Mutex<State>,
    work: Condvar,
}

pub(crate) struct LoaderWorker {
    shared: Arc<Shared>,
    policy: LoaderPolicy,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl LoaderWorker {
    pub(crate) fn start(policy: LoaderPolicy) -> Result<Self, String> {
        policy.validate()?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                stopped: false,
                order: VecDeque::new(),
                pending: HashMap::new(),
                rerun: HashMap::new(),
                in_flight: HashSet::new(),
                results: VecDeque::new(),
                obligations: HashMap::new(),
                request_bytes: 0,
                reserved_result_bytes: 0,
                config_watches: HashMap::new(),
                config_watch_bytes: 0,
            }),
            work: Condvar::new(),
        });
        let thread_shared = Arc::clone(&shared);
        let thread_policy = policy.clone();
        let join = std::thread::Builder::new()
            .name("s2-loader".into())
            .spawn(move || worker_loop(thread_shared, thread_policy))
            .map_err(|e| format!("loader worker start: {e}"))?;
        Ok(Self {
            shared,
            policy,
            join: Mutex::new(Some(join)),
        })
    }

    pub(crate) fn try_scan(&self, epoch: Epoch, revision: PathRevision, dir: PathBuf) -> Submit {
        self.try_submit(Request::Scan {
            epoch,
            revision,
            dir,
        })
    }
    pub(crate) fn try_prepare(
        &self,
        epoch: Epoch,
        revision: PathRevision,
        path: PathBuf,
    ) -> Submit {
        self.try_submit(Request::Plugin {
            epoch,
            revision,
            path,
        })
    }
    pub(crate) fn try_read_config(
        &self,
        epoch: Epoch,
        revision: PathRevision,
        path: PathBuf,
        watch_generation: Option<u64>,
    ) -> Submit {
        self.try_submit(Request::Config {
            epoch,
            revision,
            path,
            watch_generation,
        })
    }

    fn try_submit(&self, request: Request) -> Submit {
        let key = request.key();
        let req_weight = request.request_weight();
        let result_weight = request.result_reservation(&self.policy);
        if req_weight > self.policy.request_bytes || result_weight > self.policy.result_bytes {
            return Submit::Oversized;
        }
        let Ok(mut state) = self.shared.state.lock() else {
            return Submit::Stopped;
        };
        if state.stopped {
            return Submit::Stopped;
        }
        let watch = match &request {
            Request::Config { path, watch_generation: Some(generation), .. } => {
                Some((path.clone(), *generation))
            }
            _ => None,
        };
        let watch_weight = watch.as_ref().map_or(0, |(path, _)| config_watch_weight(path));
        if watch.as_ref().is_some_and(|(path, _)| {
            !state.config_watches.contains_key(path) && watch_weight > self.policy.request_bytes
        }) {
            return Submit::Oversized;
        }
        if watch.as_ref().is_some_and(|(path, _)| {
            !state.config_watches.contains_key(path)
                && (state.config_watches.len() >= self.policy.request_items
                    || state.config_watch_bytes.saturating_add(watch_weight) > self.policy.request_bytes)
        }) {
            return Submit::Full;
        }
        if state.obligations.contains_key(&key) {
            if let Some((path, generation)) = watch {
                if !state.config_watches.contains_key(&path) {
                    state.config_watch_bytes = state.config_watch_bytes.saturating_add(watch_weight);
                }
                let registered = &mut state.config_watches.entry(path).or_default().registered_generation;
                *registered = Some(registered.map_or(generation, |old| old.max(generation)));
            }
            if state.in_flight.contains(&key) {
                let request = state.rerun.remove(&key).map_or(request.clone(), |older| {
                    request.clone().coalesce_with(older)
                });
                state.rerun.insert(key, request);
            } else if let Some(old) = state.pending.remove(&key) {
                let request = request.coalesce_with(old.clone());
                state.request_bytes = state
                    .request_bytes
                    .saturating_sub(old.request_weight())
                    .saturating_add(req_weight);
                state.pending.insert(key, request);
            } else {
                state.results.retain(|(k, _)| k != &key);
                state.pending.insert(key.clone(), request);
                state.order.push_back(key);
            }
            self.shared.work.notify_one();
            return Submit::Coalesced;
        }
        if state.obligations.len() >= self.policy.request_items
            || state.request_bytes.saturating_add(req_weight) > self.policy.request_bytes
            || state.obligations.len() >= self.policy.result_items
            || state.reserved_result_bytes.saturating_add(result_weight) > self.policy.result_bytes
        {
            return Submit::Full;
        }
        if let Some((path, generation)) = watch {
            if !state.config_watches.contains_key(&path) {
                state.config_watch_bytes = state.config_watch_bytes.saturating_add(watch_weight);
            }
            let registered = &mut state.config_watches.entry(path).or_default().registered_generation;
            *registered = Some(registered.map_or(generation, |old| old.max(generation)));
        }
        state.request_bytes += req_weight;
        state.reserved_result_bytes += result_weight;
        state
            .obligations
            .insert(key.clone(), (req_weight, result_weight));
        state.pending.insert(key.clone(), request);
        state.order.push_back(key);
        self.shared.work.notify_one();
        Submit::Accepted
    }

    pub(crate) fn try_result(&self) -> Option<WorkerResult> {
        let mut state = self.shared.state.lock().ok()?;
        let (key, result) = state.results.pop_front()?;
        if let Some((req, result)) = state.obligations.remove(&key) {
            state.request_bytes = state.request_bytes.saturating_sub(req);
            state.reserved_result_bytes = state.reserved_result_bytes.saturating_sub(result);
        }
        Some(result)
    }

    /// Resolve a worker-owned config-baseline proposal after every current main-thread consumer has
    /// handled the result. `applied=false` discards it without advancing the committed baseline.
    pub(crate) fn resolve_config_watch(
        &self,
        path: PathBuf,
        generation: u64,
        revision: PathRevision,
        applied: bool,
    ) {
        let Ok(mut state) = self.shared.state.lock() else { return };
        if state.stopped { return; }
        let Some(watch) = state.config_watches.get_mut(&path) else { return };
        let control = &mut watch.control;
        if control.resolution.is_none_or(|(old_generation, old_revision, _)| {
            (generation, revision) >= (old_generation, old_revision)
        }) {
            control.resolution = Some((generation, revision, applied));
        }
        drop(state);
        self.shared.work.notify_one();
    }

    /// Retire the final watcher for this generation. The worker applies retirement, so dropping a
    /// file-sized baseline never occurs on the game thread. A newer generation is revision-fenced.
    pub(crate) fn retire_config_watch(&self, path: PathBuf, generation: u64) {
        let Ok(mut state) = self.shared.state.lock() else { return };
        if state.stopped { return; }
        let Some(watch) = state.config_watches.get_mut(&path) else { return };
        if watch.registered_generation == Some(generation) {
            watch.registered_generation = None;
        }
        let control = &mut watch.control;
        control.retire_generation = Some(
            control.retire_generation.map_or(generation, |old| old.max(generation)),
        );
        drop(state);
        self.shared.work.notify_one();
    }

    pub(crate) fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.stopped = true;
            state.order.clear();
            state.pending.clear();
            state.rerun.clear();
            state.results.clear();
            state.obligations.clear();
            state.request_bytes = 0;
            state.reserved_result_bytes = 0;
            state.config_watches.clear();
            state.config_watch_bytes = 0;
        }
        self.shared.work.notify_all();
        if let Ok(mut slot) = self.join.lock() {
            if let Some(join) = slot.take() {
                let _ = join.join();
            }
        }
    }
}

impl Drop for LoaderWorker {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

enum WorkerTurn {
    Controls(Vec<(PathBuf, ConfigControl)>),
    Request(WorkKey, Request),
}

fn worker_loop(shared: Arc<Shared>, policy: LoaderPolicy) {
    let mut config_baselines = ConfigBaselines::default();
    loop {
        let turn = {
            let mut state = match shared.state.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            loop {
                if state.stopped {
                    return;
                }
                if state.config_watches.values().any(|watch| !watch.control.is_empty()) {
                    let controls = state.config_watches.iter_mut().filter_map(|(path, watch)| {
                        (!watch.control.is_empty()).then(|| {
                            (path.clone(), std::mem::take(&mut watch.control))
                        })
                    }).collect();
                    break WorkerTurn::Controls(controls);
                }
                if let Some(key) = state.order.pop_front() {
                    if let Some(request) = state.pending.remove(&key) {
                        state.in_flight.insert(key.clone());
                        break WorkerTurn::Request(key, request);
                    }
                }
                state = match shared.work.wait(state) {
                    Ok(s) => s,
                    Err(_) => return,
                };
            }
        };
        let (key, request) = match turn {
            WorkerTurn::Request(key, request) => (key, request),
            WorkerTurn::Controls(controls) => {
                for (path, control) in controls {
                    if let Some((generation, revision, applied)) = control.resolution {
                        config_baselines.resolve(&path, generation, revision, applied);
                    }
                    if let Some(generation) = control.retire_generation {
                        config_baselines.retire(&path, generation);
                    }
                }
                if let Ok(mut state) = shared.state.lock() {
                    state.config_watches.retain(|_, watch| {
                        watch.registered_generation.is_some() || !watch.control.is_empty()
                    });
                    state.config_watch_bytes = state.config_watches.keys().fold(0usize, |bytes, path| {
                        bytes.saturating_add(config_watch_weight(path))
                    });
                }
                continue;
            }
        };
        let result = run_request(request, &policy, &mut config_baselines);
        let mut state = match shared.state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        state.in_flight.remove(&key);
        if state.stopped {
            return;
        }
        if let Some(next) = state.rerun.remove(&key) {
            state.pending.insert(key.clone(), next);
            state.order.push_back(key);
        } else {
            state.results.push_back((key, result));
        }
    }
}

#[derive(Default)]
struct ConfigBaselines {
    values: HashMap<PathBuf, ConfigBaseline>,
    proposals: HashMap<PathBuf, ConfigProposal>,
    bytes: usize,
}

struct ConfigBaseline {
    generation: u64,
    content: Option<Arc<str>>,
}

struct ConfigProposal {
    generation: u64,
    revision: PathRevision,
    content: Option<Arc<str>>,
}

impl ConfigBaselines {
    fn entry_bytes(path: &Path, content: &Option<Arc<str>>) -> usize {
        path_len(path).saturating_add(content.as_ref().map_or(0, |value| value.len()))
    }

    fn path_count(&self) -> usize {
        self.values.len()
            + self
                .proposals
                .keys()
                .filter(|path| !self.values.contains_key(*path))
                .count()
    }

    fn drop_proposal(&mut self, path: &Path) {
        if let Some(proposal) = self.proposals.remove(path) {
            self.bytes = self
                .bytes
                .saturating_sub(Self::entry_bytes(path, &proposal.content));
        }
    }

    fn discard_superseded(&mut self, path: &Path, generation: u64, revision: PathRevision) {
        if self.proposals.get(path).is_some_and(|proposal| {
            (proposal.generation, proposal.revision) < (generation, revision)
        }) {
            self.drop_proposal(path);
        }
    }

    fn compare_and_propose(
        &mut self,
        path: &Path,
        generation: u64,
        revision: PathRevision,
        content: &Option<Arc<str>>,
        policy: &LoaderPolicy,
    ) -> WatchDelta {
        let decision = match self.values.get(path) {
            Some(old) if old.generation != generation => WatchDelta::Seed,
            None => WatchDelta::Seed,
            Some(old) if old.content == *content => {
                self.drop_proposal(path);
                return WatchDelta::Unchanged;
            }
            Some(_) => WatchDelta::Changed,
        };
        self.drop_proposal(path);
        let new_path = !self.values.contains_key(path) && !self.proposals.contains_key(path);
        let new_bytes = Self::entry_bytes(path, content);
        if (new_path && self.path_count() >= policy.config_baseline_items)
            || self.bytes.saturating_add(new_bytes) > policy.config_baseline_bytes
        {
            return WatchDelta::Pressure;
        }
        self.bytes = self.bytes.saturating_add(new_bytes);
        self.proposals.insert(path.to_path_buf(), ConfigProposal {
            generation,
            revision,
            content: content.clone(),
        });
        decision
    }

    fn resolve(&mut self, path: &Path, generation: u64, revision: PathRevision, applied: bool) {
        if self.proposals.get(path).is_none_or(|proposal| {
            proposal.generation != generation || proposal.revision != revision
        }) {
            return;
        }
        let proposal = self.proposals.remove(path).unwrap();
        self.bytes = self
            .bytes
            .saturating_sub(Self::entry_bytes(path, &proposal.content));
        if !applied { return; }
        if let Some(old) = self.values.remove(path) {
            self.bytes = self
                .bytes
                .saturating_sub(Self::entry_bytes(path, &old.content));
        }
        self.bytes = self
            .bytes
            .saturating_add(Self::entry_bytes(path, &proposal.content));
        self.values.insert(path.to_path_buf(), ConfigBaseline {
            generation: proposal.generation,
            content: proposal.content,
        });
    }

    fn retire(&mut self, path: &Path, generation: u64) {
        if self.values.get(path).is_some_and(|entry| entry.generation == generation) {
            let old = self.values.remove(path).unwrap();
            self.bytes = self
                .bytes
                .saturating_sub(Self::entry_bytes(path, &old.content));
        }
        if self.proposals.get(path).is_some_and(|entry| entry.generation == generation) {
            self.drop_proposal(path);
        }
    }
}

fn run_request(
    request: Request,
    policy: &LoaderPolicy,
    config_baselines: &mut ConfigBaselines,
) -> WorkerResult {
    match request {
        Request::Scan {
            epoch,
            revision,
            dir,
        } => WorkerResult::Scan {
            epoch,
            revision,
            entries: scan_plugins(&dir, policy),
        },
        Request::Plugin {
            epoch,
            revision,
            path,
        } => {
            let prepared = stable_read(&path, policy.archive_bytes).and_then(|(bytes, stamp)| {
                parse_s2sp_parts(&bytes, policy.parse).map(
                    |(manifest, js, gamedata, manifest_bytes)| {
                        let resident_bytes = js
                            .len()
                            .saturating_add(gamedata.as_ref().map_or(0, String::len))
                            .saturating_add(manifest_bytes);
                        PreparedPlugin {
                            manifest,
                            js,
                            gamedata,
                            stamp,
                            resident_bytes,
                        }
                    },
                )
            });
            WorkerResult::Plugin {
                epoch,
                revision,
                path,
                prepared,
            }
        }
        Request::Config {
            epoch,
            revision,
            path,
            watch_generation,
        } => {
            if let Some(generation) = watch_generation {
                config_baselines.discard_superseded(&path, generation, revision);
            }
            let snapshot = match stable_read(&path, policy.config_bytes) {
                Ok((bytes, stamp)) => {
                    let content: Option<Arc<str>> = Some(String::from_utf8_lossy(&bytes).into_owned().into());
                    let watch = if let Some(generation) = watch_generation {
                        config_baselines.compare_and_propose(&path, generation, revision, &content, policy)
                    } else {
                        WatchDelta::NotRequested
                    };
                    Ok(ConfigSnapshot {
                        content,
                        stamp: Some(stamp),
                        watch,
                    })
                }
                Err(e) if e.starts_with("missing:") => {
                    let content = None;
                    let watch = if let Some(generation) = watch_generation {
                        config_baselines.compare_and_propose(&path, generation, revision, &content, policy)
                    } else {
                        WatchDelta::NotRequested
                    };
                    Ok(ConfigSnapshot {
                        content,
                        stamp: None,
                        watch,
                    })
                }
                Err(e) => Err(e),
            };
            WorkerResult::Config {
                epoch,
                revision,
                path,
                watch_generation,
                snapshot,
            }
        }
    }
}

fn path_len(path: &Path) -> usize {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().len()
    }
    #[cfg(not(unix))]
    {
        path.as_os_str().to_string_lossy().len()
    }
}

fn config_watch_weight(path: &Path) -> usize {
    // Persistent reservation key plus the worker-turn copy used to process a control.
    path_len(path).saturating_mul(2).saturating_add(64)
}

pub(crate) fn scan_plugins(
    dir: &Path,
    policy: &LoaderPolicy,
) -> Result<Vec<(PathBuf, FileStamp)>, String> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("plugin scan {:?}: {e}", dir)),
    };
    let mut out = Vec::new();
    let mut total_path_bytes = 0usize;
    let mut examined = 0usize;
    for entry in rd {
        examined += 1;
        if examined > policy.scan_entries {
            return Err(format!("plugin scan {:?}: entry limit exceeded", dir));
        }
        let entry = entry.map_err(|e| format!("plugin scan {:?}: {e}", dir))?;
        let path = entry.path();
        total_path_bytes = total_path_bytes.saturating_add(path_len(&path));
        if total_path_bytes > policy.path_bytes {
            return Err(format!("plugin scan {:?}: path byte limit exceeded", dir));
        }
        if path.extension().and_then(|e| e.to_str()) != Some("s2sp") {
            continue;
        }
        let meta = entry
            .metadata()
            .map_err(|e| format!("plugin scan {:?}: {e}", path))?;
        if !meta.file_type().is_file() {
            continue;
        }
        if out.len() >= policy.scan_candidates {
            return Err(format!("plugin scan {:?}: candidate limit exceeded", dir));
        }
        out.push((path, FileStamp::from_metadata(&meta)));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadStage {
    AfterFirstRead,
}

fn open_nonblocking(path: &Path) -> Result<File, std::io::Error> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options.open(path)
}

fn read_open_file(mut file: File, path: &Path, max: usize) -> Result<(Vec<u8>, FileStamp), String> {
    let before = file
        .metadata()
        .map_err(|e| format!("read {:?}: metadata: {e}", path))?;
    if !before.file_type().is_file() {
        return Err(format!("read {:?}: not a regular file", path));
    }
    if before.len() > max as u64 {
        return Err(format!("read {:?}: byte limit {max} exceeded", path));
    }
    let mut bytes = Vec::with_capacity((before.len() as usize).min(max));
    file.by_ref()
        .take(max.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read {:?}: {e}", path))?;
    if bytes.len() > max {
        return Err(format!("read {:?}: byte limit {max} exceeded", path));
    }
    let after = file
        .metadata()
        .map_err(|e| format!("read {:?}: metadata: {e}", path))?;
    let a = FileStamp::from_metadata(&before);
    let b = FileStamp::from_metadata(&after);
    if a != b || bytes.len() as u64 != b.len {
        return Err(format!("read {:?}: changed while reading", path));
    }
    Ok((bytes, b))
}

fn stable_read(path: &Path, max: usize) -> Result<(Vec<u8>, FileStamp), String> {
    stable_read_with_hook(path, max, |_stage| {
        #[cfg(test)]
        if _stage == ReadStage::AfterFirstRead {
            let gate = TEST_READ_GATE.lock().ok().and_then(|g| g.clone());
            if let Some(gate) = gate {
                let mut state = gate.0.lock().unwrap();
                state.0 = true;
                gate.1.notify_all();
                while !state.1 {
                    state = gate.1.wait(state).unwrap();
                }
            }
        }
    })
}

#[cfg(test)]
type TestReadGate = std::sync::Arc<(Mutex<(bool, bool)>, Condvar)>;
#[cfg(test)]
static TEST_READ_GATE: Mutex<Option<TestReadGate>> = Mutex::new(None);

fn stable_read_with_hook(
    path: &Path,
    max: usize,
    hook: impl Fn(ReadStage),
) -> Result<(Vec<u8>, FileStamp), String> {
    let first = match open_nonblocking(path) {
        Ok(file) => read_open_file(file, path, max),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("missing: {:?}", path));
        }
        Err(e) => return Err(format!("read {:?}: open: {e}", path)),
    }?;
    hook(ReadStage::AfterFirstRead);
    let second = open_nonblocking(path)
        .map_err(|e| format!("read {:?}: changed while reading: {e}", path))
        .and_then(|file| read_open_file(file, path, max))?;
    if first.1 != second.1 || first.0 != second.0 {
        return Err(format!("read {:?}: changed while reading", path));
    }
    Ok(second)
}

fn read_zip_member<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    max: usize,
    required: bool,
) -> Result<Option<String>, String> {
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(_) if !required => return Ok(None),
        Err(_) => return Err(format!("read_s2sp: missing {name} in archive")),
    };
    if entry.size() > max as u64 {
        return Err(format!("read_s2sp: {name} exceeds byte limit {max}"));
    }
    let mut bytes = Vec::with_capacity((entry.size() as usize).min(max));
    entry
        .by_ref()
        .take(max.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read_s2sp: failed to read {name}: {e}"))?;
    if bytes.len() > max {
        return Err(format!("read_s2sp: {name} exceeds byte limit {max}"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|e| format!("read_s2sp: invalid UTF-8 in {name}: {e}"))
}

pub(crate) fn parse_s2sp(
    bytes: &[u8],
    limits: ParseLimits,
) -> Result<(Manifest, String, Option<String>), String> {
    parse_s2sp_parts(bytes, limits).map(|(manifest, js, gamedata, _)| (manifest, js, gamedata))
}

fn parse_s2sp_parts(
    bytes: &[u8],
    limits: ParseLimits,
) -> Result<(Manifest, String, Option<String>, usize), String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("read_s2sp: not a valid zip: {e}"))?;
    if archive.len() > limits.zip_entries {
        return Err(format!(
            "read_s2sp: archive entry limit {} exceeded",
            limits.zip_entries
        ));
    }
    let mut name_bytes = 0usize;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| format!("read_s2sp: invalid archive entry {index}: {e}"))?;
        name_bytes = name_bytes.saturating_add(entry.name_raw().len());
        if name_bytes > limits.member_name_bytes {
            return Err(format!(
                "read_s2sp: archive member-name byte limit {} exceeded",
                limits.member_name_bytes
            ));
        }
    }
    let manifest_json =
        read_zip_member(&mut archive, "manifest.json", limits.manifest_bytes, true)?.unwrap();
    let manifest_bytes = manifest_json.len();
    let manifest: Manifest = serde_json::from_str(&manifest_json)
        .map_err(|e| format!("read_s2sp: invalid manifest.json: {e}"))?;
    if crate::gamedata_calls::is_reserved_owner(&manifest.id) {
        return Err(format!(
            "read_s2sp: manifest id {:?} is in the reserved '{}' namespace, which belongs to the runtime — rename the plugin",
            manifest.id,
            crate::gamedata_calls::RESERVED_OWNER_PREFIX
        ));
    }
    let js = read_zip_member(&mut archive, "plugin.js", limits.plugin_js_bytes, true)?.unwrap();
    let gamedata = read_zip_member(&mut archive, "gamedata.json", limits.gamedata_bytes, false)
        .unwrap_or(None);
    Ok((manifest, js, gamedata, manifest_bytes))
}

fn assert_send_static<T: Send + 'static>() {}
#[allow(dead_code)]
fn payload_contracts() {
    assert_send_static::<Request>();
    assert_send_static::<WorkerResult>();
    assert_send_static::<PreparedPlugin>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn next_result(worker: &LoaderWorker) -> WorkerResult {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(result) = worker.try_result() {
                return result;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not make bounded progress"
            );
            std::thread::yield_now();
        }
    }

    fn archive(manifest: &str, js: &[u8]) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = || {
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored)
        };
        zip.start_file("manifest.json", opts()).unwrap();
        zip.write_all(manifest.as_bytes()).unwrap();
        zip.start_file("plugin.js", opts()).unwrap();
        zip.write_all(js).unwrap();
        zip.finish().unwrap().into_inner()
    }

    fn archive_with_gamedata(manifest: &str, js: &[u8], gamedata: &[u8]) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = || {
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored)
        };
        for (name, bytes) in [
            ("manifest.json", manifest.as_bytes()),
            ("plugin.js", js),
            ("gamedata.json", gamedata),
        ] {
            zip.start_file(name, opts()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn policy_rejects_a_result_budget_that_cannot_cover_each_admitted_obligation() {
        let mut p = LoaderPolicy::default();
        p.request_items = 8;
        p.result_items = 7;
        assert!(p.validate().unwrap_err().contains("result_items"));

        p.result_items = 8;
        p.archive_bytes = p.result_bytes + 1;
        assert!(p.validate().unwrap_err().contains("archive_bytes"));
    }

    #[test]
    fn policy_requires_retained_capacity_for_archive_plus_decoded_config() {
        let mut p = LoaderPolicy::default();
        p.prepared_bytes =
            p.parse.manifest_bytes + p.parse.plugin_js_bytes + p.parse.gamedata_bytes;
        assert!(p.validate().unwrap_err().contains("prepared_bytes"));

        let mut p = LoaderPolicy::default();
        p.config_baseline_bytes = p.config_bytes * 3;
        assert!(p.validate().unwrap_err().contains("config baseline"));
    }

    #[test]
    fn request_queue_coalesces_the_same_path_without_consuming_another_slot() {
        let p = LoaderPolicy {
            request_items: 1,
            result_items: 1,
            ..LoaderPolicy::default()
        };
        let worker = LoaderWorker::start(p).unwrap();
        let path = std::path::PathBuf::from("missing.s2sp");
        assert_eq!(worker.try_prepare(1, 1, path.clone()), Submit::Accepted);
        assert_eq!(worker.try_prepare(1, 2, path), Submit::Coalesced);
        assert!(matches!(
            next_result(&worker),
            WorkerResult::Plugin { revision: 2, .. }
        ));
        worker.shutdown();
    }

    #[test]
    fn three_revisions_while_the_first_is_in_flight_keep_only_the_latest_rerun() {
        let path = std::env::temp_dir().join(format!("s2-loader-abc-{}", std::process::id()));
        std::fs::write(&path, b"{}").unwrap();
        let gate: TestReadGate = Arc::new((Mutex::new((false, false)), Condvar::new()));
        *TEST_READ_GATE.lock().unwrap() = Some(gate.clone());
        let worker = LoaderWorker::start(LoaderPolicy::default()).unwrap();
        assert_eq!(
            worker.try_read_config(7, 1, path.clone(), None),
            Submit::Accepted
        );

        let mut state = gate.0.lock().unwrap();
        while !state.0 {
            state = gate.1.wait(state).unwrap();
        }
        assert_eq!(
            worker.try_read_config(7, 2, path.clone(), Some(1)),
            Submit::Coalesced
        );
        assert_eq!(
            worker.try_read_config(7, 3, path.clone(), None),
            Submit::Coalesced
        );
        state.1 = true;
        gate.1.notify_all();
        drop(state);

        match next_result(&worker) {
            WorkerResult::Config {
                revision,
                snapshot: Ok(snapshot),
                ..
            } => {
                assert_eq!(revision, 3);
                assert_eq!(
                    snapshot.watch,
                    WatchDelta::Seed,
                    "coalescing must retain watch intent"
                );
            }
            other => panic!("unexpected result: {other:?}"),
        }
        worker.shutdown();
        *TEST_READ_GATE.lock().unwrap() = None;
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn archive_member_limits_reject_oversized_plugin_js_before_allocation() {
        let bytes = archive(
            r#"{"id":"@demo/a","version":"1.0.0","apiVersion":"2.x"}"#,
            b"123456789",
        );
        let limits = ParseLimits {
            plugin_js_bytes: 8,
            ..ParseLimits::default()
        };
        let err = parse_s2sp(&bytes, limits).unwrap_err();
        assert!(err.contains("plugin.js"));
        assert!(err.contains("limit"));
    }

    #[test]
    fn stable_read_detects_delete_and_recreate_during_the_read() {
        let dir = std::env::temp_dir().join(format!("s2-loader-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.s2sp");
        std::fs::write(&path, b"old").unwrap();
        let err = stable_read_with_hook(&path, 32, |stage| {
            if stage == ReadStage::AfterFirstRead {
                std::fs::remove_file(&path).unwrap();
                std::fs::write(&path, b"new").unwrap();
            }
        })
        .unwrap_err();
        assert!(err.contains("changed while reading"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn scan_error_is_distinct_from_a_missing_directory() {
        let root = std::env::temp_dir().join(format!("s2-loader-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            scan_plugins(&root, &LoaderPolicy::default())
                .unwrap()
                .is_empty()
        );
        std::fs::write(&root, b"not a directory").unwrap();
        assert!(
            scan_plugins(&root, &LoaderPolicy::default())
                .unwrap_err()
                .contains("scan")
        );
        std::fs::remove_file(root).unwrap();
    }

    #[test]
    fn queue_pressure_is_retryable_after_the_reserved_result_is_drained() {
        let p = LoaderPolicy {
            request_items: 1,
            result_items: 1,
            ..LoaderPolicy::default()
        };
        let worker = LoaderWorker::start(p).unwrap();
        assert_eq!(
            worker.try_prepare(3, 1, PathBuf::from("a-missing.s2sp")),
            Submit::Accepted
        );
        assert_eq!(
            worker.try_prepare(3, 1, PathBuf::from("b-missing.s2sp")),
            Submit::Full
        );
        let _ = next_result(&worker);
        assert_eq!(
            worker.try_prepare(3, 2, PathBuf::from("b-missing.s2sp")),
            Submit::Accepted
        );
        worker.shutdown();
    }

    #[test]
    fn missing_config_is_a_successful_absent_snapshot_with_original_tags() {
        let worker = LoaderWorker::start(LoaderPolicy::default()).unwrap();
        let path = PathBuf::from("definitely-missing-config.json");
        assert_eq!(
            worker.try_read_config(41, 9, path.clone(), None),
            Submit::Accepted
        );
        match next_result(&worker) {
            WorkerResult::Config {
                epoch,
                revision,
                path: got,
                snapshot,
                ..
            } => {
                assert_eq!((epoch, revision, got), (41, 9, path));
                assert!(snapshot.unwrap().content.is_none());
            }
            other => panic!("unexpected result: {other:?}"),
        }
        worker.shutdown();
    }

    #[test]
    fn config_utf8_decoding_matches_the_existing_lossy_engine_adapter() {
        let path = std::env::temp_dir().join(format!("s2-loader-config-{}", std::process::id()));
        std::fs::write(&path, [b'{', 0xff, b'}']).unwrap();
        let worker = LoaderWorker::start(LoaderPolicy::default()).unwrap();
        assert_eq!(
            worker.try_read_config(4, 2, path.clone(), None),
            Submit::Accepted
        );
        match next_result(&worker) {
            WorkerResult::Config { snapshot, .. } => {
                assert_eq!(snapshot.unwrap().content.as_deref(), Some("{�}"));
            }
            other => panic!("unexpected result: {other:?}"),
        }
        worker.shutdown();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn config_result_reservation_covers_worst_case_lossy_utf8_expansion() {
        let path = std::env::temp_dir().join(format!("s2-loader-lossy-{}", std::process::id()));
        std::fs::write(&path, vec![0xff; 1024]).unwrap();
        let mut policy = LoaderPolicy::default();
        policy.config_bytes = 1024;
        let reserved = Request::Config {
            epoch: 1,
            revision: 1,
            path: path.clone(),
            watch_generation: None,
        }
        .result_reservation(&policy);
        let worker = LoaderWorker::start(policy).unwrap();
        assert_eq!(
            worker.try_read_config(1, 1, path.clone(), None),
            Submit::Accepted
        );
        let result = next_result(&worker);
        assert!(
            result.weight() <= reserved,
            "{} > {}",
            result.weight(),
            reserved
        );
        worker.shutdown();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn watched_config_exact_comparison_and_baseline_live_on_the_worker() {
        let path = std::env::temp_dir().join(format!("s2-loader-watch-{}", std::process::id()));
        std::fs::write(&path, b"alpha").unwrap();
        let worker = LoaderWorker::start(LoaderPolicy::default()).unwrap();

        assert_eq!(
            worker.try_read_config(1, 1, path.clone(), Some(1)),
            Submit::Accepted
        );
        match next_result(&worker) {
            WorkerResult::Config {
                snapshot: Ok(snapshot),
                ..
            } => {
                assert_eq!(snapshot.watch, WatchDelta::Seed);
            }
            other => panic!("unexpected result: {other:?}"),
        }
        worker.resolve_config_watch(path.clone(), 1, 1, true);
        assert_eq!(
            worker.try_read_config(1, 2, path.clone(), Some(1)),
            Submit::Accepted
        );
        match next_result(&worker) {
            WorkerResult::Config {
                snapshot: Ok(snapshot),
                ..
            } => {
                assert_eq!(snapshot.watch, WatchDelta::Unchanged);
            }
            other => panic!("unexpected result: {other:?}"),
        }
        std::fs::write(&path, b"beta").unwrap();
        assert_eq!(
            worker.try_read_config(1, 3, path.clone(), Some(1)),
            Submit::Accepted
        );
        match next_result(&worker) {
            WorkerResult::Config {
                snapshot: Ok(snapshot),
                ..
            } => {
                assert_eq!(snapshot.watch, WatchDelta::Changed);
            }
            other => panic!("unexpected result: {other:?}"),
        }

        worker.shutdown();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn baseline_and_proposal_bytes_stay_charged_until_worker_resolution_or_retirement() {
        let policy = LoaderPolicy::default();
        let path = PathBuf::from("config.json");
        let alpha: Option<Arc<str>> = Some(Arc::from("alpha"));
        let beta: Option<Arc<str>> = Some(Arc::from("beta"));
        let alpha_bytes = ConfigBaselines::entry_bytes(&path, &alpha);
        let beta_bytes = ConfigBaselines::entry_bytes(&path, &beta);
        let mut baselines = ConfigBaselines::default();

        assert_eq!(baselines.compare_and_propose(&path, 1, 1, &alpha, &policy), WatchDelta::Seed);
        assert_eq!(baselines.bytes, alpha_bytes);
        baselines.resolve(&path, 1, 1, true);
        assert_eq!(baselines.bytes, alpha_bytes);
        assert_eq!(baselines.compare_and_propose(&path, 1, 2, &beta, &policy), WatchDelta::Changed);
        assert_eq!(baselines.bytes, alpha_bytes + beta_bytes);
        baselines.retire(&path, 1);
        assert_eq!(baselines.bytes, 0);
        assert!(baselines.values.is_empty());
        assert!(baselines.proposals.is_empty());
    }

    #[test]
    fn watched_config_baseline_pressure_is_bounded_and_retryable() {
        let root = std::env::temp_dir().join(format!("s2-loader-watch-cap-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.json");
        let second = root.join("second.json");
        std::fs::write(&first, b"a").unwrap();
        std::fs::write(&second, b"b").unwrap();
        let mut policy = LoaderPolicy::default();
        policy.config_baseline_items = 1;
        let worker = LoaderWorker::start(policy).unwrap();
        assert_eq!(worker.try_read_config(1, 1, first.clone(), Some(1)), Submit::Accepted);
        assert!(matches!(
            next_result(&worker),
            WorkerResult::Config {
                snapshot: Ok(ConfigSnapshot {
                    watch: WatchDelta::Seed,
                    ..
                }),
                ..
            }
        ));
        worker.resolve_config_watch(first.clone(), 1, 1, true);
        assert_eq!(worker.try_read_config(1, 2, second, Some(1)), Submit::Accepted);
        assert!(matches!(
            next_result(&worker),
            WorkerResult::Config {
                snapshot: Ok(ConfigSnapshot {
                    watch: WatchDelta::Pressure,
                    ..
                }),
                ..
            }
        ));
        worker.shutdown();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn undelivered_change_survives_a_coalesced_unchanged_reread() {
        let path = std::env::temp_dir().join(format!("s2-loader-watch-ack-{}", std::process::id()));
        std::fs::write(&path, b"alpha").unwrap();
        let worker = LoaderWorker::start(LoaderPolicy::default()).unwrap();
        assert_eq!(worker.try_read_config(1, 1, path.clone(), Some(1)), Submit::Accepted);
        assert!(matches!(next_result(&worker), WorkerResult::Config {
            snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Seed, .. }), ..
        }));
        worker.resolve_config_watch(path.clone(), 1, 1, true);

        std::fs::write(&path, b"beta").unwrap();
        let gate: TestReadGate = Arc::new((Mutex::new((false, false)), Condvar::new()));
        *TEST_READ_GATE.lock().unwrap() = Some(gate.clone());
        assert_eq!(worker.try_read_config(1, 2, path.clone(), Some(1)), Submit::Accepted);
        let mut state = gate.0.lock().unwrap();
        while !state.0 { state = gate.1.wait(state).unwrap(); }
        assert_eq!(worker.try_read_config(1, 3, path.clone(), Some(1)), Submit::Coalesced);
        state.1 = true;
        gate.1.notify_all();
        drop(state);

        assert!(matches!(next_result(&worker), WorkerResult::Config {
            revision: 3,
            snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Changed, .. }),
            ..
        }));
        worker.shutdown();
        *TEST_READ_GATE.lock().unwrap() = None;
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn coalesced_seed_replaced_by_error_does_not_leak_its_watch_slot() {
        let root = std::env::temp_dir().join(format!("s2-loader-watch-orphan-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.json");
        let second = root.join("second.json");
        std::fs::write(&first, b"alpha").unwrap();
        std::fs::write(&second, b"beta").unwrap();
        let mut policy = LoaderPolicy::default();
        policy.config_baseline_items = 1;
        let worker = LoaderWorker::start(policy).unwrap();

        assert_eq!(worker.try_read_config(1, 1, first.clone(), Some(1)), Submit::Accepted);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while worker.shared.state.lock().unwrap().results.is_empty() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        std::fs::remove_file(&first).unwrap();
        std::fs::create_dir(&first).unwrap();
        assert_eq!(worker.try_read_config(1, 2, first.clone(), Some(1)), Submit::Coalesced);
        assert!(matches!(next_result(&worker), WorkerResult::Config { snapshot: Err(_), .. }));
        worker.retire_config_watch(first, 1);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match worker.try_read_config(1, 1, second.clone(), Some(2)) {
                Submit::Accepted => break,
                Submit::Full => assert!(std::time::Instant::now() < deadline),
                other => panic!("unexpected submit after retirement: {other:?}"),
            }
            std::thread::yield_now();
        }
        let next = next_result(&worker);
        worker.shutdown();
        std::fs::remove_dir_all(root).unwrap();
        assert!(matches!(next, WorkerResult::Config {
            snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Seed, .. }), ..
        }), "abandoned seed occupies historical slot: {next:?}");
    }

    #[test]
    fn failed_superseding_read_discards_the_older_undelivered_proposal() {
        let policy = LoaderPolicy::default();
        let path = PathBuf::from("config.json");
        let alpha: Option<Arc<str>> = Some(Arc::from("alpha"));
        let mut baselines = ConfigBaselines::default();
        assert_eq!(baselines.compare_and_propose(&path, 1, 1, &alpha, &policy), WatchDelta::Seed);

        baselines.discard_superseded(&path, 1, 2);
        assert_eq!(baselines.bytes, 0);
        assert!(baselines.proposals.is_empty());
    }

    #[test]
    fn watched_generation_reserves_retirement_control_before_delivery_ack() {
        let root = std::env::temp_dir().join(format!("s2-loader-watch-reserve-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.json");
        let second = root.join("second.json");
        std::fs::write(&first, b"alpha").unwrap();
        std::fs::write(&second, b"beta").unwrap();
        let mut policy = LoaderPolicy::default();
        policy.request_items = 1;
        policy.result_items = 1;
        let worker = LoaderWorker::start(policy).unwrap();

        assert_eq!(worker.try_read_config(1, 1, first.clone(), Some(1)), Submit::Accepted);
        assert!(matches!(next_result(&worker), WorkerResult::Config {
            snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Seed, .. }), ..
        }));
        assert_eq!(worker.try_read_config(1, 1, second.clone(), Some(2)), Submit::Full);

        worker.retire_config_watch(first, 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match worker.try_read_config(1, 1, second.clone(), Some(2)) {
                Submit::Accepted => break,
                Submit::Full => assert!(std::time::Instant::now() < deadline),
                other => panic!("unexpected submit after retirement: {other:?}"),
            }
            std::thread::yield_now();
        }
        assert!(matches!(next_result(&worker), WorkerResult::Config {
            snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Seed, .. }), ..
        }));

        worker.shutdown();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn full_watch_control_partition_does_not_consume_transient_request_progress() {
        let path = std::env::temp_dir().join(format!("s2-loader-watch-partition-{}", std::process::id()));
        std::fs::write(&path, b"alpha").unwrap();
        let mut policy = LoaderPolicy::default();
        policy.request_items = 1;
        policy.result_items = 1;
        let worker = LoaderWorker::start(policy).unwrap();

        assert_eq!(worker.try_read_config(1, 1, path.clone(), Some(1)), Submit::Accepted);
        assert!(matches!(next_result(&worker), WorkerResult::Config {
            snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Seed, .. }), ..
        }));
        worker.resolve_config_watch(path.clone(), 1, 1, true);

        // The one-slot watch/control partition remains occupied for this long-lived watch, while
        // its independent one-slot transient partition is free for the next reread.
        assert_eq!(worker.try_read_config(1, 2, path.clone(), Some(1)), Submit::Accepted);
        assert!(matches!(next_result(&worker), WorkerResult::Config {
            snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Unchanged, .. }), ..
        }));

        worker.shutdown();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn impossible_watch_control_reservation_is_oversized_not_retryable_pressure() {
        let mut policy = LoaderPolicy::default();
        policy.request_bytes = 100;
        let worker = LoaderWorker::start(policy).unwrap();
        let path = PathBuf::from("x".repeat(30));
        assert!(Request::Config {
            epoch: 1,
            revision: 1,
            path: path.clone(),
            watch_generation: Some(1),
        }.request_weight() <= 100);
        assert_eq!(worker.try_read_config(1, 1, path, Some(1)), Submit::Oversized);
        worker.shutdown();
    }

    #[test]
    fn final_unwatch_retires_capacity_and_old_retire_cannot_erase_a_new_generation() {
        let root = std::env::temp_dir().join(format!("s2-loader-watch-retire-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.json");
        let second = root.join("second.json");
        std::fs::write(&first, b"a").unwrap();
        std::fs::write(&second, b"b").unwrap();
        let mut policy = LoaderPolicy::default();
        policy.config_baseline_items = 1;
        let worker = LoaderWorker::start(policy).unwrap();

        assert_eq!(worker.try_read_config(1, 1, first.clone(), Some(1)), Submit::Accepted);
        assert!(matches!(next_result(&worker), WorkerResult::Config {
            snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Seed, .. }), ..
        }));
        worker.resolve_config_watch(first.clone(), 1, 1, true);
        worker.retire_config_watch(first.clone(), 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match worker.try_read_config(1, 2, second.clone(), Some(2)) {
                Submit::Accepted => break,
                Submit::Full => assert!(std::time::Instant::now() < deadline),
                other => panic!("unexpected submit after retirement: {other:?}"),
            }
            std::thread::yield_now();
        }
        assert!(matches!(next_result(&worker), WorkerResult::Config {
            snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Seed, .. }), ..
        }), "retirement must release the only baseline slot");
        worker.resolve_config_watch(second.clone(), 2, 2, true);

        worker.retire_config_watch(second.clone(), 2);
        assert_eq!(worker.try_read_config(1, 3, second.clone(), Some(3)), Submit::Accepted);
        assert!(matches!(next_result(&worker), WorkerResult::Config {
            snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Seed, .. }), ..
        }));
        worker.resolve_config_watch(second.clone(), 3, 3, true);
        worker.retire_config_watch(second.clone(), 2);
        assert_eq!(worker.try_read_config(1, 4, second.clone(), Some(3)), Submit::Accepted);
        assert!(matches!(next_result(&worker), WorkerResult::Config {
            snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Unchanged, .. }), ..
        }), "an old-generation retire must not erase the new committed baseline");

        worker.shutdown();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retirement_control_progresses_while_the_data_result_queue_is_full() {
        let root = std::env::temp_dir().join(format!("s2-loader-watch-control-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.json");
        let second = root.join("second.json");
        std::fs::write(&first, b"a").unwrap();
        std::fs::write(&second, b"b").unwrap();
        let mut policy = LoaderPolicy::default();
        policy.request_items = 1;
        policy.result_items = 1;
        policy.config_baseline_items = 1;
        let worker = LoaderWorker::start(policy).unwrap();

        assert_eq!(worker.try_read_config(1, 1, first.clone(), Some(1)), Submit::Accepted);
        let _ = next_result(&worker);
        worker.resolve_config_watch(first.clone(), 1, 1, true);
        assert_eq!(worker.try_prepare(1, 1, root.join("missing.s2sp")), Submit::Accepted);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while worker.shared.state.lock().unwrap().results.is_empty() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }

        worker.retire_config_watch(first.clone(), 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while worker.shared.state.lock().unwrap().config_watches.contains_key(&first) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let _ = next_result(&worker);
        assert_eq!(worker.try_read_config(1, 2, second, Some(2)), Submit::Accepted);
        assert!(matches!(next_result(&worker), WorkerResult::Config {
            snapshot: Ok(ConfigSnapshot { watch: WatchDelta::Seed, .. }), ..
        }));

        worker.shutdown();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn slow_read_barrier_never_blocks_main_side_polling() {
        let path = std::env::temp_dir().join(format!("s2-loader-slow-{}", std::process::id()));
        std::fs::write(
            &path,
            archive(
                r#"{"id":"@demo/slow","version":"1","apiVersion":"2.x"}"#,
                b"module.exports={};",
            ),
        )
        .unwrap();
        let gate: TestReadGate = std::sync::Arc::new((Mutex::new((false, false)), Condvar::new()));
        *TEST_READ_GATE.lock().unwrap() = Some(gate.clone());
        let worker = LoaderWorker::start(LoaderPolicy::default()).unwrap();
        assert_eq!(worker.try_prepare(7, 1, path.clone()), Submit::Accepted);
        let mut state = gate.0.lock().unwrap();
        while !state.0 {
            state = gate.1.wait(state).unwrap();
        }
        assert!(
            worker.try_result().is_none(),
            "main-side poll must remain nonblocking"
        );
        state.1 = true;
        gate.1.notify_all();
        drop(state);
        assert!(matches!(
            next_result(&worker),
            WorkerResult::Plugin { epoch: 7, .. }
        ));
        worker.shutdown();
        *TEST_READ_GATE.lock().unwrap() = None;
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn shutdown_joins_a_slow_regular_file_read_without_detaching() {
        let path = std::env::temp_dir().join(format!("s2-loader-join-{}", std::process::id()));
        std::fs::write(
            &path,
            archive(
                r#"{"id":"@demo/join","version":"1","apiVersion":"2.x"}"#,
                b"module.exports={};",
            ),
        )
        .unwrap();
        let gate: TestReadGate = std::sync::Arc::new((Mutex::new((false, false)), Condvar::new()));
        *TEST_READ_GATE.lock().unwrap() = Some(gate.clone());
        let worker = LoaderWorker::start(LoaderPolicy::default()).unwrap();
        assert_eq!(worker.try_prepare(8, 1, path.clone()), Submit::Accepted);
        let mut state = gate.0.lock().unwrap();
        while !state.0 {
            state = gate.1.wait(state).unwrap();
        }
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let shutdown = std::thread::spawn(move || {
            worker.shutdown();
            done_tx.send(()).unwrap();
        });
        assert!(
            done_rx.try_recv().is_err(),
            "shutdown must still own and join the worker"
        );
        state.1 = true;
        gate.1.notify_all();
        drop(state);
        done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        shutdown.join().unwrap();
        *TEST_READ_GATE.lock().unwrap() = None;
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn scan_overflow_fails_the_entire_snapshot_instead_of_returning_a_prefix() {
        let root = std::env::temp_dir().join(format!("s2-loader-overflow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.s2sp"), b"a").unwrap();
        std::fs::write(root.join("b.s2sp"), b"b").unwrap();
        let p = LoaderPolicy {
            scan_candidates: 1,
            ..LoaderPolicy::default()
        };
        assert!(
            scan_plugins(&root, &p)
                .unwrap_err()
                .contains("candidate limit")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stable_read_rejects_a_file_larger_than_the_absolute_capacity() {
        let path = std::env::temp_dir().join(format!("s2-loader-large-{}", std::process::id()));
        std::fs::write(&path, b"12345").unwrap();
        assert!(stable_read(&path, 4).unwrap_err().contains("byte limit 4"));
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_opened_nonblocking_and_refused_as_non_regular() {
        let path = std::env::temp_dir().join(format!("s2-loader-fifo-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let err = stable_read(&path, 32).unwrap_err();
        assert!(err.contains("not a regular file"), "{err}");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn malformed_utf8_plugin_member_is_named() {
        let bytes = archive(
            r#"{"id":"@demo/a","version":"1.0.0","apiVersion":"2.x"}"#,
            &[0xff, 0xfe],
        );
        let err = parse_s2sp(&bytes, ParseLimits::default()).unwrap_err();
        assert!(err.contains("plugin.js") && err.contains("UTF-8"), "{err}");
    }

    #[test]
    fn invalid_optional_gamedata_remains_absent() {
        let bytes = archive_with_gamedata(
            r#"{"id":"@demo/a","version":"1.0.0","apiVersion":"2.x"}"#,
            b"module.exports={};",
            &[0xff, 0xfe],
        );
        let (_, _, gamedata) = parse_s2sp(&bytes, ParseLimits::default()).unwrap();
        assert!(gamedata.is_none());
    }

    #[test]
    fn archive_entry_count_is_bounded_before_member_extraction() {
        let bytes = archive(
            r#"{"id":"@demo/a","version":"1.0.0","apiVersion":"2.x"}"#,
            b"module.exports={};",
        );
        let limits = ParseLimits {
            zip_entries: 1,
            ..ParseLimits::default()
        };
        assert!(
            parse_s2sp(&bytes, limits)
                .unwrap_err()
                .contains("entry limit 1")
        );
    }
}
