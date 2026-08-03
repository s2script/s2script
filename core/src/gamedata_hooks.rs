//! Declaratively-hooked engine functions — the core's half of the declarative-inbound-hooks slice
//! (spec `docs/superpowers/specs/2026-08-02-declarative-inbound-hooks-design.md`).
//!
//! This is the INBOUND sibling of `gamedata_calls`, and is deliberately shaped like it: one
//! per-owner descriptor registry, one authorization gate, a NAMED degrade per descriptor, and a
//! `status()` an operator can read. Resolution goes through `gamedata_calls::resolve_target` — the
//! same entry, the same validators, the same `.text` range check (spec §6: "No second resolver").
//!
//! FOUR RULES AN OUTBOUND CALL DOES NOT HAVE, each of them because a hook patches BYTES:
//!
//! 1. **A validator is MANDATORY** (spec §5). A `calls` entry may rely on signature uniqueness
//!    alone; a `hooks` entry may not. A wrong call address misbehaves — a wrong DETOUR address
//!    overwrites the prologue of whatever is actually there. `s2detour::Install` refuses an
//!    unrelocatable prologue, but that is a decode check, not an identity check.
//! 2. **`engine:hooks` is a SEPARATE permission** from `engine:calls` (spec §7). An operator who
//!    granted a plugin the ability to call engine functions has not thereby granted it the ability
//!    to detour them, or to suppress engine behaviour.
//! 3. **Install is LAZY and idempotent** (spec §6). No subscribers means no patched bytes and no
//!    treadmill surface. Every subscribe calls `subscribe()`, and only the first one patches.
//! 4. **A hook slot is never re-pointed once installed.** There is no uninstall: removing a live
//!    detour races the engine calling through it, and SourceMod does not do it either. So the slot
//!    table below OUTLIVES `drop_owner` — a reload re-uses the same slot for the same address, and a
//!    descriptor that moved gets a NEW slot rather than silently re-pointing an installed one.
//!
//! BOUNDARY. Exactly as in `gamedata_calls`: every name in a descriptor — pattern, class, validator
//! keys, the shape NAME — is an opaque owner-supplied string. No game identifier is compiled in, and
//! `scripts/check-core-boundary.sh` stays green.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::raw::c_char;

/// The capability the DECLARING owner must have in its manifest AND be operator-allow-listed for
/// (spec §7). Separate from `gamedata_calls::PERMISSION` on purpose — see rule 2 above.
///
/// V1 SCOPE: the game package is the only declarer that can exist today — it is exempt as
/// first-party runtime, and `s2s build` refuses both a `hooks` gamedata section and this permission,
/// so no `.s2sp` can carry one (see the scope note in the spec). The non-exempt branch below is
/// therefore reachable only from `cargo test` for now, and is KEPT rather than narrowed to match:
/// the check is correct for the design, the plugin path is a planned follow-up slice, and a
/// default-deny gate relaxed to fit a temporary scope stops being a gate.
pub(crate) const PERMISSION: &str = "engine:hooks";

/// The shim's hook-slot budget (`S2_HOOK_MAX` in `shim/src/hook_dispatch.h`). MUST match it: the
/// shim's latch array and thunk tables are sized by it, and an id past the end is refused there by
/// name. Mirrored here so core can report "no free hook slot" as a load-time degrade instead of
/// discovering it at install.
const MAX_HOOKS: i32 = 64;

/// Buffer size for a degrade reason written back by the shim (same as `gamedata_calls`).
const REASON_CAP: usize = 256;

/// The shim's CLOSED shape vocabulary, mirrored name → id.
///
/// MUST match `kShapes` in `shim/src/hook_dispatch.cpp` — this is an ABI, not a convention, exactly
/// like `gamedata_calls`' `GP_*`/`RET_*` mirrors of `engine_calls.cpp`'s arg/return enums.
/// `scripts/check-hook-shapes.sh` diffs the two tables so a rename or a re-numbering fails the
/// build: a shape id that means one signature in core and another in the shim would install a thunk
/// with the WRONG ABI, which corrupts the stack on the first engine call.
///
/// An unknown NAME is rejected here, by name; an unknown ID (i.e. this table drifting ahead of the
/// shim's) is rejected at install by the shim's own "unknown hook shape id" reason. Both are named
/// degrades, never a silent default to shape 0.
pub(crate) const SHAPES: &[(&str, i32)] =
    &[("this_void", 0), ("this_f32_i32_i32_i32", 1)];

/// The shape id for a vocabulary name, or `None` for anything outside it.
fn shape_id(name: &str) -> Option<i32> {
    SHAPES.iter().find(|(n, _)| *n == name).map(|(_, id)| *id)
}

/// Everything a dispatch needs, cloned out of the registry so the registry borrow is released BEFORE
/// any JS runs — the same discipline (and the same reason) as `gamedata_calls::InvokePlan`: a
/// handler can call back into core.
#[derive(Clone)]
pub(crate) struct HookPlan {
    /// The shim's hook slot. Baked into the thunk at compile time, so it is also what identifies
    /// this hook on the way back in (`s2script_core_dispatch_hook`).
    pub hook_id: i32,
    /// The shape id (`SHAPES`). Decides the thunk's signature, hence the arg view's contents.
    pub shape: i32,
    /// The descriptor's positional param names, in author order. Index i is the arg view's index i.
    pub params: Vec<String>,
    /// Parallel to `params`: may a handler write this one back to the engine?
    pub writable: Vec<bool>,
    /// `receiver.as` — the name to surface the detour's `this` under, as a books-gated `EntityRef`.
    /// `None` for `receiver.kind: "none"` (the default): most detour receivers are not entities at
    /// all (a rules/services singleton), and that is a normal "nothing to surface".
    pub receiver: Option<String>,
    /// `expose.ctx` — the `ctx` namespace the generated binding hangs this hook off. Core validates
    /// it and carries it for the boot log; the codegen (Task 6) reads it from the gamedata itself.
    pub ctx_ns: String,
}

/// One registered descriptor. `Ready` carries the plan; `Degraded` carries the named reason
/// `status()` reports.
enum Descriptor {
    Ready {
        plan: HookPlan,
        /// The resolved absolute address, held for the LAZY install (spec §6). Opaque to core: it is
        /// only ever handed back to `hook_install`, which re-proves the whole patch window is inside
        /// a loaded module's executable range before `s2detour` reads a byte.
        addr: i64,
        /// `bypassWith` — the `calls` descriptor in this SAME owner whose invocation arms the latch.
        bypass_with: Option<String>,
        /// A named reason recorded for a LATE failure, kept separate from `Degraded` because the
        /// descriptor itself resolved (mirrors `gamedata_calls`' `via_miss`). Two things set it: an
        /// install that the shim refused at first subscribe, and an arg-view accessor that returned
        /// -1 during a dispatch. The second is why it exists at all — a handler reading a param must
        /// never receive a plausible-looking `0` when the read actually FAILED.
        miss: Option<String>,
    },
    Degraded(String),
}

/// `(owner id, hook name)` → descriptor. Per-owner by key, so `drop_owner` is the whole teardown.
struct HookRegistry {
    hooks: HashMap<(String, String), Descriptor>,
}

