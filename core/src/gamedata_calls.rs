//! Plugin-declared engine calls — the core's half of the plugin-gamedata slice (spec §10, the
//! "Core" row): the per-plugin descriptor registry, the two-part authorization gate, and the SysV
//! argument classification the invoke native marshals against.
//!
//! BOUNDARY (spec §10). Every name in a descriptor — target kind, module, byte pattern, resolver
//! strategy, class, field, the whole `validate` object — is an OPAQUE plugin-supplied string that crosses core
//! untouched, exactly the discipline `__s2_schema_offset` already follows. No game identifier is
//! compiled into core; `scripts/check-core-boundary.sh` stays green.
//!
//! DEGRADE PER-DESCRIPTOR (spec §12). Every failure carries a NAMED reason and disables exactly one
//! call: the plugin still loads, its other calls still work, and the framework never crashes. The
//! reason is what `Engine.status(name)` reports, so an operator can always tell WHY a command is
//! unavailable instead of guessing.
//!
//! Registration is NOT a native. Core parses the packed `gamedata.json` (`read_s2sp`'s third
//! element) at plugin load and registers every entry itself, so JS can never supply a declaration —
//! it only asks `ready` / `status` / `invoke` by name.

use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::c_char;

/// The capability a plugin must declare in its manifest AND be operator-allow-listed for (spec §6).
pub(crate) const PERMISSION: &str = "engine:calls";

/// Sentinel prefix for the RESERVED owner ids the runtime registers under (A5b, spec §9.1b).
///
/// The game package's descriptors are first-party runtime, shipped in the same zip as core and the
/// shim, and are consequently exempt from the `engine:calls` allow-list — the natives they replace
/// are unconditionally callable from any plugin today, so gating them on an operator's list would
/// be a new restriction, not a preserved one. That exemption is only safe if the identity carrying
/// it cannot be CLAIMED, so two independent things hold it shut:
///
///   1. this prefix contains `:`, which no npm package name may contain, so no legitimate plugin id
///      collides with it; and
///   2. `loader::read_s2sp` REFUSES any `.s2sp` whose manifest id carries the prefix, and
///      `register_plugin` below refuses to register one — because a manifest id is an arbitrary
///      JSON string that nothing validates against the npm grammar, so (1) alone is a naming
///      convention, not a gate.
///
/// The exemption itself is never derived from the id string: it is a parameter passed by the
/// registration entry point (`register_game_package`), so even a spoofed id would not carry it.
pub(crate) const RESERVED_OWNER_PREFIX: &str = "game-package:";

/// The reserved owner id for a game package (`@s2script/cs2` → `game-package:@s2script/cs2`).
/// Engine-generic: the package name comes from the shim, never from a constant in core.
pub(crate) fn reserved_owner_id(package: &str) -> String {
    format!("{}{}", RESERVED_OWNER_PREFIX, package)
}

/// True for an id in the reserved namespace — i.e. an identity a `.s2sp` must never hold.
pub(crate) fn is_reserved_owner(id: &str) -> bool {
    id.starts_with(RESERVED_OWNER_PREFIX)
}

/// The platform id whose nested details the runtime consumes (spec §5, Global Constraints). Exactly
/// one key is selected by each native build; plugin archives may carry both keys.
#[cfg(target_os = "linux")]
const PLATFORM: &str = "linuxsteamrt64";
#[cfg(target_os = "windows")]
const PLATFORM: &str = "windows64";
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!("s2script-core supports only Linux and Windows native runtimes");

/// Per-GP-slot arg kind bytes. MUST match `engine_calls.cpp`'s `kArg*` enum — this is an ABI, not a
/// convention.
pub(crate) const GP_SCALAR: u8 = 0;
pub(crate) const GP_ENTITY: u8 = 1;
pub(crate) const GP_STRING: u8 = 2;
pub(crate) const GP_VECTOR: u8 = 3;

/// Return-kind codes. MUST match `engine_calls.cpp`'s `kRet*` enum.
pub(crate) const RET_VOID: i32 = 0;
pub(crate) const RET_BOOL: i32 = 1;
pub(crate) const RET_INT: i32 = 2;
pub(crate) const RET_FLOAT: i32 = 3;
pub(crate) const RET_ENTITY: i32 = 4;

/// The shim's "no entity" marker for `RET_ENTITY` (`kInvalidEntityHandle` in `engine_calls.cpp`).
/// MUST stay in sync with it, and MUST NOT be 0 — zero decodes to the legal (index 0, serial 0), so
/// an absent entity would be indistinguishable from a live handle to entity slot 0.
pub(crate) const INVALID_ENTITY_HANDLE: u32 = 0xFFFF_FFFF;

/// SysV budget (spec §4): `this` consumes the first of the six GP argument registers, leaving 5;
/// there are 8 xmm argument registers. The SDK fails the BUILD past either bound — these are the
/// runtime backstop for a hand-rolled or older `.s2sp`.
/// Declared integer-class args. Mirrors `kMaxGpArgs` in `engine_calls.cpp` — this is an ABI.
///
/// Six is the SysV *register* count, not a call-shape limit: further integer args spill to the
/// stack, which the shim's prototypes now cover. The budget is 9 declared args plus the optional
/// receiver, so a static factory taking seven can be declared.
const MAX_GP_ARGS: usize = 9;
const MAX_FP_ARGS: usize = 8;

/// Buffer size for a degrade reason written back by the shim.
const REASON_CAP: usize = 256;

/// The numeric return code for a return-vocabulary name, or `None` for anything outside it.
pub(crate) fn ret_code(returns: &str) -> Option<i32> {
    Some(match returns {
        "void" => RET_VOID,
        "bool" => RET_BOOL,
        "int" => RET_INT,
        "float" => RET_FLOAT,
        "entity" => RET_ENTITY,
        _ => return None,
    })
}

