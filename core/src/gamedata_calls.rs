//! Plugin-declared engine calls — the core's half of the plugin-gamedata slice (spec §10, the
//! "Core" row): the per-plugin descriptor registry, the two-part authorization gate, and the SysV
//! argument classification the invoke native marshals against.
//!
//! BOUNDARY (spec §10). Every name in a descriptor — target kind, module, byte pattern, resolver
//! strategy, class, field, prologue — is an OPAQUE plugin-supplied string that crosses core
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

/// The platform id whose nested details the runtime consumes (spec §5, Global Constraints). Exactly
/// one platform ships today; the key stays explicit so a second one is additive.
const PLATFORM: &str = "linuxsteamrt64";

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
const MAX_GP_ARGS: usize = 5;
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
    pub(crate) fn register(
        &mut self,
        plugin_id: &str,
        call_name: &str,
        decl_json: &str,
        ops_available: bool,
    ) {
        match prepare(plugin_id, decl_json, ops_available) {
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
fn prepare(plugin_id: &str, decl_json: &str, ops_available: bool) -> Result<InvokePlan, String> {
    // (1) Authorization (spec §6). Declaration in the manifest is necessary but NOT sufficient: the
    // operator's allow-list decides, and an unloaded/absent allow-list denies everything.
    if !crate::loader::permission_allowed(plugin_id, PERMISSION) {
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

    // Receiver: v1 is entity-only, and `kind` is a TAGGED field so a later slice adds a kind
    // additively rather than redesigning the format (spec §4).
    let receiver = decl.get("receiver");
    let rkind = receiver.and_then(|r| r.get("kind")).and_then(|v| v.as_str()).unwrap_or("entity");
    if rkind != "entity" {
        return Err(format!("unsupported receiver kind '{}'", rkind));
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
            "too many integer-class args ({} > {}) — stack-passed args are out of scope",
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

    Ok(InvokePlan { call_id, args, ret_code: ret, via })
}

/// Call the shim's resolve op with the descriptor's opaque strings. Returns the call id, or the
/// shim's own named reason (signature miss / ambiguous pattern / prologue mismatch / slot outside
/// `.text` / …) verbatim — core never re-words it, so the log and `Engine.status()` say exactly what
/// the resolver decided.
fn resolve_target(target: &serde_json::Value) -> Result<i32, String> {
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
    let prologue = cstr(
        target.get("validate").and_then(|v| v.get("prologue")).and_then(|v| v.as_str()).unwrap_or(""),
    )?;
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
        prologue.as_ptr(),
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
fn flatten_decl(
    decl: &serde_json::Value,
    signatures: Option<&serde_json::Map<String, serde_json::Value>>,
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
                    .and_then(|v| v.get(PLATFORM))
                    .ok_or_else(|| {
                        format!("no '{}' entry for signature '{}'", PLATFORM, name)
                    })?;
                for key in ["module", "pattern", "resolve"] {
                    if let Some(v) = spec.get(key) {
                        target.insert(key.to_string(), v.clone());
                    }
                }
            }
        }
        "vtable" => {
            let plat = target
                .get(PLATFORM)
                .cloned()
                .ok_or_else(|| format!("vtable target has no '{}' entry", PLATFORM))?;
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
}

/// Register every `calls` entry of a plugin's packed `gamedata.json` (`read_s2sp`'s third element).
/// Called from the loader at plugin load, BEFORE the plugin's factory runs — the factory's
/// `Engine.call(name)` must see a resolved (or named-degraded) descriptor.
///
/// Degrade-never-crash: a malformed member WARNs once and registers nothing (every call then reports
/// "not declared"); a gamedata with no `calls` section is the normal case and does nothing.
pub(crate) fn register_plugin(plugin_id: &str, gamedata_json: &str) {
    let gd: serde_json::Value = match serde_json::from_str(gamedata_json) {
        Ok(v) => v,
        Err(e) => {
            crate::v8host::log_warn(&format!(
                "WARN: [engine-calls] '{}': unreadable gamedata.json ({}) - no declared calls registered",
                plugin_id, e
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
            Ok(decl) => REGISTRY
                .with(|r| r.borrow_mut().register(plugin_id, name, &decl, ops_available)),
            Err(reason) => REGISTRY.with(|r| r.borrow_mut().degrade(plugin_id, name, &reason)),
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
        reg.register("@demo/gd", "ignite", &decl_json(), /*ops_available=*/ true);
        assert!(reg.call_id("@demo/gd", "ignite").is_none());
        assert!(reg.status("@demo/gd", "ignite").contains("not permitted"));
    }

    #[test]
    fn registry_degrades_when_ops_absent() {
        crate::loader::load_permissions_from_str(r#"{"engine:calls":["@demo/gd"]}"#).unwrap();
        let mut reg = CallRegistry::new();
        reg.register("@demo/gd", "ignite", &decl_json(), /*ops_available=*/ false);
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
        reg.register("@demo/gd", "ignite", &decl_json(), true);
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

    /// The arg/return vocabulary and the SysV budget are enforced with named reasons even though the
    /// SDK already fails the build on them (an older or hand-rolled `.s2sp` must not slip through).
    #[test]
    fn out_of_vocabulary_and_over_budget_descriptors_are_named() {
        crate::loader::load_permissions_from_str(r#"{"engine:calls":["@demo/gd"]}"#).unwrap();
        let mut reg = CallRegistry::new();

        let bad_arg = r#"{"receiver":{"kind":"entity"},"target":{"kind":"signature","pattern":"55"},
                          "args":["double"],"returns":"void"}"#;
        reg.register("@demo/gd", "badArg", bad_arg, true);
        assert!(reg.status("@demo/gd", "badArg").contains("unknown arg kind"));

        let bad_ret = r#"{"receiver":{"kind":"entity"},"target":{"kind":"signature","pattern":"55"},
                          "args":[],"returns":"string"}"#;
        reg.register("@demo/gd", "badRet", bad_ret, true);
        assert!(reg.status("@demo/gd", "badRet").contains("unknown return kind"));

        let too_many = r#"{"receiver":{"kind":"entity"},"target":{"kind":"signature","pattern":"55"},
                           "args":["int","int","int","int","int","int"],"returns":"void"}"#;
        reg.register("@demo/gd", "tooMany", too_many, true);
        assert!(reg.status("@demo/gd", "tooMany").contains("too many integer-class args"));

        let bad_receiver = r#"{"receiver":{"kind":"interface"},"target":{"kind":"signature","pattern":"55"},
                               "args":[],"returns":"void"}"#;
        reg.register("@demo/gd", "badRecv", bad_receiver, true);
        assert!(reg.status("@demo/gd", "badRecv").contains("unsupported receiver kind"));

        reg.register("@demo/gd", "malformed", "{not json", true);
        assert!(reg.status("@demo/gd", "malformed").contains("malformed descriptor"));
    }

    /// `drop_plugin` drops ONLY the named plugin's descriptors.
    #[test]
    fn drop_plugin_leaves_other_plugins_alone() {
        crate::loader::load_permissions_from_str(r#"{"engine:calls":[]}"#).unwrap();
        let mut reg = CallRegistry::new();
        reg.register("@demo/a", "x", &decl_json(), true);
        reg.register("@demo/b", "x", &decl_json(), true);
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