impl HookRegistry {
    fn new() -> Self {
        Self { hooks: HashMap::new() }
    }

    fn degrade(&mut self, owner: &str, name: &str, reason: &str) {
        crate::v8host::log_warn(&format!(
            "WARN: [engine-hooks] '{}' hook '{}' unavailable: {}",
            owner, name, reason
        ));
        self.hooks.insert(
            (owner.to_string(), name.to_string()),
            Descriptor::Degraded(reason.to_string()),
        );
    }

    fn status(&self, owner: &str, name: &str) -> String {
        match self.hooks.get(&(owner.to_string(), name.to_string())) {
            None => "not declared in this owner's gamedata".to_string(),
            Some(Descriptor::Degraded(reason)) => reason.clone(),
            Some(Descriptor::Ready { miss: Some(reason), .. }) => reason.clone(),
            Some(Descriptor::Ready { .. }) => "available".to_string(),
        }
    }

    fn plan(&self, owner: &str, name: &str) -> Option<HookPlan> {
        match self.hooks.get(&(owner.to_string(), name.to_string())) {
            Some(Descriptor::Ready { plan, .. }) => Some(plan.clone()),
            _ => None,
        }
    }

    fn drop_owner(&mut self, owner: &str) {
        self.hooks.retain(|(o, _), _| o != owner);
    }
}

/// A hook SLOT: the shim-side identity of one installed (or installable) detour.
///
/// Deliberately NOT part of `HookRegistry`, because it has a different lifetime. There is no
/// uninstall until Unload (spec §6), so once `installed` is true the pair (slot, address) is fixed
/// for the process: re-pointing that slot at another address would orphan the first detour's
/// trampoline, and the shim refuses it by name anyway. Surviving `drop_owner` is what makes a plugin
/// RELOAD re-use its own slot — the re-resolved address matches, `hook_install` takes its idempotent
/// path, and nothing is patched twice.
#[derive(Clone, Copy)]
struct Slot {
    id: i32,
    shape: i32,
    addr: i64,
    installed: bool,
}

// ---------------------------------------------------------------------------
// The two ENGINE hops, behind function pointers.
//
// Injected rather than called directly for the reason `shim/src/hook_dispatch.cpp` gives for its own
// one outside contact: a rule that cannot fail in a test is decoration. The lazy-install rule ("the
// second subscribe must NOT re-install") and the no-uninstall rule are the two things this module
// exists to get right, and neither is observable without an engine unless the hop is replaceable.
// Production is the default; only this file's tests ever swap them.
// ---------------------------------------------------------------------------

/// Resolve a flattened `target` to an absolute address (spec §6: through `S2_EngineCallResolve`).
type ResolveFn = fn(&serde_json::Value) -> Result<i64, String>;
/// Install `(hook_id, shape, addr)`. `Err` carries the shim's own named reason, verbatim.
type InstallFn = fn(i32, i32, i64) -> Result<(), String>;

thread_local! {
    static REGISTRY: RefCell<HookRegistry> = RefCell::new(HookRegistry::new());
    /// `(owner, name)` → slot. Outlives `drop_owner` — see `Slot`.
    static SLOTS: RefCell<HashMap<(String, String), Slot>> = RefCell::new(HashMap::new());
    static RESOLVE: Cell<ResolveFn> = Cell::new(resolve_via_engine);
    static INSTALL: Cell<InstallFn> = Cell::new(install_via_engine);
}

/// Resolve through the SAME path an outbound call uses, then ask the shim for the address behind the
/// returned call id. Two ops, one resolution: `S2_EngineCallResolve` does every check (uniqueness,
/// both validators, the `.text` range), and `engine_call_address` is only the id → address hop that
/// an install needs and an invoke does not.
fn resolve_via_engine(target: &serde_json::Value) -> Result<i64, String> {
    let call_id = crate::gamedata_calls::resolve_target(target)?;
    let Some(addr_of) = crate::v8host::engine_ops().and_then(|o| o.engine_call_address) else {
        return Err("engine op unavailable".to_string());
    };
    let addr = addr_of(call_id);
    if addr == 0 {
        return Err("resolved descriptor has no address".to_string());
    }
    Ok(addr)
}

/// Ask the shim to patch. Every failure here is ONE hook disabled with the shim's own named reason
/// (out-of-range id, unknown shape, an address outside any module's executable range, an address
/// another hook already owns, a prologue `s2detour` refuses) — never a load failure, never a crash.
fn install_via_engine(hook_id: i32, shape: i32, addr: i64) -> Result<(), String> {
    let Some(func) = crate::v8host::engine_ops().and_then(|o| o.hook_install) else {
        return Err("engine op unavailable".to_string());
    };
    let mut reason = vec![0u8; REASON_CAP];
    let rc = func(hook_id, shape, addr, reason.as_mut_ptr() as *mut c_char, REASON_CAP as i32);
    if rc == 0 {
        return Ok(());
    }
    let text = reason_string(&reason);
    Err(if text.is_empty() { "detour install refused".to_string() } else { text })
}

/// Read the shim's NUL-terminated reason out of the buffer (lossy; never panics on non-UTF-8).
fn reason_string(buf: &[u8]) -> String {
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).trim().to_string()
}

// ---------------------------------------------------------------------------
// Registration.
// ---------------------------------------------------------------------------

/// Register every `hooks` entry a PLUGIN declared in its packed `gamedata.json`. Called by the
/// loader at plugin load, beside `gamedata_calls::register_plugin` and AFTER it — `bypassWith` is
/// validated against the same tree's `calls` section, so the two never have to agree about order.
///
/// Refuses a reserved owner id for the same reason `gamedata_calls::register_plugin` does: that
/// identity is permission-EXEMPT and belongs to the runtime, and a hook is the more dangerous of the
/// two capabilities it would grant.
pub(crate) fn register_plugin(plugin_id: &str, gamedata_json: &str) {
    if crate::gamedata_calls::is_reserved_owner(plugin_id) {
        crate::v8host::log_warn(&format!(
            "WARN: [engine-hooks] REFUSED a plugin registration under the reserved owner id '{}' — \
             that identity is the runtime's, not a plugin's; no hooks registered",
            plugin_id
        ));
        return;
    }
    register_owner(plugin_id, gamedata_json);
}

/// Register the GAME PACKAGE's declared hooks from the merged gamedata the shim produced for that
/// owner. Lands under `gamedata_calls::reserved_owner_id(package)`, which no `.s2sp` can hold, and
/// is permission-exempt: first-party runtime shipped in the same zip as core and the shim.
pub(crate) fn register_game_package(package: &str, gamedata_json: &str) {
    let owner = crate::gamedata_calls::reserved_owner_id(package);
    // Idempotent: a re-register replaces the previous view whole. The SLOT table is deliberately
    // untouched — a re-registered descriptor that resolves to the same address re-uses its slot, so
    // the detour installed by the previous registration is exactly the one the new one wants.
    REGISTRY.with(|r| r.borrow_mut().drop_owner(&owner));
    if gamedata_json.trim().is_empty() {
        return;
    }
    register_owner_inner(&owner, gamedata_json, /*permission_exempt=*/ true);
}