/// The GP-slot kind byte for an integer-class arg kind. `None` for `float` (the float class) and for
/// an unknown kind — a caller that must tell those apart checks `arg == "float"` first. The single
/// source of truth for the arg vocabulary, shared by `classify_args` and the invoke native's
/// marshaller so the two can never drift.
pub(crate) fn gp_kind_of(arg: &str) -> Option<u8> {
    Some(match arg {
        // `bool` is widened to a 0/1 integer by the caller; both are plain scalars to the callee.
        "bool" | "int" => GP_SCALAR,
        "entity" => GP_ENTITY,
        "string" => GP_STRING,
        "vector" => GP_VECTOR,
        _ => return None,
    })
}

/// Split an arg list into its two SysV register sequences: the per-GP-slot kind bytes and the count
/// of float slots, **preserving order within each class**. Positional interleaving between the
/// classes is irrelevant to the callee (integer-class and float-class args are assigned from two
/// independent register sequences), which is what makes one fixed max-arity thunk shim-side able to
/// call the whole closed v1 vocabulary.
///
/// An arg kind outside the vocabulary is skipped here; `prepare` rejects it with a named reason
/// BEFORE classification, so a live descriptor never reaches this with an unknown kind.
pub(crate) fn classify_args(args: &[String]) -> (Vec<u8>, usize) {
    let mut gp_kinds = Vec::new();
    let mut fp_count = 0usize;
    for a in args {
        if a == "float" {
            fp_count += 1;
        } else if let Some(k) = gp_kind_of(a) {
            gp_kinds.push(k);
        }
    }
    (gp_kinds, fp_count)
}

/// Everything the invoke native needs, cloned out of the registry so the registry borrow is released
/// BEFORE the engine call runs. That matters: an engine function may synchronously fire an entity
/// output / game event, which dispatches back into JS, which may call `Engine.call` again — holding
/// the `RefCell` across the call would double-borrow.
#[derive(Clone)]
pub(crate) struct InvokePlan {
    /// The shim's call-table id (`S2_EngineCallResolve`'s return).
    pub call_id: i32,
    /// The declared arg kinds, in author order (the marshaller walks these against the JS args).
    pub args: Vec<String>,
    /// `RET_*`.
    pub ret_code: i32,
    /// `receiver.via` — an opaque (class, field) pair resolved LAZILY at first invoke through the
    /// cached schema-offset resolver (spec §11: schema resolves at map-live, not at Load).
    pub via: Option<(String, String)>,
    /// `receiver.kind: "none"` — a static/free function. The callable takes no `self`, and the
    /// invoke passes the receiverless sentinel instead of an entity pair.
    pub receiverless: bool,
}

/// One registered descriptor. `Ready` carries the resolved plan; `Degraded` carries the named reason
/// `Engine.status()` reports.
enum Descriptor {
    Ready {
        plan: InvokePlan,
        /// A named reason recorded when the LAZY `via` hop failed to resolve (spec §11: "a miss
        /// degrades that invocation and flips `Engine.status(name)` to a named reason"). Kept
        /// separate from `Degraded` deliberately: the descriptor itself resolved, so this is
        /// recoverable — it clears the moment the schema offset resolves (i.e. at map-live).
        via_miss: Option<String>,
    },
    Degraded(String),
}

/// `(plugin id, call name)` → descriptor. Per-plugin by key, so `drop_plugin` is the whole teardown.
pub(crate) struct CallRegistry {
    calls: HashMap<(String, String), Descriptor>,
}

impl CallRegistry {
    pub(crate) fn new() -> Self {
        Self { calls: HashMap::new() }
    }

    /// Register one declared call. Order of checks (each failure is a named degrade, never a load
    /// failure — spec §6/§12):
    ///   1. the operator allow-list (default-deny),
    ///   2. the engine op's presence,
    ///   3. the descriptor's own shape + the resolve op against the live binary.
    ///
    /// `decl_json` is a FLATTENED descriptor: the platform-nested details (`signatures[name][platform]`
    /// / `target[platform]`) have already been lifted into `target` by `flatten_decl`.
    ///
    /// `permission_exempt` skips check (1) for FIRST-PARTY RUNTIME descriptors (the game package —
    /// see `RESERVED_OWNER_PREFIX`). It is a parameter rather than something derived from
    /// `plugin_id`, so the exemption travels with the CALL SITE that is entitled to it and a
    /// spoofed id can never pick it up.
    pub(crate) fn register(
        &mut self,
        plugin_id: &str,
        call_name: &str,
        decl_json: &str,
        ops_available: bool,
        permission_exempt: bool,
    ) {
        match prepare(plugin_id, decl_json, ops_available, permission_exempt) {
            Ok(plan) => {
                // An `unsafe`-capability call reaching a real engine function is worth one audit line.
                crate::v8host::log_warn(&format!(
                    "[engine-calls] '{}' armed '{}' (call id {})",
                    plugin_id, call_name, plan.call_id
                ));
                self.calls.insert(
                    (plugin_id.to_string(), call_name.to_string()),
                    Descriptor::Ready { plan, via_miss: None },
                );
            }
            Err(reason) => self.degrade(plugin_id, call_name, &reason),
        }
    }

    /// Record a descriptor as degraded with a named reason, WARNing once (registration runs once per
    /// plugin load, so "once per load" is "once").
    pub(crate) fn degrade(&mut self, plugin_id: &str, call_name: &str, reason: &str) {
        crate::v8host::log_warn(&format!(
            "WARN: [engine-calls] '{}' call '{}' unavailable: {}",
            plugin_id, call_name, reason
        ));
        self.calls.insert(
            (plugin_id.to_string(), call_name.to_string()),
            Descriptor::Degraded(reason.to_string()),
        );
    }

    /// The shim call id of a resolved descriptor, or `None` when unknown/degraded.
    pub(crate) fn call_id(&self, plugin_id: &str, name: &str) -> Option<i32> {
        match self.calls.get(&(plugin_id.to_string(), name.to_string())) {
            Some(Descriptor::Ready { plan, .. }) => Some(plan.call_id),
            _ => None,
        }
    }