/// The registration entry the tests drive, and the plugin path's body. Non-exempt: the operator
/// allow-list decides, exactly as it does for a plugin's `calls`.
pub(crate) fn register_owner(owner_id: &str, gamedata_json: &str) {
    register_owner_inner(owner_id, gamedata_json, /*permission_exempt=*/ false);
}

/// Parse, flatten and register each named `hooks` entry.
///
/// Degrade-never-crash: unreadable gamedata WARNs once and registers nothing (every hook then
/// reports "not declared"); a gamedata with no `hooks` section is the normal case and does nothing.
fn register_owner_inner(owner_id: &str, gamedata_json: &str, permission_exempt: bool) {
    let gd: serde_json::Value = match serde_json::from_str(gamedata_json) {
        Ok(v) => v,
        Err(e) => {
            crate::v8host::log_warn(&format!(
                "WARN: [engine-hooks] '{}': unreadable gamedata.json ({}) - no declared hooks registered",
                owner_id, e
            ));
            return;
        }
    };
    let Some(hooks) = gd.get("hooks").and_then(|v| v.as_object()) else { return };
    let signatures = gd.get("signatures").and_then(|v| v.as_object());
    // The owner's own `calls` keys — what `bypassWith` must name. Read from the SAME tree rather
    // than from the call registry, so the check does not depend on registration order and reports
    // "unknown bypassWith" for a typo even when that call itself failed to resolve.
    let call_names: Vec<String> = gd
        .get("calls")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    // Deterministic order so the boot log reads the same on every start.
    let mut names: Vec<&String> = hooks.keys().collect();
    names.sort();
    for name in names {
        let decl = match crate::gamedata_calls::flatten_decl(&hooks[name], signatures) {
            Ok(flat) => flat,
            Err(reason) => {
                REGISTRY.with(|r| r.borrow_mut().degrade(owner_id, name, &reason));
                continue;
            }
        };
        match prepare(owner_id, name, &decl, &call_names, permission_exempt) {
            Ok((plan, addr, bypass_with)) => {
                // A descriptor that is now one subscribe away from patching engine bytes is worth
                // one audit line, exactly as an armed `calls` descriptor is.
                crate::v8host::log_warn(&format!(
                    "[engine-hooks] '{}' armed '{}' (slot {}, shape '{}', ctx.{}) — not yet installed \
                     (install is lazy, on first subscribe)",
                    owner_id,
                    name,
                    plan.hook_id,
                    shape_name(plan.shape),
                    plan.ctx_ns
                ));
                REGISTRY.with(|r| {
                    r.borrow_mut().hooks.insert(
                        (owner_id.to_string(), name.to_string()),
                        Descriptor::Ready { plan, addr, bypass_with, miss: None },
                    )
                });
            }
            Err(reason) => REGISTRY.with(|r| r.borrow_mut().degrade(owner_id, name, &reason)),
        }
    }
}

/// The canonical name of a shape id (for logs). Falls back to the id — an id with no name cannot
/// reach here, but a log line must never be the thing that panics.
fn shape_name(shape: i32) -> String {
    SHAPES
        .iter()
        .find(|(_, id)| *id == shape)
        .map(|(n, _)| (*n).to_string())
        .unwrap_or_else(|| format!("#{}", shape))
}

/// A plain identifier: what may become a `ctx` namespace, a param name or a receiver name in
/// generated TypeScript. Deliberately strict — these strings are EMITTED into a `.d.ts` and read
/// back as property names, so anything that would need quoting or escaping is refused at the door.
fn is_plain_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Validate + resolve one flattened hook descriptor. Returns `(plan, address, bypassWith)`, or the
/// named degrade reason.
///
/// Check order matches `gamedata_calls::prepare`: authorization first (an unauthorized owner learns
/// nothing about whether its descriptor would have resolved), then the descriptor's own shape, then
/// the live resolve — which is the only expensive step and the only one that touches the binary.
fn prepare(
    owner_id: &str,
    hook_name: &str,
    decl_json: &str,
    call_names: &[String],
    permission_exempt: bool,
) -> Result<(HookPlan, i64, Option<String>), String> {
    // (1) Authorization (spec §7). Declaring `engine:hooks` in the manifest is necessary but NOT
    // sufficient: the operator's allow-list decides, and an absent allow-list denies everything.
    if !permission_exempt && !crate::loader::permission_allowed(owner_id, PERMISSION) {
        return Err(format!(
            "not permitted by the operator allow-list (add \"{}\" under \"{}\" in configs/permissions.json)",
            owner_id, PERMISSION
        ));
    }

    let decl: serde_json::Value =
        serde_json::from_str(decl_json).map_err(|e| format!("malformed descriptor: {}", e))?;

    // (2) `expose.ctx` — the namespace the generated binding hangs off. Mandatory: a hook nothing
    // can subscribe to is a patched prologue with no purpose.
    let ctx_ns = decl
        .get("expose")
        .and_then(|e| e.get("ctx"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "descriptor has no 'expose.ctx' (nothing could subscribe to it)".to_string())?;
    if !is_plain_ident(ctx_ns) {
        return Err(format!("'expose.ctx' must be a plain identifier, got '{}'", ctx_ns));
    }

    // (3) The shape. Rejected BY NAME here: a typo must never default to shape 0, whose wrong ABI
    // would corrupt the stack on the first engine call.
    let shape_str = decl
        .get("shape")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "descriptor has no 'shape'".to_string())?;
    let shape = shape_id(shape_str).ok_or_else(|| {
        format!(
            "unknown hook shape '{}' (this build compiles thunks for: {})",
            shape_str,
            SHAPES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
        )
    })?;

    // (4) THE MANDATORY VALIDATOR (spec §5). Unlike `calls`, uniqueness alone is not enough: a
    // wrong call address misbehaves, a wrong DETOUR address overwrites the prologue of whatever is
    // actually there. `{}` and `null` count as absent — they would resolve exactly as no validator.
    let has_validate = decl
        .get("target")
        .and_then(|t| t.get("validate"))
        .is_some_and(|v| v.as_object().is_some_and(|o| !o.is_empty()));
    if !has_validate {
        return Err(
            "a hook target MUST carry a non-empty 'validate' (a wrong detour address overwrites \
             the prologue of whatever is actually there)"
                .to_string(),
        );
    }

    // (5) `bypassWith`, when present, names a `calls` descriptor in the SAME owner — the declarative
    // form of SourceMod's g_pIgnoreTerminateDetour. A typo would silently mean "no bypass", i.e. a
    // plugin-issued call firing our own hook, which is the semantic this whole latch exists to
    // prevent, so it is refused by name.
    let bypass_with = match decl.get("bypassWith") {
        None => None,
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| "'bypassWith' must be a call name".to_string())?;
            if !call_names.iter().any(|c| c == s) {
                return Err(format!(
                    "'bypassWith' names '{}', which is not a 'calls' descriptor in this owner's gamedata",
                    s
                ));
            }
            Some(s.to_string())
        }
    };

    // (6) `params` / `mutable`. Positional: index i in the descriptor is index i in the arg view, so
    // a duplicate name would make one of the two unreachable from the generated binding.
    let params: Vec<String> = decl
        .get("params")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect())
        .unwrap_or_default();
    for p in &params {
        if !is_plain_ident(p) {
            return Err(format!("param name '{}' is not a plain identifier", p));
        }
        if params.iter().filter(|q| *q == p).count() > 1 {
            return Err(format!("param name '{}' is declared more than once", p));
        }
    }
    let mutables: Vec<String> = decl
        .get("mutable")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect())
        .unwrap_or_default();
    for m in &mutables {
        if !params.iter().any(|p| p == m) {
            return Err(format!("'mutable' names '{}', which is not one of this hook's params", m));
        }
    }
    let writable: Vec<bool> = params.iter().map(|p| mutables.iter().any(|m| m == p)).collect();

    // (7) The receiver. Tagged `kind`, like a call's, so a new kind is additive rather than a
    // redesign. `none` (the default) surfaces nothing: a detour `this` is frequently not an entity
    // at all, and pretending otherwise is what a books-first walk exists to refuse.
    let receiver = decl.get("receiver");
    let rkind = receiver.and_then(|r| r.get("kind")).and_then(|v| v.as_str()).unwrap_or("none");
    let receiver = match rkind {
        "none" => None,
        "entity" => {
            let as_name = receiver
                .and_then(|r| r.get("as"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| "receiver.kind 'entity' needs an 'as' name to surface it under".to_string())?;
            if !is_plain_ident(as_name) {
                return Err(format!("receiver 'as' must be a plain identifier, got '{}'", as_name));
            }
            if params.iter().any(|p| p == as_name) {
                return Err(format!("receiver 'as' name '{}' collides with a param name", as_name));
            }
            Some(as_name.to_string())
        }
        other => return Err(format!("unsupported receiver kind '{}'", other)),
    };

    // (8) The live resolve — the same entry, the same validators, the same range check an outbound
    // call gets. The shim's reason crosses back verbatim; core never re-words it.
    let target = decl.get("target").ok_or_else(|| "descriptor has no target".to_string())?;
    let addr = RESOLVE.with(|r| r.get())(target)?;

    // (9) The slot. Allocated only now, so a descriptor that failed any check above never consumes
    // one of the 64.
    let hook_id = allocate_slot(owner_id, hook_name, shape, addr)?;

    Ok((HookPlan { hook_id, shape, params, writable, receiver, ctx_ns: ctx_ns.to_string() }, addr, bypass_with))
}

/// Give `(owner, name)` a shim hook slot.
///
/// Re-uses the existing slot when this owner+name already has one for the SAME (shape, address) —
/// which is what a plugin reload is, and what makes `hook_install`'s idempotent path the one taken.
/// A slot whose target MOVED is only re-pointed while it is still uninstalled; once installed it is
/// frozen (there is no uninstall, and re-pointing would orphan the first detour's trampoline), so a
/// moved target gets a fresh slot and the stale detour stays until Unload.
fn allocate_slot(owner: &str, name: &str, shape: i32, addr: i64) -> Result<i32, String> {
    SLOTS.with(|s| {
        let mut slots = s.borrow_mut();
        let key = (owner.to_string(), name.to_string());
        if let Some(existing) = slots.get(&key).copied() {
            if existing.shape == shape && existing.addr == addr {
                return Ok(existing.id);
            }
            if !existing.installed {
                slots.insert(key, Slot { id: existing.id, shape, addr, installed: false });
                return Ok(existing.id);
            }
            crate::v8host::log_warn(&format!(
                "WARN: [engine-hooks] '{}' hook '{}' resolved to a DIFFERENT target than the detour \
                 already installed in slot {} — taking a fresh slot; the old detour stays until Unload",
                owner, name, existing.id
            ));
        }
        // Another owner may already hold a slot for this exact address (two owners declaring a hook
        // on one function). The shim refuses the second install by name ("another hook id is already
        // installed at this address"), which is the correct outcome — but it must be a NAMED degrade
        // of the second hook, not a silent one, so nothing special is done here.
        let used: Vec<i32> = slots.values().map(|sl| sl.id).collect();
        let id = (0..MAX_HOOKS).find(|c| !used.contains(c)).ok_or_else(|| {
            format!("no free hook slot (this build installs at most {})", MAX_HOOKS)
        })?;
        slots.insert((owner.to_string(), name.to_string()), Slot { id, shape, addr, installed: false });
        Ok(id)
    })
}

// ---------------------------------------------------------------------------
// The accessors core's natives / loader / dispatch use.
// ---------------------------------------------------------------------------

/// Ledger teardown: drop every descriptor this owner declared (unload / reload).
///
/// It does NOT uninstall anything, and it does not release slots (spec §6). Uninstalling a live
/// detour races the engine calling through it; SourceMod does not do it either. `s2detour::RemoveAll()`
/// at Unload restores every prologue.
pub(crate) fn drop_owner(owner_id: &str) {
    REGISTRY.with(|r| r.borrow_mut().drop_owner(owner_id));
    // A reload is a new situation, not a continuation of the old one: let the re-entrancy report
    // fire again for this owner rather than inheriting the previous load's once-per-hook silence.
    REENTRANT_REPORTED.with(|r| r.borrow_mut().retain(|(o, _)| o != owner_id));
}

/// `"available"`, or the named reason this hook is not.
pub(crate) fn status(owner_id: &str, name: &str) -> String {
    REGISTRY.with(|r| r.borrow().status(owner_id, name))
}

/// The dispatch plan for a resolved hook (cloned — see `HookPlan`).
pub(crate) fn plan(owner_id: &str, name: &str) -> Option<HookPlan> {
    REGISTRY.with(|r| r.borrow().plan(owner_id, name))
}

/// The `(owner, name)` a shim hook slot belongs to. The reverse of the id baked into the thunk, and
/// how an inbound dispatch finds its descriptor. `None` for an id core never handed out — i.e. a
/// detour installed by a PREVIOUS core (Metamod reload), which degrades to Continue.
pub(crate) fn hook_for_id(hook_id: i32) -> Option<(String, String)> {
    SLOTS.with(|s| {
        s.borrow()
            .iter()
            .find(|(_, sl)| sl.id == hook_id && sl.installed)
            .map(|((o, n), _)| (o.clone(), n.clone()))
    })
}

/// LAZY INSTALL (spec §6). Called on EVERY subscribe, and idempotent by construction: the second
/// call sees `installed` and returns without asking the shim, because a double detour would chain
/// the trampoline to itself.
///
/// Not "install on the first subscriber" but "ensure installed": if the first attempt failed (a
/// stale address after a game update), a later subscribe retries rather than leaving the hook dead
/// for the process. Returns the named reason on failure — the caller WARNs; the subscription itself
/// is still recorded, so the ledger and teardown stay uniform.
pub(crate) fn subscribe(owner_id: &str, name: &str) -> Result<(), String> {
    let (hook_id, shape, addr) = {
        let plan = REGISTRY.with(|r| {
            let reg = r.borrow();
            match reg.hooks.get(&(owner_id.to_string(), name.to_string())) {
                Some(Descriptor::Ready { plan, addr, .. }) => Some((plan.hook_id, plan.shape, *addr)),
                _ => None,
            }
        });
        match plan {
            Some(t) => t,
            None => return Err(status(owner_id, name)),
        }
    };

    let already = SLOTS.with(|s| {
        s.borrow().get(&(owner_id.to_string(), name.to_string())).map(|sl| sl.installed).unwrap_or(false)
    });
    if already {
        return Ok(());
    }

    // The registry borrow is released before the engine op — installing patches bytes, and the op
    // must never run under a borrow core might re-enter.
    match INSTALL.with(|i| i.get())(hook_id, shape, addr) {
        Ok(()) => {
            SLOTS.with(|s| {
                if let Some(sl) = s.borrow_mut().get_mut(&(owner_id.to_string(), name.to_string())) {
                    sl.installed = true;
                }
            });
            note_miss(owner_id, name, None);
            crate::v8host::log_warn(&format!(
                "[engine-hooks] '{}' hook '{}' INSTALLED (slot {})",
                owner_id, name, hook_id
            ));
            Ok(())
        }
        Err(reason) => {
            note_miss(owner_id, name, Some(reason.clone()));
            Err(reason)
        }
    }
}