    /// True iff the descriptor passed every LOAD-time gate — which is exactly what `Engine.call()`
    /// keys callable-or-null on. A pending `via` hop does NOT make it false (spec §11: the sub-object
    /// offset is not decidable at load, so a `via` descriptor legitimately returns a callable that
    /// may no-op until schema is live).
    pub(crate) fn is_ready(&self, plugin_id: &str, name: &str) -> bool {
        // Deliberately defined AS "it has a call id": readiness and callability are the same fact,
        // and routing both through `call_id` keeps them from drifting apart.
        self.call_id(plugin_id, name).is_some()
    }

    /// `"available"`, or the named reason it is not.
    pub(crate) fn status(&self, plugin_id: &str, name: &str) -> String {
        match self.calls.get(&(plugin_id.to_string(), name.to_string())) {
            None => "not declared in this plugin's gamedata".to_string(),
            Some(Descriptor::Degraded(reason)) => reason.clone(),
            Some(Descriptor::Ready { via_miss: Some(reason), .. }) => reason.clone(),
            Some(Descriptor::Ready { .. }) => "available".to_string(),
        }
    }

    /// The marshalling plan for a resolved descriptor (cloned — see `InvokePlan`).
    pub(crate) fn plan(&self, plugin_id: &str, name: &str) -> Option<InvokePlan> {
        match self.calls.get(&(plugin_id.to_string(), name.to_string())) {
            Some(Descriptor::Ready { plan, .. }) => Some(plan.clone()),
            _ => None,
        }
    }

    /// Record / clear the lazy `via`-hop reason (see `Descriptor::Ready::via_miss`).
    pub(crate) fn set_via_miss(&mut self, plugin_id: &str, name: &str, reason: Option<String>) {
        if let Some(Descriptor::Ready { via_miss, .. }) =
            self.calls.get_mut(&(plugin_id.to_string(), name.to_string()))
        {
            if *via_miss != reason {
                *via_miss = reason;
            }
        }
    }

    /// Ledger teardown for one plugin: drop EVERY descriptor it declared. Called from the same
    /// teardown that walks the ledger, so a reload always re-resolves from scratch (spec §12).
    pub(crate) fn drop_plugin(&mut self, plugin_id: &str) {
        self.calls.retain(|(pid, _), _| pid != plugin_id);
    }
}

/// Validate + resolve one flattened descriptor. Returns the plan, or the named degrade reason.
fn prepare(
    plugin_id: &str,
    decl_json: &str,
    ops_available: bool,
    permission_exempt: bool,
) -> Result<InvokePlan, String> {
    // (1) Authorization (spec §6). Declaration in the manifest is necessary but NOT sufficient: the
    // operator's allow-list decides, and an unloaded/absent allow-list denies everything.
    // First-party runtime descriptors (the game package) are exempt — see `RESERVED_OWNER_PREFIX`.
    if !permission_exempt && !crate::loader::permission_allowed(plugin_id, PERMISSION) {
        return Err(format!(
            "not permitted by the operator allow-list (add \"{}\" under \"{}\" in configs/permissions.json)",
            plugin_id, PERMISSION
        ));
    }
    // (2) The engine op itself (no shim / an older shim → every descriptor degrades, plugin loads).
    if !ops_available {
        return Err("engine op unavailable".to_string());
    }

    // (3) Shape, then the live resolve.
    let decl: serde_json::Value =
        serde_json::from_str(decl_json).map_err(|e| format!("malformed descriptor: {}", e))?;

    // Receiver: `kind` is a TAGGED field, so kinds are added additively rather than by redesigning
    // the format (spec §4).
    //
    //   "entity" — the default: a books-gated (index, serial) pair supplies `this`.
    //   "none"   — a STATIC/free function with no `this` at all. The generated callable takes no
    //              `self`, and the first declared arg lands where the receiver would have.
    //
    // A receiverless descriptor must not also carry `via`: a sub-object hop is a hop FROM a
    // receiver, so the pair is contradictory and is rejected rather than silently ignored.
    let receiver = decl.get("receiver");
    let rkind = receiver.and_then(|r| r.get("kind")).and_then(|v| v.as_str()).unwrap_or("entity");
    if rkind != "entity" && rkind != "none" {
        return Err(format!("unsupported receiver kind '{}'", rkind));
    }
    let receiverless = rkind == "none";
    if receiverless && receiver.and_then(|r| r.get("via")).is_some() {
        return Err("receiver.kind 'none' cannot carry a 'via' sub-object hop".to_string());
    }
    let via = receiver.and_then(|r| r.get("via")).and_then(|v| {
        let class = v.get("class").and_then(|x| x.as_str())?;
        let field = v.get("field").and_then(|x| x.as_str())?;
        Some((class.to_string(), field.to_string()))
    });

    let args: Vec<String> = decl
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect())
        .unwrap_or_default();
    for a in &args {
        if a != "float" && gp_kind_of(a).is_none() {
            return Err(format!("unknown arg kind '{}'", a));
        }
    }
    let (gp_kinds, fp_count) = classify_args(&args);
    if gp_kinds.len() > MAX_GP_ARGS {
        return Err(format!(
            "too many integer-class args ({} > {})",
            gp_kinds.len(),
            MAX_GP_ARGS
        ));
    }
    if fp_count > MAX_FP_ARGS {
        return Err(format!("too many float args ({} > {})", fp_count, MAX_FP_ARGS));
    }

    let returns = decl.get("returns").and_then(|v| v.as_str()).unwrap_or("void");
    let ret = ret_code(returns).ok_or_else(|| format!("unknown return kind '{}'", returns))?;

    let target = decl.get("target").ok_or_else(|| "descriptor has no target".to_string())?;
    let call_id = resolve_target(target)?;

    Ok(InvokePlan { call_id, args, ret_code: ret, via, receiverless })
}