/// Record (or clear) the LATE degrade reason of a resolved hook, WARNing only when it CHANGES.
///
/// The change-gate is not cosmetic: an arg-view accessor failing on a hook the engine calls every
/// tick would otherwise write a log line every tick.
///
/// An accessor miss LATCHES — it is cleared by a successful install, not by the next successful
/// read. That is deliberate: a param index the shape does not have is a stale binding, which does
/// not heal on its own, and a set/clear per access would turn `status()` into a coin flip and the
/// log into churn. (`gamedata_calls`' `via_miss` clears on success because a schema offset genuinely
/// does resolve later, at map-live; nothing here does.)
pub(crate) fn note_miss(owner_id: &str, name: &str, reason: Option<String>) {
    let changed = REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        match reg.hooks.get_mut(&(owner_id.to_string(), name.to_string())) {
            Some(Descriptor::Ready { miss, .. }) => {
                if *miss == reason {
                    false
                } else {
                    *miss = reason.clone();
                    true
                }
            }
            _ => false,
        }
    });
    if changed {
        if let Some(text) = reason {
            crate::v8host::log_warn(&format!(
                "WARN: [engine-hooks] '{}' hook '{}' degraded: {}",
                owner_id, name, text
            ));
        }
    }
}

/// Hooks that have already reported a re-entrant skip. See `note_reentrant_skip`.
thread_local! {
    static REENTRANT_REPORTED: RefCell<std::collections::HashSet<(String, String)>> =
        RefCell::new(std::collections::HashSet::new());
}

/// Report a dispatch that was SKIPPED because the detour fired while core already held the isolate
/// borrow — AT MOST ONCE per (owner, hook).
///
/// WHY THIS EXISTS. The bypass latch (`bypassWith`) makes "the latch was not armed, therefore the
/// call came from the engine, therefore core is not borrowed" true for the hook's OWN declared
/// outbound descriptor — and only for that one. `bypass_ids_for_call` is scoped to (owner, call
/// name), where SourceMod's `g_pIgnoreTerminateDetour` is global, so any OTHER JS→engine path to
/// the same address (a plugin's own `calls` descriptor, which `engine:calls` permits) arms nothing.
/// The thunk then fires re-entrantly, `fan_out_inner` cannot take the borrow, and the fan-out does
/// not run. Skipping is the SAFE direction and stays — the engine action proceeds unhooked — but
/// before this it also vanished: no log, no `note_miss`, no `status()` change.
///
/// WHY ONCE PER HOOK, and not a counter or a time window. This fires on an ENGINE CALL, so an
/// unbounded report is its own defect — a hooked function the engine drives every tick would write
/// a line per tick. The cap is therefore on the only axis that is bounded by construction: the
/// number of declared hooks (`MAX_HOOKS`), not the call rate. It costs nothing diagnostically,
/// because the condition is STRUCTURAL rather than statistical — some JS→engine path reaches this
/// address without the latch — so the first occurrence carries the whole finding and the
/// thousandth adds nothing. `note_miss` already WARNs only when the reason CHANGES, which would
/// almost do it on its own; the explicit set is what additionally stops a FLIP-FLOP (this reason
/// alternating with an accessor miss on the same hook) from logging once per engine call after all.
/// The set is cleared by `drop_owner`/`reset_all`, so a plugin reload — a genuinely new situation —
/// re-arms the report rather than inheriting a previous load's silence.
pub(crate) fn note_reentrant_skip(owner_id: &str, name: &str) {
    let first = REENTRANT_REPORTED
        .with(|r| r.borrow_mut().insert((owner_id.to_string(), name.to_string())));
    if !first {
        return;
    }
    note_miss(
        owner_id,
        name,
        Some(
            "a dispatch was SKIPPED because the detour fired re-entrantly — core already held the \
             isolate borrow, so no handler could run and the engine call proceeded UNHOOKED. Some \
             JS→engine path reaches this function without arming its bypass latch (only the hook's \
             own 'bypassWith' descriptor arms it). Reported once per hook."
                .to_string(),
        ),
    );
}

/// The hook slots in `owner` whose `bypassWith` names `call_name` — the latches an outbound
/// invocation of that call must arm.
///
/// Only INSTALLED slots are returned: an uninstalled hook has no thunk to take the latch, and
/// arming it would only widen the window the disarm has to close.
pub(crate) fn bypass_ids_for_call(owner_id: &str, call_name: &str) -> Vec<i32> {
    REGISTRY.with(|r| {
        r.borrow()
            .hooks
            .iter()
            .filter_map(|((o, n), d)| {
                if o != owner_id {
                    return None;
                }
                let Descriptor::Ready { plan, bypass_with, .. } = d else { return None };
                if bypass_with.as_deref() != Some(call_name) {
                    return None;
                }
                let installed = SLOTS.with(|s| {
                    s.borrow().get(&(o.clone(), n.clone())).map(|sl| sl.installed).unwrap_or(false)
                });
                if installed { Some(plan.hook_id) } else { None }
            })
            .collect()
    })
}

/// Whole-process teardown (`owner_stores::sweep_reset`). Clears BOTH tables.
///
/// The slot table MUST be cleared here even though it deliberately outlives `drop_owner`: the shim
/// pairs this with `S2_HookResetAll()` at Unload, which forgets its own half. Leaving `installed`
/// set would make the next core's first subscribe take the "already installed" path and never patch
/// — every hook silently dead, which is exactly the failure `S2_HookResetAll` exists to prevent on
/// the other side of the boundary.
pub(crate) fn reset_all() {
    REGISTRY.with(|r| *r.borrow_mut() = HookRegistry::new());
    SLOTS.with(|s| s.borrow_mut().clear());
    REENTRANT_REPORTED.with(|r| r.borrow_mut().clear());
}