/// Call the shim's resolve op with the descriptor's opaque strings. Returns the call id, or the
/// shim's own named reason (signature miss / ambiguous pattern / prologue mismatch / slot outside
/// `.text` / …) verbatim — core never re-words it, so the log and `Engine.status()` say exactly what
/// the resolver decided.
///
/// `pub(crate)` for `gamedata_hooks`: a declarative inbound hook resolves its target through THIS
/// path and no other (spec §6, "No second resolver"). Both validators, the `.text` range check and
/// the uniqueness rule are therefore identical for a detour and for a call.
pub(crate) fn resolve_target(target: &serde_json::Value) -> Result<i32, String> {
    let Some(func) = crate::v8host::engine_ops().and_then(|o| o.engine_call_resolve) else {
        return Err("engine op unavailable".to_string());
    };
    let s = |key: &str| target.get(key).and_then(|v| v.as_str()).unwrap_or("");
    // A descriptor string with an interior NUL can never be a valid C string — fail closed rather
    // than truncate it into a DIFFERENT pattern/class name.
    let cstr = |v: &str| {
        CString::new(v).map_err(|_| "descriptor string contains an interior NUL".to_string())
    };
    let kind = cstr(s("kind"))?;
    let module = cstr(s("module"))?;
    let pattern = cstr(s("pattern"))?;
    let resolve = cstr(s("resolve"))?;
    let class_name = cstr(s("class"))?;
    // The WHOLE `validate` object, verbatim. Core does not know the validator vocabulary and must
    // not learn it: it is closed in the shim, which is what dispatches on it, so a mistyped
    // validator (`vtable_member`) degrades that descriptor by name instead of being quietly dropped
    // on the way across — which is what cherry-picking one known key here used to do. serde_json
    // never emits a literal NUL, so `cstr` cannot fail on this one.
    let validate = cstr(&target.get("validate").map(|v| v.to_string()).unwrap_or_default())?;
    // A missing/oversized index becomes -1, which the shim rejects with its own named reason.
    let index = target
        .get("index")
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok())
        .unwrap_or(-1);

    let mut reason = vec![0u8; REASON_CAP];
    let id = func(
        kind.as_ptr(),
        module.as_ptr(),
        pattern.as_ptr(),
        resolve.as_ptr(),
        class_name.as_ptr(),
        index,
        validate.as_ptr(),
        reason.as_mut_ptr() as *mut c_char,
        REASON_CAP as i32,
    );
    if id < 0 {
        let text = reason_string(&reason);
        return Err(if text.is_empty() { "descriptor did not resolve".to_string() } else { text });
    }
    Ok(id)
}

/// Read the shim's NUL-terminated reason out of the buffer (lossy; never panics on non-UTF-8).
fn reason_string(buf: &[u8]) -> String {
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).trim().to_string()
}

/// Lift the platform-nested details of a descriptor's target into the target itself, so the registry
/// (and the shim) never has to know the gamedata's nesting shape (spec §5).
///
/// - `signature`: `signatures[target.name][PLATFORM]` supplies `module`/`pattern`/`resolve`.
///   An inline `pattern` on the target wins (keeps this idempotent and accepts a hand-written
///   flattened descriptor).
/// - `vtable`: `target[PLATFORM]` supplies `index` and `validate`.
///
/// `pub(crate)` for `gamedata_hooks`: a hook's `target` is the SAME grammar (it references the same
/// `signatures` section, by name, with the same `validate` co-location rule), so it is flattened by
/// the same function rather than by a second copy that could drift on the next platform key.
pub(crate) fn flatten_decl(
    decl: &serde_json::Value,
    signatures: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<String, String> {
    flatten_decl_for_platform(decl, signatures, PLATFORM)
}

fn flatten_decl_for_platform(
    decl: &serde_json::Value,
    signatures: Option<&serde_json::Map<String, serde_json::Value>>,
    platform: &str,
) -> Result<String, String> {
    let mut out = decl.clone();
    let target = out
        .get_mut("target")
        .and_then(|t| t.as_object_mut())
        .ok_or_else(|| "descriptor has no target".to_string())?;
    let kind = target.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
    match kind.as_str() {
        "signature" => {
            let already_inline =
                target.get("pattern").and_then(|v| v.as_str()).is_some_and(|p| !p.is_empty());
            if !already_inline {
                let name = target.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if name.is_empty() {
                    return Err("signature target has no name".to_string());
                }
                let spec = signatures
                    .and_then(|m| m.get(&name))
                    .and_then(|v| v.get(platform))
                    .ok_or_else(|| {
                        format!("no '{}' entry for signature '{}'", platform, name)
                    })?;
                for key in ["module", "pattern", "resolve"] {
                    if let Some(v) = spec.get(key) {
                        target.insert(key.to_string(), v.clone());
                    }
                }
                // `validate` is authored NEXT TO the pattern it guards, and is lifted with it.
                // The override channel is why: `custom/*.jsonc` replaces at the NAMED-ENTRY level,
                // and the entry an operator replaces after a game update is the SIGNATURE — a new
                // pattern. A validator whose offsets live in another section would then be checked
                // against the shipped instruction layout, which the new pattern has just made
                // meaningless: either a correct hot-fix is spuriously rejected, or worse, it
                // coincidentally passes. Co-locating them makes one override replace both,
                // atomically. An inline `target.validate` still wins, exactly as an inline
                // `pattern` does.
                if !target.contains_key("validate") {
                    if let Some(v) = spec.get("validate") {
                        target.insert("validate".to_string(), v.clone());
                    }
                }
            }
        }
        "vtable" => {
            let plat = target
                .get(platform)
                .cloned()
                .ok_or_else(|| format!("vtable target has no '{}' entry", platform))?;
            for key in ["index", "validate"] {
                if let Some(v) = plat.get(key) {
                    target.insert(key.to_string(), v.clone());
                }
            }
        }
        other => return Err(format!("unknown target kind '{}'", other)),
    }
    Ok(out.to_string())
}

// ---------------------------------------------------------------------------
// The host-side registry instance + the accessors core's natives / loader use.
//
// thread_local like every other core registry: the game thread owns V8, the loader, and every
// engine op.
// ---------------------------------------------------------------------------

thread_local! {
    static REGISTRY: std::cell::RefCell<CallRegistry> =
        std::cell::RefCell::new(CallRegistry::new());

    /// The reserved owner id of the registered game package, set by
    /// `s2script_core_register_package_gamedata`. `None` on a host with no game package (every unit
    /// test, and any future headless embedding) — the game-scoped natives then report a named
    /// reason rather than silently keying on an empty id.
    static GAME_PACKAGE_OWNER: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Register every `calls` entry of a plugin's packed `gamedata.json` (`read_s2sp`'s third element).
/// Called from the loader at plugin load, BEFORE the plugin's factory runs — the factory's
/// `Engine.call(name)` must see a resolved (or named-degraded) descriptor.
///
/// Degrade-never-crash: a malformed member WARNs once and registers nothing (every call then reports
/// "not declared"); a gamedata with no `calls` section is the normal case and does nothing.
pub(crate) fn register_plugin(plugin_id: &str, gamedata_json: &str) {
    // A `.s2sp` may never register under a reserved owner id: that identity is permission-exempt,
    // and its descriptors are the game package's. `loader::read_s2sp` refuses such a manifest
    // outright, so reaching here means either a new load path that skipped that door or a direct
    // caller — both worth a loud line and a refusal rather than a silent privilege grant.
    if is_reserved_owner(plugin_id) {
        crate::v8host::log_warn(&format!(
            "WARN: [engine-calls] REFUSED a plugin registration under the reserved owner id '{}' — \
             that identity is the runtime's, not a plugin's; no descriptors registered",
            plugin_id
        ));
        return;
    }
    register_owner(plugin_id, gamedata_json, /*permission_exempt=*/ false);
}

/// Register the GAME PACKAGE's declared calls from the merged gamedata the shim already produced
/// for that owner (spec §9.1b). Called once at Load through
/// `s2script_core_register_package_gamedata`, with `package` the same injected-package name the
/// shim passes to `s2script_core_register_package` (e.g. `@s2script/cs2`) — core never names a game.
///
/// Descriptors land under `reserved_owner_id(package)`, which no `.s2sp` can hold, and are
/// permission-exempt: they are first-party runtime shipped in the same zip as core, replacing
/// natives that are unconditionally callable from any plugin today.
pub(crate) fn register_game_package(package: &str, gamedata_json: &str) {
    let owner = reserved_owner_id(package);
    // Idempotent: a re-register (a second Load in one process) replaces the previous view whole
    // rather than leaving descriptors from a gamedata tree that is no longer on disk.
    REGISTRY.with(|r| r.borrow_mut().drop_plugin(&owner));
    // The owner id is recorded even when the tree declares nothing, so an ask reports the accurate
    // "not declared" rather than the game-scoped natives' "no game package registered".
    GAME_PACKAGE_OWNER.with(|o| *o.borrow_mut() = Some(owner.clone()));
    // An owner with neither `signatures` nor `calls` serialises to an EMPTY string shim-side, which
    // is the normal state until A5b moves the first descriptor across. Nothing to parse, and
    // nothing worth a WARN.
    if gamedata_json.trim().is_empty() {
        return;
    }
    register_owner(&owner, gamedata_json, /*permission_exempt=*/ true);
}

/// The reserved owner id the game-package-scoped natives key on, or `None` if no game package has
/// registered gamedata.
pub(crate) fn game_package_owner() -> Option<String> {
    GAME_PACKAGE_OWNER.with(|o| o.borrow().clone())
}

/// The shared body of both registration entry points: parse, flatten, register each named entry.
fn register_owner(owner_id: &str, gamedata_json: &str, permission_exempt: bool) {
    let gd: serde_json::Value = match serde_json::from_str(gamedata_json) {
        Ok(v) => v,
        Err(e) => {
            crate::v8host::log_warn(&format!(
                "WARN: [engine-calls] '{}': unreadable gamedata.json ({}) - no declared calls registered",
                owner_id, e
            ));
            return;
        }
    };
    let Some(calls) = gd.get("calls").and_then(|v| v.as_object()) else { return };
    let signatures = gd.get("signatures").and_then(|v| v.as_object());
    // One op probe for the whole batch: a null resolve op degrades every descriptor identically.
    let ops_available = crate::v8host::engine_ops().and_then(|o| o.engine_call_resolve).is_some();
    // Deterministic order so the boot log reads the same on every start.
    let mut names: Vec<&String> = calls.keys().collect();
    names.sort();
    for name in names {
        match flatten_decl(&calls[name], signatures) {
            Ok(decl) => REGISTRY.with(|r| {
                r.borrow_mut().register(owner_id, name, &decl, ops_available, permission_exempt)
            }),
            Err(reason) => REGISTRY.with(|r| r.borrow_mut().degrade(owner_id, name, &reason)),
        }
    }
}

/// Ledger teardown: drop every descriptor this plugin declared (unload / reload).
pub(crate) fn drop_plugin(plugin_id: &str) {
    REGISTRY.with(|r| r.borrow_mut().drop_plugin(plugin_id));
}

/// Backs `__s2_engine_call_ready`.
pub(crate) fn is_ready(plugin_id: &str, name: &str) -> bool {
    REGISTRY.with(|r| r.borrow().is_ready(plugin_id, name))
}

/// Backs `__s2_engine_call_receiverless` — whether the callable should take a leading `self`.
/// False for an unknown or degraded descriptor, which is the safe default (the receiver form is
/// what every pre-existing descriptor uses).
pub(crate) fn is_receiverless(plugin_id: &str, name: &str) -> bool {
    REGISTRY.with(|r| r.borrow().plan(plugin_id, name).map(|p| p.receiverless).unwrap_or(false))
}

/// Backs `__s2_engine_call_status`.
pub(crate) fn status(plugin_id: &str, name: &str) -> String {
    REGISTRY.with(|r| r.borrow().status(plugin_id, name))
}

/// Backs `__s2_engine_call_invoke` — the plan is CLONED so the borrow is released before the call.
pub(crate) fn plan(plugin_id: &str, name: &str) -> Option<InvokePlan> {
    REGISTRY.with(|r| r.borrow().plan(plugin_id, name))
}

/// Record / clear the lazy `via`-hop degrade reason for a resolved descriptor.
pub(crate) fn set_via_miss(plugin_id: &str, name: &str, reason: Option<String>) {
    REGISTRY.with(|r| r.borrow_mut().set_via_miss(plugin_id, name, reason));
}

// ---------------------------------------------------------------------------
// Tests (written FIRST — Task 7 step 1).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_degrades_when_permission_denied() {
        // The allow-list is HOST-GLOBAL and the whole crate's tests share one process (single-
        // threaded, name-ordered), so a sibling test may already have loaded one. An allow-list that
        // does not NAME this plugin is the same DENY the never-loaded (default-deny) posture
        // produces, and is order-independent; `loader::permission_is_default_deny` covers the
        // never-loaded case itself.
        crate::loader::load_permissions_from_str(r#"{"engine:calls":[]}"#).unwrap();
        let mut reg = CallRegistry::new();
        reg.register("@demo/gd", "ignite", &decl_json(), /*ops_available=*/ true, /*exempt=*/ false);
        assert!(reg.call_id("@demo/gd", "ignite").is_none());
        assert!(reg.status("@demo/gd", "ignite").contains("not permitted"));
    }

    #[test]
    fn registry_degrades_when_ops_absent() {
        crate::loader::load_permissions_from_str(r#"{"engine:calls":["@demo/gd"]}"#).unwrap();
        let mut reg = CallRegistry::new();
        reg.register("@demo/gd", "ignite", &decl_json(), /*ops_available=*/ false, /*exempt=*/ false);
        assert!(reg.call_id("@demo/gd", "ignite").is_none());
        assert_eq!(reg.status("@demo/gd", "ignite"), "engine op unavailable");
    }

    #[test]
    fn status_of_unknown_call_is_named() {
        let reg = CallRegistry::new();
        assert!(reg.status("@demo/gd", "nope").contains("not declared"));
    }

    #[test]
    fn unload_drops_a_plugins_descriptors() {
        crate::loader::load_permissions_from_str(r#"{"engine:calls":["@demo/gd"]}"#).unwrap();
        let mut reg = CallRegistry::new();
        reg.register("@demo/gd", "ignite", &decl_json(), true, false);
        reg.drop_plugin("@demo/gd");
        assert!(reg.status("@demo/gd", "ignite").contains("not declared"));
    }

    #[test]
    fn arg_classification_splits_int_and_float_slots() {
        // ["float","int","float"] -> 1 GP slot, 2 float slots, preserving per-class order.
        let (gp_kinds, fp_count) = classify_args(&["float".into(), "int".into(), "float".into()]);
        assert_eq!(gp_kinds.len(), 1);
        assert_eq!(fp_count, 2);
    }

    /// Order within each class is what SysV assignment depends on — assert it explicitly.
    #[test]
    fn arg_classification_preserves_order_within_each_class() {
        let (gp_kinds, fp_count) = classify_args(&[
            "string".into(),
            "float".into(),
            "entity".into(),
            "vector".into(),
            "float".into(),
        ]);
        assert_eq!(gp_kinds, vec![GP_STRING, GP_ENTITY, GP_VECTOR]);
        assert_eq!(fp_count, 2);
    }

    /// The signatures section is lifted into the target, so the shim only ever sees a flat descriptor.
    #[test]
    fn flatten_lifts_the_platform_signature_entry_into_the_target() {
        let gd: serde_json::Value = serde_json::from_str(
            r#"{"signatures":{"Ig":{"linuxsteamrt64":{"module":"m.so","pattern":"55 48","resolve":"direct"}}},
                "calls":{"ignite":{"receiver":{"kind":"entity"},
                                   "target":{"kind":"signature","name":"Ig"},
                                   "args":[],"returns":"void"}}}"#,
        )
        .unwrap();
        let sigs = gd.get("signatures").unwrap().as_object();
        let decl = gd.get("calls").unwrap().get("ignite").unwrap();
        let flat: serde_json::Value = serde_json::from_str(&flatten_decl(decl, sigs).unwrap()).unwrap();
        assert_eq!(flat["target"]["pattern"], "55 48");
        assert_eq!(flat["target"]["module"], "m.so");
        assert_eq!(flat["target"]["resolve"], "direct");
    }

    #[test]
    fn flatten_can_select_the_windows_signature_entry() {
        let gd: serde_json::Value = serde_json::from_str(
            r#"{"signatures":{"Ig":{
                    "linuxsteamrt64":{"module":"libserver.so","pattern":"55 48","resolve":"direct"},
                    "windows64":{"module":"server.dll","pattern":"48 89","resolve":"lea-disp"}}},
                "calls":{"ignite":{"receiver":{"kind":"entity"},
                                   "target":{"kind":"signature","name":"Ig"},
                                   "args":[],"returns":"void"}}}"#,
        )
        .unwrap();
        let sigs = gd.get("signatures").unwrap().as_object();
        let decl = gd.get("calls").unwrap().get("ignite").unwrap();
        let flat: serde_json::Value = serde_json::from_str(
            &flatten_decl_for_platform(decl, sigs, "windows64").unwrap(),
        )
        .unwrap();
        assert_eq!(flat["target"]["module"], "server.dll");
        assert_eq!(flat["target"]["pattern"], "48 89");
        assert_eq!(flat["target"]["resolve"], "lea-disp");
    }

    /// A signature entry's `validate` is lifted WITH its pattern: the two are co-derived from the
    /// same disassembly pass and an operator's `custom/` override replaces the signature entry
    /// whole, so a validator parked in another section would be checked against a pattern it no
    /// longer describes.
    #[test]
    fn flatten_lifts_the_signature_entrys_validate_with_its_pattern() {
        let gd: serde_json::Value = serde_json::from_str(
            r#"{"signatures":{"Ig":{"linuxsteamrt64":{"module":"m.so","pattern":"55 48","resolve":"direct",
                                    "validate":{"string-xref":{"at":11,"dispOff":3,"instrLen":7,"expect":"Scope"}}}}},
                "calls":{"ignite":{"receiver":{"kind":"entity"},
                                   "target":{"kind":"signature","name":"Ig"},
                                   "args":[],"returns":"void"}}}"#,
        )
        .unwrap();
        let sigs = gd.get("signatures").unwrap().as_object();
        let decl = gd.get("calls").unwrap().get("ignite").unwrap();
        let flat: serde_json::Value = serde_json::from_str(&flatten_decl(decl, sigs).unwrap()).unwrap();
        assert_eq!(flat["target"]["validate"]["string-xref"]["at"], 11);
        assert_eq!(flat["target"]["validate"]["string-xref"]["expect"], "Scope");
    }

    /// An inline `target.validate` wins over the signature entry's, exactly as an inline `pattern`
    /// does — a hand-written flattened descriptor stays legal, and flattening stays idempotent.
    #[test]
    fn an_inline_target_validate_wins_over_the_signature_entrys() {
        let gd: serde_json::Value = serde_json::from_str(
            r#"{"signatures":{"Ig":{"linuxsteamrt64":{"module":"m.so","pattern":"55 48","resolve":"direct",
                                    "validate":{"vtable-member":"CFromSignature"}}}},
                "calls":{"ignite":{"receiver":{"kind":"entity"},
                                   "target":{"kind":"signature","name":"Ig",
                                             "validate":{"vtable-member":"CFromTarget"}},
                                   "args":[],"returns":"void"}}}"#,
        )
        .unwrap();
        let sigs = gd.get("signatures").unwrap().as_object();
        let decl = gd.get("calls").unwrap().get("ignite").unwrap();
        let flat: serde_json::Value = serde_json::from_str(&flatten_decl(decl, sigs).unwrap()).unwrap();
        assert_eq!(flat["target"]["validate"]["vtable-member"], "CFromTarget");
    }

    /// A signature with no entry for THIS platform is a named load-time degrade (spec §12), not a
    /// silent zero.
    #[test]
    fn flatten_names_a_missing_platform_entry() {
        let decl: serde_json::Value =
            serde_json::from_str(r#"{"target":{"kind":"signature","name":"Ig"}}"#).unwrap();
        let err = flatten_decl(&decl, None).unwrap_err();
        assert!(err.contains("linuxsteamrt64"), "{}", err);
    }

    /// A vtable target's `index`/`validate` live under the platform key (a prologue is as
    /// platform-specific as the slot it guards).
    #[test]
    fn flatten_lifts_the_platform_vtable_entry_into_the_target() {
        let decl: serde_json::Value = serde_json::from_str(
            r#"{"target":{"kind":"vtable","class":"CFoo",
                 "linuxsteamrt64":{"index":264,"validate":{"prologue":"55 48"}}}}"#,
        )
        .unwrap();
        let flat: serde_json::Value = serde_json::from_str(&flatten_decl(&decl, None).unwrap()).unwrap();
        assert_eq!(flat["target"]["index"], 264);
        assert_eq!(flat["target"]["validate"]["prologue"], "55 48");
    }

    #[test]
    fn flatten_can_select_the_windows_vtable_entry() {
        let decl: serde_json::Value = serde_json::from_str(
            r#"{"target":{"kind":"vtable","class":"CFoo",
                 "linuxsteamrt64":{"index":264,"validate":{"prologue":"55 48"}},
                 "windows64":{"index":271,"validate":{"prologue":"48 89"}}}}"#,
        )
        .unwrap();
        let flat: serde_json::Value = serde_json::from_str(
            &flatten_decl_for_platform(&decl, None, "windows64").unwrap(),
        )
        .unwrap();
        assert_eq!(flat["target"]["index"], 271);
        assert_eq!(flat["target"]["validate"]["prologue"], "48 89");
    }

    /// The arg/return vocabulary and the SysV budget are enforced with named reasons even though the
    /// SDK already fails the build on them (an older or hand-rolled `.s2sp` must not slip through).
    #[test]
    fn out_of_vocabulary_and_over_budget_descriptors_are_named() {
        crate::loader::load_permissions_from_str(r#"{"engine:calls":["@demo/gd"]}"#).unwrap();
        let mut reg = CallRegistry::new();

        let bad_arg = r#"{"receiver":{"kind":"entity"},"target":{"kind":"signature","pattern":"55"},
                          "args":["double"],"returns":"void"}"#;
        reg.register("@demo/gd", "badArg", bad_arg, true, false);
        assert!(reg.status("@demo/gd", "badArg").contains("unknown arg kind"));

        let bad_ret = r#"{"receiver":{"kind":"entity"},"target":{"kind":"signature","pattern":"55"},
                          "args":[],"returns":"string"}"#;
        reg.register("@demo/gd", "badRet", bad_ret, true, false);
        assert!(reg.status("@demo/gd", "badRet").contains("unknown return kind"));

        // Ten integer-class args: one past the 9-arg budget. Six used to be over-budget, back when
        // the limit was the SysV register count; args beyond the sixth now spill to the stack.
        let too_many = r#"{"receiver":{"kind":"entity"},"target":{"kind":"signature","pattern":"55"},
                           "args":["int","int","int","int","int","int","int","int","int","int"],
                           "returns":"void"}"#;
        reg.register("@demo/gd", "tooMany", too_many, true, false);
        assert!(reg.status("@demo/gd", "tooMany").contains("too many integer-class args"));

        // `none` is a SUPPORTED kind now — a static/free function with no receiver.
        let static_via = r#"{"receiver":{"kind":"none","via":{"class":"C","field":"m_f"}},
                             "target":{"kind":"signature","pattern":"55"},"args":[],"returns":"void"}"#;
        reg.register("@demo/gd", "staticVia", static_via, true, false);
        assert!(reg.status("@demo/gd", "staticVia").contains("cannot carry a 'via'"));

        let bad_receiver = r#"{"receiver":{"kind":"interface"},"target":{"kind":"signature","pattern":"55"},
                               "args":[],"returns":"void"}"#;
        reg.register("@demo/gd", "badRecv", bad_receiver, true, false);
        assert!(reg.status("@demo/gd", "badRecv").contains("unsupported receiver kind"));

        reg.register("@demo/gd", "malformed", "{not json", true, false);
        assert!(reg.status("@demo/gd", "malformed").contains("malformed descriptor"));
    }

    /// `drop_plugin` drops ONLY the named plugin's descriptors.
    #[test]
    fn drop_plugin_leaves_other_plugins_alone() {
        crate::loader::load_permissions_from_str(r#"{"engine:calls":[]}"#).unwrap();
        let mut reg = CallRegistry::new();
        reg.register("@demo/a", "x", &decl_json(), true, false);
        reg.register("@demo/b", "x", &decl_json(), true, false);
        reg.drop_plugin("@demo/a");
        assert!(reg.status("@demo/a", "x").contains("not declared"));
        assert!(reg.status("@demo/b", "x").contains("not permitted"));
    }

    fn decl_json() -> String {
        r#"{"receiver":{"kind":"entity"},"target":{"kind":"signature","name":"Ig",
            "module":"libserver.so","pattern":"55 48","resolve":"direct"},
            "args":["float"],"returns":"void"}"#
            .to_string()
    }

    // --- A5b: the game package's descriptor owner (spec §9.1b) --------------------------------

    /// The exact JSON text the shim's `GameConfig::mergedJson` hands core: `signatures` nested
    /// under the platform key, `calls` verbatim. Written out in full here so this test fails if
    /// either side's idea of the wire shape moves.
    fn merged_owner_gamedata(call: &str) -> String {
        format!(
            r#"{{"signatures":{{"DoThing":{{"linuxsteamrt64":{{"module":"libserver.so",
                 "pattern":"55 48","resolve":"direct"}}}}}},
                "calls":{{"{call}":{{"receiver":{{"kind":"entity"}},
                 "target":{{"kind":"signature","name":"DoThing"}},
                 "args":["int"],"returns":"void"}}}}}}"#
        )
    }

    /// THE point of the reserved owner: the game package's descriptors register without any
    /// `engine:calls` allow-list entry. Asserted through the GATE ORDER rather than a fake engine
    /// op — permission is check (1) and the op is check (2), so "engine op unavailable" can only
    /// be reached by a descriptor that already passed the allow-list.
    #[test]
    fn game_package_descriptors_bypass_the_engine_calls_allow_list() {
        // An allow-list that names nobody: the same DENY a never-loaded (default-deny) one gives.
        crate::loader::load_permissions_from_str(r#"{"engine:calls":[]}"#).unwrap();
        register_game_package("@demo/bypass", &merged_owner_gamedata("doThing"));
        let owner = reserved_owner_id("@demo/bypass");
        let st = status(&owner, "doThing");
        assert!(!st.contains("not permitted"), "the allow-list must not gate the runtime: {st}");
        assert!(!st.contains("not declared"), "the descriptor must be registered: {st}");
        assert_eq!(game_package_owner().as_deref(), Some(owner.as_str()));
        drop_plugin(&owner);
    }

    /// The same descriptor set arriving as a PLUGIN's is still gated — the exemption belongs to the
    /// registration entry point, not to the descriptor shape.
    #[test]
    fn the_same_descriptors_from_a_plugin_are_still_allow_listed() {
        crate::loader::load_permissions_from_str(r#"{"engine:calls":[]}"#).unwrap();
        register_plugin("@demo/plain", &merged_owner_gamedata("doThing"));
        assert!(
            status("@demo/plain", "doThing").contains("not permitted"),
            "a plugin's declared call must still need the operator allow-list"
        );
        drop_plugin("@demo/plain");
    }

    /// A `.s2sp` must never hold the permission-exempt identity. `loader::read_s2sp` refuses such a
    /// manifest at the door; this is the second, independent latch on the registration itself.
    #[test]
    fn a_plugin_cannot_register_under_the_reserved_owner_id() {
        crate::loader::load_permissions_from_str(r#"{"engine:calls":[]}"#).unwrap();
        let spoofed = reserved_owner_id("@demo/spoof");
        register_plugin(&spoofed, &merged_owner_gamedata("doThing"));
        assert_eq!(
            status(&spoofed, "doThing"),
            "not declared in this plugin's gamedata",
            "a plugin registration under a reserved owner id must register NOTHING"
        );
    }

    /// The reserved namespace is disjoint from every npm-style package name — including the game
    /// package's own name, which is a real npm package and therefore spellable by a manifest.
    #[test]
    fn the_reserved_namespace_is_not_an_npm_name() {
        assert!(RESERVED_OWNER_PREFIX.contains(':'), "an npm package name may not contain ':'");
        assert!(!is_reserved_owner("@s2script/cs2"));
        assert!(!is_reserved_owner("@demo/anything"));
        assert!(!is_reserved_owner("plain-plugin"));
        assert!(is_reserved_owner(&reserved_owner_id("@s2script/cs2")));
    }

    /// Re-registering an owner REPLACES its view: a descriptor that left the gamedata tree must not
    /// survive as a live callable from the previous load.
    #[test]
    fn re_registering_the_game_package_replaces_the_previous_view() {
        crate::loader::load_permissions_from_str(r#"{"engine:calls":[]}"#).unwrap();
        register_game_package("@demo/replace", &merged_owner_gamedata("first"));
        register_game_package("@demo/replace", &merged_owner_gamedata("second"));
        let owner = reserved_owner_id("@demo/replace");
        assert!(status(&owner, "first").contains("not declared"), "the retired descriptor is gone");
        assert!(!status(&owner, "second").contains("not declared"), "the new one registered");
        drop_plugin(&owner);
    }

    /// `argNames` is an SDK-only documentation field (it names the generated `.d.ts` parameters). The
    /// packed `gamedata.json` carries it, so core MUST ignore it rather than reject the descriptor.
    /// This holds today because the decl is read as a raw `serde_json::Value`; the test exists so that
    /// swapping in a typed struct with `deny_unknown_fields` fails here instead of at load on a live
    /// server.
    #[test]
    fn decl_with_arg_names_still_flattens() {
        let decl: serde_json::Value = serde_json::from_str(
            r#"{"receiver":{"kind":"entity"},"target":{"kind":"signature","name":"Ig",
                "module":"libserver.so","pattern":"55 48","resolve":"direct"},
                "args":["float","int"],"argNames":["flFlameLifetime","nFlags"],"returns":"void"}"#,
        )
        .expect("parses");
        let flat = flatten_decl(&decl, None).expect("argNames must not break flattening");
        assert!(flat.contains("\"args\""), "kinds survive: {flat}");
        assert!(flat.contains("float"), "kinds survive: {flat}");
    }
}