// ---------------------------------------------------------------------------
// Tests.
//
// The two engine hops are swapped for stubs (see the `ResolveFn`/`InstallFn` comment): everything
// else — the check order, the named reasons, the slot bookkeeping — is the shipped code.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    /// A descriptor that passes every check: a validator, a known shape, an `expose.ctx`.
    const VALID_GD: &str = r#"{"hooks":{"onX":{"target":{"kind":"signature","name":"Sig","module":"m.so",
                                "pattern":"55 48","resolve":"direct","validate":{"prologue":"55"}},
                                "shape":"this_void","expose":{"ctx":"g"}}}}"#;

    // Count of `install_via_engine` stand-in calls — what proves the lazy-install rule.
    thread_local! {
        static INSTALLS: Cell<usize> = const { Cell::new(0) };
    }

    fn fake_resolve(_t: &serde_json::Value) -> Result<i64, String> {
        // A plausible non-zero .text address. Never dereferenced by anything under test.
        Ok(0x0000_7f00_0040_0000)
    }
    fn counting_install(_id: i32, _shape: i32, _addr: i64) -> Result<(), String> {
        INSTALLS.with(|c| c.set(c.get() + 1));
        Ok(())
    }
    fn refusing_install(_id: i32, _shape: i32, _addr: i64) -> Result<(), String> {
        Err("detour install refused this prologue (relative/rip-relative or undecodable)".to_string())
    }

    /// Put the module in a known state: permission granted for `owner`, both engine hops stubbed,
    /// both tables empty, the install counter zeroed. The whole crate's tests share one process
    /// (single-threaded), so every test starts from here rather than from whatever ran before it.
    fn harness(owner: &str) {
        crate::loader::load_permissions_from_str(&format!(
            r#"{{"engine:hooks":["{}"]}}"#,
            owner
        ))
        .expect("parses");
        reset_all();
        INSTALLS.with(|c| c.set(0));
        RESOLVE.with(|r| r.set(fake_resolve));
        INSTALL.with(|i| i.set(counting_install));
    }

    /// Subscribe twice to the same hook and report how many times the engine install ran.
    fn install_call_count_with_two_subscribes() -> usize {
        harness("@t/x");
        register_owner("@t/x", VALID_GD);
        subscribe("@t/x", "onX").expect("first subscribe installs");
        subscribe("@t/x", "onX").expect("second subscribe is a no-op");
        INSTALLS.with(|c| c.get())
    }

    /// Is the shim still holding a detour for this hook? Core's slot table is the only record of
    /// that — there IS no uninstall op to observe, which is the point of the test that calls this.
    fn detour_still_installed(owner: &str, name: &str) -> bool {
        SLOTS.with(|s| {
            s.borrow().get(&(owner.to_string(), name.to_string())).map(|sl| sl.installed).unwrap_or(false)
        })
    }

    #[test]
    fn a_hook_without_a_validator_is_rejected_by_name() {
        // Mandatory-validator rule (spec §5): a wrong CALL misbehaves, a wrong DETOUR overwrites the
        // prologue of whatever is actually there. Uniqueness alone is not enough.
        harness("@t/x");
        let gd = r#"{"hooks":{"onX":{"target":{"kind":"signature","name":"Sig","pattern":"55"},
                                     "shape":"this_void","expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", gd);
        assert!(
            status("@t/x", "onX").contains("validate"),
            "the reason must name the missing validator, got: {}",
            status("@t/x", "onX")
        );
    }

    /// An EMPTY validator object is the same hole with a lid on it, and must fail the same way.
    #[test]
    fn an_empty_validator_object_is_rejected_too() {
        harness("@t/x");
        let gd = r#"{"hooks":{"onX":{"target":{"kind":"signature","name":"Sig","pattern":"55","validate":{}},
                                     "shape":"this_void","expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", gd);
        assert!(status("@t/x", "onX").contains("validate"), "{}", status("@t/x", "onX"));
    }

    #[test]
    fn an_unknown_shape_is_rejected_by_name() {
        harness("@t/x");
        let gd = r#"{"hooks":{"onX":{"target":{"kind":"signature","name":"Sig","pattern":"55",
                                               "validate":{"prologue":"55"}},
                                     "shape":"this_i32","expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", gd);
        assert!(status("@t/x", "onX").contains("shape"), "{}", status("@t/x", "onX"));
        // And it consumed no slot: a rejected descriptor must not spend one of the 64.
        assert!(!detour_still_installed("@t/x", "onX"));
        assert_eq!(SLOTS.with(|s| s.borrow().len()), 0, "a rejected hook takes no slot");
    }

    #[test]
    fn an_unknown_bypass_target_is_rejected_by_name() {
        harness("@t/x");
        let gd = r#"{"hooks":{"onX":{"target":{"kind":"signature","name":"Sig","pattern":"55",
                                               "validate":{"prologue":"55"}},
                                     "shape":"this_void","bypassWith":"nope","expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", gd);
        assert!(status("@t/x", "onX").contains("bypassWith"), "{}", status("@t/x", "onX"));
    }

    /// The same `bypassWith` naming a call that IS in this owner's tree resolves, and the outbound
    /// invoke path can find the latch to arm.
    #[test]
    fn a_known_bypass_target_arms_that_calls_latch() {
        harness("@t/x");
        let gd = r#"{"signatures":{"Sig":{"linuxsteamrt64":{"module":"m.so","pattern":"55 48",
                                          "resolve":"direct","validate":{"prologue":"55"}}}},
                     "calls":{"doThing":{"receiver":{"kind":"entity"},
                              "target":{"kind":"signature","name":"Sig"},"args":[],"returns":"void"}},
                     "hooks":{"onX":{"target":{"kind":"signature","name":"Sig"},
                              "shape":"this_void","bypassWith":"doThing","expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", gd);
        assert_eq!(status("@t/x", "onX"), "available", "{}", status("@t/x", "onX"));
        // Not installed yet -> no latch to arm. That is the point: arming an uninstalled slot only
        // widens the window the disarm has to close.
        assert!(bypass_ids_for_call("@t/x", "doThing").is_empty());
        subscribe("@t/x", "onX").expect("installs");
        assert_eq!(bypass_ids_for_call("@t/x", "doThing").len(), 1);
        assert!(bypass_ids_for_call("@t/x", "somethingElse").is_empty());
        assert!(bypass_ids_for_call("@other/y", "doThing").is_empty(), "latches are per-owner");
    }

    #[test]
    fn install_happens_on_first_subscribe_only() {
        // Lazy install (spec §6): no subscribers, no patched bytes. The second subscribe must NOT
        // re-install — a double detour would chain the trampoline to itself.
        let installs = install_call_count_with_two_subscribes();
        assert_eq!(installs, 1, "install must be idempotent across subscribes");
    }

    /// The other half of "lazy": REGISTERING a hook patches nothing at all.
    #[test]
    fn registration_alone_installs_nothing() {
        harness("@t/x");
        register_owner("@t/x", VALID_GD);
        assert_eq!(INSTALLS.with(|c| c.get()), 0, "no subscribers means no patched bytes");
        assert!(!detour_still_installed("@t/x", "onX"));
    }

    #[test]
    fn dropping_an_owner_leaves_the_detour_installed() {
        // §6: uninstalling a live detour races the engine calling through it. SM does not do it either.
        harness("@t/x");
        register_owner("@t/x", VALID_GD);
        subscribe("@t/x", "onX").unwrap();
        drop_owner("@t/x");
        assert!(detour_still_installed("@t/x", "onX"));
        assert!(status("@t/x", "onX").contains("not declared"), "the DESCRIPTOR is gone");
    }

    /// A reload re-registers the same descriptor: it must re-use its slot (so `hook_install` takes
    /// its idempotent path) rather than burning a second one out of the 64.
    #[test]
    fn a_reload_reuses_the_same_slot_and_does_not_reinstall() {
        harness("@t/x");
        register_owner("@t/x", VALID_GD);
        subscribe("@t/x", "onX").unwrap();
        let first = plan("@t/x", "onX").expect("ready").hook_id;
        drop_owner("@t/x");
        register_owner("@t/x", VALID_GD);
        let second = plan("@t/x", "onX").expect("ready again").hook_id;
        assert_eq!(first, second, "the same (owner, name) keeps its slot across a reload");
        subscribe("@t/x", "onX").unwrap();
        assert_eq!(INSTALLS.with(|c| c.get()), 1, "the surviving detour is not patched twice");
    }

    /// A refused install is a NAMED degrade on that one hook, not a load failure — and a later
    /// subscribe retries rather than leaving it dead for the process.
    #[test]
    fn a_refused_install_is_named_and_retried() {
        harness("@t/x");
        INSTALL.with(|i| i.set(refusing_install));
        register_owner("@t/x", VALID_GD);
        let err = subscribe("@t/x", "onX").unwrap_err();
        assert!(err.contains("prologue"), "{}", err);
        assert!(status("@t/x", "onX").contains("prologue"), "{}", status("@t/x", "onX"));
        assert!(!detour_still_installed("@t/x", "onX"));
        INSTALL.with(|i| i.set(counting_install));
        subscribe("@t/x", "onX").expect("a later subscribe retries");
        assert!(detour_still_installed("@t/x", "onX"));
        assert_eq!(status("@t/x", "onX"), "available", "the reason clears on success");
    }

    #[test]
    fn registry_degrades_when_permission_denied() {
        // The allow-list is HOST-GLOBAL and shared by the whole crate's tests. An allow-list that
        // does not NAME this owner is the same DENY the never-loaded (default-deny) posture gives.
        harness("@t/other");
        register_owner("@t/x", VALID_GD);
        assert!(status("@t/x", "onX").contains("not permitted"), "{}", status("@t/x", "onX"));
        assert_eq!(SLOTS.with(|s| s.borrow().len()), 0, "an unauthorized hook takes no slot");
    }

    /// `engine:calls` is NOT `engine:hooks` (spec §7): granting the ability to CALL an engine
    /// function must not grant the ability to DETOUR it.
    #[test]
    fn the_engine_calls_permission_does_not_grant_hooks() {
        reset_all();
        crate::loader::load_permissions_from_str(r#"{"engine:calls":["@t/x"]}"#).expect("parses");
        RESOLVE.with(|r| r.set(fake_resolve));
        INSTALL.with(|i| i.set(counting_install));
        register_owner("@t/x", VALID_GD);
        assert!(status("@t/x", "onX").contains("not permitted"), "{}", status("@t/x", "onX"));
    }

    /// The game package's hooks register without an allow-list entry — the A5b reserved-owner
    /// exemption, which travels with the ENTRY POINT and not with the id.
    #[test]
    fn game_package_hooks_bypass_the_allow_list() {
        harness("@t/nobody");
        register_game_package("@demo/gp", VALID_GD);
        let owner = crate::gamedata_calls::reserved_owner_id("@demo/gp");
        let st = status(&owner, "onX");
        assert!(!st.contains("not permitted"), "the runtime is not allow-listed: {st}");
        assert_eq!(st, "available", "{st}");
        drop_owner(&owner);
    }

    /// A `.s2sp` may never register under the reserved owner id — the second latch behind
    /// `loader::read_s2sp`'s refusal, and it matters MORE here than for calls (a hook patches bytes).
    #[test]
    fn a_plugin_cannot_register_under_the_reserved_owner_id() {
        harness("@t/x");
        let spoofed = crate::gamedata_calls::reserved_owner_id("@demo/spoof");
        register_plugin(&spoofed, VALID_GD);
        assert!(
            status(&spoofed, "onX").contains("not declared"),
            "a plugin registration under a reserved owner id must register NOTHING"
        );
    }

    #[test]
    fn status_of_an_unknown_hook_is_named() {
        harness("@t/x");
        assert!(status("@t/x", "nope").contains("not declared"));
    }

    #[test]
    fn subscribing_to_an_unknown_hook_reports_the_named_reason() {
        harness("@t/x");
        let err = subscribe("@t/x", "nope").unwrap_err();
        assert!(err.contains("not declared"), "{}", err);
        assert_eq!(INSTALLS.with(|c| c.get()), 0, "nothing is patched for a hook that does not exist");
    }

    /// `expose.ctx` is mandatory and must be emittable as a TypeScript property name.
    #[test]
    fn a_hook_with_no_or_bad_ctx_namespace_is_rejected_by_name() {
        harness("@t/x");
        let missing = r#"{"hooks":{"onX":{"target":{"kind":"signature","name":"S","pattern":"55",
                                          "validate":{"prologue":"55"}},"shape":"this_void"}}}"#;
        register_owner("@t/x", missing);
        assert!(status("@t/x", "onX").contains("expose.ctx"), "{}", status("@t/x", "onX"));

        let bad = r#"{"hooks":{"onY":{"target":{"kind":"signature","name":"S","pattern":"55",
                                      "validate":{"prologue":"55"}},"shape":"this_void",
                                      "expose":{"ctx":"game rules"}}}}"#;
        register_owner("@t/x", bad);
        assert!(status("@t/x", "onY").contains("plain identifier"), "{}", status("@t/x", "onY"));
    }

    /// `params` are positional and become property names; `mutable` must be a subset of them.
    #[test]
    fn param_and_mutable_grammar_is_enforced_by_name() {
        harness("@t/x");
        let dup = r#"{"hooks":{"onDup":{"target":{"kind":"signature","name":"S","pattern":"55",
                                        "validate":{"prologue":"55"}},"shape":"this_f32_i32_i32_i32",
                                        "params":["a","a","c","d"],"expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", dup);
        assert!(status("@t/x", "onDup").contains("more than once"), "{}", status("@t/x", "onDup"));

        let bad_ident = r#"{"hooks":{"onBad":{"target":{"kind":"signature","name":"S","pattern":"55",
                                              "validate":{"prologue":"55"}},"shape":"this_void",
                                              "params":["2delay"],"expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", bad_ident);
        assert!(status("@t/x", "onBad").contains("plain identifier"), "{}", status("@t/x", "onBad"));

        let stray = r#"{"hooks":{"onStray":{"target":{"kind":"signature","name":"S","pattern":"55",
                                            "validate":{"prologue":"55"}},"shape":"this_f32_i32_i32_i32",
                                            "params":["delay","reason"],"mutable":["nope"],
                                            "expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", stray);
        assert!(status("@t/x", "onStray").contains("not one of this hook's params"), "{}", status("@t/x", "onStray"));
    }

    /// `mutable` decides which params the generated view may write back. It is carried per-index,
    /// so a regression that transposed it would silently make a read-only param writable.
    #[test]
    fn mutable_is_recorded_per_param_index() {
        harness("@t/x");
        let gd = r#"{"hooks":{"onX":{"target":{"kind":"signature","name":"S","pattern":"55",
                                     "validate":{"prologue":"55"}},"shape":"this_f32_i32_i32_i32",
                                     "params":["delay","reason","u3","u4"],"mutable":["delay","reason"],
                                     "expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", gd);
        let p = plan("@t/x", "onX").expect("ready");
        assert_eq!(p.params, vec!["delay", "reason", "u3", "u4"]);
        assert_eq!(p.writable, vec![true, true, false, false]);
        assert_eq!(p.shape, 1, "this_f32_i32_i32_i32");
    }

    /// The receiver is opt-in and tagged, and its name may not collide with a param's.
    #[test]
    fn receiver_grammar_is_enforced_by_name() {
        harness("@t/x");
        let no_as = r#"{"hooks":{"onA":{"target":{"kind":"signature","name":"S","pattern":"55",
                                        "validate":{"prologue":"55"}},"shape":"this_void",
                                        "receiver":{"kind":"entity"},"expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", no_as);
        assert!(status("@t/x", "onA").contains("'as'"), "{}", status("@t/x", "onA"));

        let bad_kind = r#"{"hooks":{"onB":{"target":{"kind":"signature","name":"S","pattern":"55",
                                           "validate":{"prologue":"55"}},"shape":"this_void",
                                           "receiver":{"kind":"interface"},"expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", bad_kind);
        assert!(status("@t/x", "onB").contains("unsupported receiver kind"), "{}", status("@t/x", "onB"));

        let collide = r#"{"hooks":{"onC":{"target":{"kind":"signature","name":"S","pattern":"55",
                                          "validate":{"prologue":"55"}},"shape":"this_f32_i32_i32_i32",
                                          "params":["player"],"receiver":{"kind":"entity","as":"player"},
                                          "expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", collide);
        assert!(status("@t/x", "onC").contains("collides"), "{}", status("@t/x", "onC"));

        let ok = r#"{"hooks":{"onD":{"target":{"kind":"signature","name":"S","pattern":"55",
                                     "validate":{"prologue":"55"}},"shape":"this_void",
                                     "receiver":{"kind":"entity","as":"player"},"expose":{"ctx":"g"}}}}"#;
        register_owner("@t/x", ok);
        assert_eq!(plan("@t/x", "onD").expect("ready").receiver.as_deref(), Some("player"));
        // The default is "surface nothing": most detour receivers are not entities.
        register_owner("@t/x", VALID_GD);
        assert_eq!(plan("@t/x", "onX").expect("ready").receiver, None);
    }

    /// A resolve failure is the shim's own reason, verbatim — core never re-words it.
    #[test]
    fn a_resolve_failure_carries_the_shims_reason() {
        harness("@t/x");
        fn failing(_t: &serde_json::Value) -> Result<i64, String> {
            Err("signature is ambiguous (>1 match — tighten it)".to_string())
        }
        RESOLVE.with(|r| r.set(failing));
        register_owner("@t/x", VALID_GD);
        assert_eq!(status("@t/x", "onX"), "signature is ambiguous (>1 match — tighten it)");
    }

    /// A resolved-but-address-less descriptor must degrade, never install at 0.
    #[test]
    fn a_zero_address_is_refused() {
        harness("@t/x");
        fn zero(_t: &serde_json::Value) -> Result<i64, String> {
            Err("resolved descriptor has no address".to_string())
        }
        RESOLVE.with(|r| r.set(zero));
        register_owner("@t/x", VALID_GD);
        assert!(status("@t/x", "onX").contains("no address"), "{}", status("@t/x", "onX"));
        assert_eq!(INSTALLS.with(|c| c.get()), 0);
    }

    /// Gamedata with no `hooks` section is the normal case for almost every plugin.
    #[test]
    fn gamedata_without_hooks_registers_nothing_and_warns_nothing() {
        harness("@t/x");
        register_owner("@t/x", r#"{"calls":{"doThing":{"target":{"kind":"signature","pattern":"55"}}}}"#);
        assert!(status("@t/x", "anything").contains("not declared"));
    }

    /// Malformed gamedata degrades the OWNER's hooks, not the load.
    #[test]
    fn malformed_gamedata_registers_nothing() {
        harness("@t/x");
        register_owner("@t/x", "{not json");
        assert!(status("@t/x", "onX").contains("not declared"));
    }

    /// Slots are handed out from a bounded pool and exhaustion is a NAMED degrade, not a panic and
    /// not an out-of-range install (which the shim would refuse anyway, one hook too late).
    #[test]
    fn slot_exhaustion_is_a_named_degrade() {
        harness("@t/x");
        // Fill the pool with distinct addresses so no two share a slot.
        SLOTS.with(|s| {
            let mut slots = s.borrow_mut();
            for i in 0..MAX_HOOKS {
                slots.insert(
                    ("@t/filler".to_string(), format!("h{}", i)),
                    Slot { id: i, shape: 0, addr: 0x1000 + i as i64, installed: true },
                );
            }
        });
        register_owner("@t/x", VALID_GD);
        assert!(status("@t/x", "onX").contains("no free hook slot"), "{}", status("@t/x", "onX"));
    }

    /// The id → (owner, name) reverse lookup an inbound dispatch depends on. An id core never
    /// handed out — a detour installed by a PREVIOUS core — resolves to nothing and degrades to
    /// Continue instead of dispatching into some other hook's subscribers.
    #[test]
    fn hook_for_id_only_answers_for_installed_slots() {
        harness("@t/x");
        register_owner("@t/x", VALID_GD);
        let id = plan("@t/x", "onX").expect("ready").hook_id;
        assert_eq!(hook_for_id(id), None, "an uninstalled slot cannot be dispatched to");
        subscribe("@t/x", "onX").unwrap();
        assert_eq!(hook_for_id(id), Some(("@t/x".to_string(), "onX".to_string())));
        assert_eq!(hook_for_id(MAX_HOOKS + 5), None, "an id core never handed out");
        assert_eq!(hook_for_id(-1), None);
    }

    /// The shape mirror is an ABI. If this list ever disagrees with the shim's `kShapes`, a hook
    /// installs a thunk with the WRONG signature — so the ids are pinned here as well as gated by
    /// `scripts/check-hook-shapes.sh`.
    #[test]
    fn the_shape_vocabulary_is_pinned() {
        assert_eq!(shape_id("this_void"), Some(0));
        assert_eq!(shape_id("this_f32_i32_i32_i32"), Some(1));
        assert_eq!(shape_id("this_i32"), None);
        assert_eq!(shape_id(""), None);
    }

    /// Whole-process teardown clears BOTH tables — the slot table included, because the shim's
    /// `S2_HookResetAll()` forgets its half at the same moment.
    #[test]
    fn reset_all_clears_the_slot_table_too() {
        harness("@t/x");
        register_owner("@t/x", VALID_GD);
        subscribe("@t/x", "onX").unwrap();
        assert!(detour_still_installed("@t/x", "onX"));
        reset_all();
        assert!(!detour_still_installed("@t/x", "onX"), "a re-inited core must patch again");
        assert!(status("@t/x", "onX").contains("not declared"));
    }
}
