# SDKHooks virtuals + natives Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the wiki SDKHooks catalog on top of PR #134: per-entity VP hooks, `SetTransmit` on CheckTransmit, remaining damage types, and `SDKHooks.takeDamage` / `dropWeapon`.

**Architecture:** Five atomic stacked PRs. Shim owns SourceHook / CheckTransmit / DTA; core owns the JS table already in `sdkhooks.rs`. VP slots are derived at load from `gamedata/sdkhooks/` signatures + `vtable-member`. Owners gate grows an `extension` kind so shim may name those keys.

**Tech Stack:** SourceHook `SH_DECL_MANUALHOOK` + `SH_MANUALHOOK_RECONFIGURE`, `s2vtable::GetVTableByName`, V8 natives, `entity_live`, `@s2script/sdk` overloads, isolate tests (`cargo +stable test -p s2script-core <substring>`).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-30-sdkhooks-virtuals-design.md`
- Base of the stack: `cursor/sdkhooks-virtuals-a8c9` (spec) on `cursor/sdkhooks-a8c9` (PR #134)
- 0.x minor `@s2script/sdk`
- `SDKHook(entity, SDKHookType.Think, cb)` for every wiki hook; `SDKHooks.takeDamage` / `dropWeapon` only for the two natives
- Wiki `Action` = `HookResult`; omit return = Continue
- Unknown type string **throws**; wiki name with failed/missing descriptor **returns false**
- Gamedata owner `gamedata/sdkhooks/` (extension). DTA + CheckTransmit stay `gamedata/core/`
- `kGamedataOwners[]` braces: **only owner-name string literals** (gate regex). Kind is `GdOwnerKind::Core|Game|Extension` with no quotes
- Do not rustfmt all of `v8host.rs`
- Isolate tests: `cargo +stable test -p s2script-core <substring>` (no `|` filter, no `--test-threads`)
- Do not wire `pawn.dropActiveWeapon()`. Do not add DHooks. Do not invent `SetTransmitPost`
- One PR per slice; stack on the parent; each `make ci` green alone (Docker `test-gate.sh` miss is benign for pure-npm tasks)

**PR map (stack, bottom → top):**

| PR | Branch suffix | Delivers |
|----|---------------|----------|
| A | `sdkhooks-vp-touch` | extension owner + gate + VP primitive + Touch family |
| B | `sdkhooks-lifecycle` | Spawn/Think/Use/GetMaxHealth/ShouldCollide/GroundEnt/VPhysics/PrePostThink/CanBeAutobalanced |
| C | `sdkhooks-settransmit` | SetTransmit on existing CheckTransmit POST, AND-merge with `Transmit.setVisibleTo` |
| D | `sdkhooks-weapons-damage` | Weapon*/Reload + OnTakeDamagePost/Alive/TraceAttack/FireBulletsPost |
| E | `sdkhooks-natives` | `SDKHooks.takeDamage` / `dropWeapon` + `bypassHooks` latch |

PRs B–E start after A is on the remote. B and C do not share files (shim VP table vs CheckTransmit); D and E wait on A’s table/kind helper.

---

## File map (PR A)

- Create: `gamedata/sdkhooks/master.gamedata.jsonc`, `gamedata/sdkhooks/game.cs2.jsonc`
- Modify: `scripts/check-gamedata-owners.sh`, `shim/src/s2script_mm.cpp` (owner table + VP install), `shim/src/s2script_mm.h`, `core/engine-ops.jsonc` + regen, `core/src/sdkhooks.rs`, `core/js/prelude.js`, `core/src/ffi.rs`, `packages/sdk/sdkhooks.d.ts`, `.changeset/`
- Possibly create: `shim/src/sdkhooks_vp.cpp` if the Load() function would grow another 400 lines — prefer a new file over stuffing `s2script_mm.cpp`

---

### Task 1: Extension owner + owners gate

**Files:**
- Create: `gamedata/sdkhooks/master.gamedata.jsonc`, `gamedata/sdkhooks/game.cs2.jsonc` (empty `signatures: {}` is OK for this task; Touch rows land in Task 4)
- Modify: `shim/src/s2script_mm.cpp` (`s_gdSdkhooks`, `kGamedataOwners`), `scripts/check-gamedata-owners.sh`

**Interfaces:**
- Produces: loader opens owner `sdkhooks`; gate treats `GdOwnerKind::Extension` like core (keys MAY appear in shim/core) and `Game` like today’s `cs2` (keys must NOT)

- [ ] **Step 1:** Add owner storage next to `s_gdGame` (no quoted kind strings inside the array):

```cpp
enum class GdOwnerKind { Core, Game, Extension };
static GameConfig s_gdSdkhooks;
static std::string s_gdErrorSdkhooks;
struct GamedataOwner {
    const char* name;
    GameConfig* cfg;
    std::string* error;
    GdOwnerKind kind;
};
static const GamedataOwner kGamedataOwners[] = {
    { "core", &s_gdCore, &s_gdErrorCore, GdOwnerKind::Core },
    { "cs2",  &s_gdGame, &s_gdErrorGame, GdOwnerKind::Game },
    { "sdkhooks", &s_gdSdkhooks, &s_gdErrorSdkhooks, GdOwnerKind::Extension },
};
```

Crash fingerprint / `Load()` already walk `kGamedataOwners` by `.name` — they keep working. Any site that assumed a 3-field struct must compile.

- [ ] **Step 2:** `gamedata/sdkhooks/master.gamedata.jsonc`:

```jsonc
{
  "files": [
    { "file": "game.cs2.jsonc", "game": "csgo" }
  ]
}
```

`game.cs2.jsonc`: standard DO-NOT-EDIT header (custom/ is the hot-fix), owner comment `sdkhooks`, `"signatures": {}`.

- [ ] **Step 3:** Amend `scripts/check-gamedata-owners.sh`. Keep extracting owner **names** as quoted strings in `kGamedataOwners[]`. Pair each `{ "name"` with the following `GdOwnerKind::X`:

```python
# After loader_owners = set(re.findall(...))
kind_by_owner = {}
for m in re.finditer(
    r'\{\s*"([^"]+)"\s*,.*?GdOwnerKind::(Core|Game|Extension)',
    table.group(1), re.S):
    kind_by_owner[m.group(1)] = m.group(2)
```

Rules:
- `Core` / missing kind: today’s core rule (every key named in shim/core)
- `Game`: today’s non-core rule (no key named in shim/core)
- `Extension`: keys MAY be named in shim/core (same as Core’s “must be named” if we add signatures in a later task; empty `{}` has no keys)

If `sdkhooks` is in `kGamedataOwners` but `gamedata/sdkhooks/` is missing → fail (already). Inverse still fails.

- [ ] **Step 4:** Run `bash scripts/check-gamedata-owners.sh`. Expected: `check-gamedata-owners: ownership rule holds for every entry`

- [ ] **Step 5:** Commit `feat(gamedata): sdkhooks extension owner + gate kind`

---

### Task 2: Types + prelude allowlist (Touch family)

**Files:**
- Modify: `packages/sdk/sdkhooks.d.ts`, `core/js/prelude.js`, `core/src/sdkhooks.rs` tests that currently expect throw on `"OnTouch"`
- Create: `.changeset/sdkhooks-virtuals.md` (`@s2script/sdk` minor)

**Interfaces:**
- Produces: `SDKHookType.StartTouch|Touch|EndTouch|Blocked` and matching `*Post`; overloads `(entity, other: EntityRef | null) => HookResultValue | void` (pre) / `void` (post)
- Consumes: existing `SDKHook` / `SDKUnhook` boolean API

Wiki names, not `OnTouch`. Update `sdkhook_prelude_throws_on_unsupported_type` to use a garbage string `"NotAType"` (still throws). A Touch hook with no VP op returns **false** (isolate has no shim).

Prelude:

```js
var SDK_HOOK_TYPES = {
  OnTakeDamage: "OnTakeDamage",
  StartTouch: "StartTouch", Touch: "Touch", EndTouch: "EndTouch", Blocked: "Blocked",
  StartTouchPost: "StartTouchPost", TouchPost: "TouchPost",
  EndTouchPost: "EndTouchPost", BlockedPost: "BlockedPost",
};
function SDKHook(entity, type, callback) {
  if (entity == null || typeof entity.index !== "number" || typeof entity.id !== "number") return false;
  if (!Object.prototype.hasOwnProperty.call(SDK_HOOK_TYPES, type) &&
      SDK_HOOK_TYPES[type] !== type) {
    // type is a value from the object OR a literal equal to a value
  }
  var known = false;
  for (var k in SDK_HOOK_TYPES) if (SDK_HOOK_TYPES[k] === type) { known = true; break; }
  if (!known) throw new Error("s2script: SDKHook type '" + type + "' is not supported");
  if (typeof callback !== "function") throw new TypeError("s2script: SDKHook callback must be a function");
  return __s2_sdkhook(entity.index, entity.id, type, callback);
}
```

Export `SDKHookType: SDK_HOOK_TYPES`. Native still decides backed vs not.

- [ ] **Step 1:** Overloads + `SDKHookType` members in `sdkhooks.d.ts`. JSDoc: Touch `Handled` skips the original virtual (SUPERCEDE). Document every exported symbol (doccov).
- [ ] **Step 2:** Prelude known-set. Change isolate test: `"OnTouch"` throw → `"NotAType"` throw; add `SDKHook(..., SDKHookType.Touch, fn)` returns `"false"` without VP op.
- [ ] **Step 3:** `cd packages/sdk && npm test` plus `cargo +stable test -p s2script-core sdkhook`
- [ ] **Step 4:** Commit `feat(sdk): SDKHookType Touch family`

---

### Task 3: Core table + VP add/remove ops + Touch dispatch (TDD)

**Files:**
- Modify: `core/engine-ops.jsonc` (append-only), regen with `python3 scripts/gen-engine-ops.py`
- Modify: `core/src/sdkhooks.rs`, `core/src/ffi.rs`, `core/src/v8host.rs` (one `extern "C"` dispatch), `shim/include/s2script_core.h` if ffi needs a new export (hand-written, not generated)

**Ops (ABI tail):**

```jsonc
{
  "name": "sdkhook_vp_add",
  "alias": "SdkhookVpAddFn",
  "rust": "extern \"C\" fn(c_int, c_int, *const c_char, c_int) -> c_int",
  "group": "SDKHooks VP — install per-entity SourceHook",
  "c_ret": "int",
  "c_params": "int index, int serial, const char* type, int post",
  "shim": "s2_sdkhook_vp_add"
},
{
  "name": "sdkhook_vp_remove",
  "alias": "SdkhookVpRemoveFn",
  "rust": "extern \"C\" fn(c_int, c_int, *const c_char, c_int) -> c_int",
  "c_ret": "int",
  "c_params": "int index, int serial, const char* type, int post",
  "shim": "s2_sdkhook_vp_remove"
},
{
  "name": "sdkhook_vp_drop",
  "alias": "SdkhookVpDropFn",
  "rust": "extern \"C\" fn(c_int, c_int) -> c_int",
  "c_ret": "int",
  "c_params": "int index, int serial",
  "shim": "s2_sdkhook_vp_drop"
}
```

`type` is the wiki name (`Touch`). `post` is 0/1. Return 1 = ok, 0 = degrade.

**Core `s2_sdkhook`:** after books-gate, if kind is OnTakeDamage → today’s path. If Touch family → if this is the first callback for `(entity_id, kind)`, call `sdkhook_vp_add` with engine serial from `entity_live::engine_serial_for`; if op missing or returns 0, **do not push** the entry, return false. If kind unknown to native (not in the known set) return false (prelude already threw).

**Unhook / drop_entity / owner unload:** if last callback for that `(entity_id, kind)` is gone, `sdkhook_vp_remove`. `drop_entity` also `sdkhook_vp_drop` (needs index+serial — `drop_entity` today only has host id). **Change:** `drop_entity` must receive index+serial or look them up. Prefer extending `ffi` on_deleted to pass index+serial into `sdkhooks::drop_entity(index, serial, id)` so shim can SH_REMOVE by pointer.

**Dispatch:** `s2script_core_dispatch_sdkhook_touch(int this_index, int this_serial, int other_handle, int post, const char* type) -> int`  
Returns collapsed HookResult 0–3. Shim SUPERCEDE when `>= Handled`.

Snapshot like damage: callbacks where `entity_id` matches `this` and `kind == type`. Args: build `EntityRef` for this and other (other may be null). Post hooks: fire all, ignore return. Pre: collapse max, Stop short-circuits.

Isolate: fake `sdkhook_vp_add` returning 1; call `dispatch_sdkhook_touch` from the test (export `pub(crate)` like `dispatch_damage`). Cover: first add calls op; second add same entity+type does not; Handled does not skip later; Stop skips; unhook last calls remove; destroy drop; no op → false and no table entry.

- [ ] **Step 1:** Failing isolate tests (`sdkhook_touch_*`).
- [ ] **Step 2:** `cargo +stable test -p s2script-core sdkhook_touch` FAIL.
- [ ] **Step 3:** Ops + native + dispatch. Regen engine-ops. Wire ffi.
- [ ] **Step 4:** Tests green. Commit `feat(core): SDKHook Touch table and VP ops`

---

### Task 4: Shim VP primitive + Touch signatures

**Files:**
- Create: `shim/src/sdkhooks_vp.cpp` + header (keep `s2script_mm.cpp` from growing)
- Modify: `gamedata/sdkhooks/game.cs2.jsonc` (Touch/StartTouch/EndTouch/Blocked signatures), CMake for the new .cpp, Load() resolve loop

**Mechanism:**

```cpp
SH_DECL_MANUALHOOK1_void(MHook_StartTouch, 0, 0, 0, CBaseEntity *);
SH_DECL_MANUALHOOK1_void(MHook_Touch,      0, 0, 0, CBaseEntity *);
SH_DECL_MANUALHOOK1_void(MHook_EndTouch,   0, 0, 0, CBaseEntity *);
SH_DECL_MANUALHOOK1_void(MHook_Blocked,    0, 0, 0, CBaseEntity *);
```

At Load, for each key in `{StartTouch,Touch,EndTouch,Blocked}`:
1. `s_gdSdkhooks.signatures.find(name)`
2. `ResolveSigValidated` on **that** GameConfig’s SigSpec (do not read `s_gdCore`)
3. Parse `validate` JSON for `vtable-member` class (reuse `call_validate` vtable-member walk)
4. Scan `GetVTableByName("libserver.so", class)` for the address → slot
5. `SH_MANUALHOOK_RECONFIGURE(MHook_Touch, slot, 0, 0)`
6. Banner `gamedata OK Touch (slot N)`

Handlers: META_RES from `s2script_core_dispatch_sdkhook_touch`. Pack `this` and `pOther` the same way DTA packs the victim (existing handle helpers). `RETURN_META(MRES_SUPERCEDE)` when result >= 2; else `MRES_IGNORED`. Post hook: dispatch with post=1, always IGNORED.

`s2_sdkhook_vp_add`: `s2_ent_resolve` → if already hooked `(ptr,type,post)` refcount++; else `SH_ADD_MANUALHOOK` + refcount 1. Missing slot → 0.

RE: self-resolve on the live `libserver.so` (docker CS2 data or container). CSSharp/SM slot numbers are HINTS only. If this environment has no binary, ship the signature with a comment `// unresolved in this workspace — live-gate must print gamedata OK Touch` and degrade until the sniper/live gate fills it. Do **not** ship a borrowed slot as `offsets`.

- [ ] **Step 1:** CMake + empty add/remove/drop returning 0.
- [ ] **Step 2:** Resolve loop + RECONFIGURE + handlers.
- [ ] **Step 3:** Signatures in `gamedata/sdkhooks/game.cs2.jsonc`. Gate: extension keys named in shim (`"Touch"` in sdkhooks_vp.cpp).
- [ ] **Step 4:** `bash scripts/check-gamedata-owners.sh`. Commit `feat(shim): per-entity SDKHook Touch VP hooks`

---

### Task 5: PR A gates

- [ ] `cargo +stable test -p s2script-core sdkhook`
- [ ] `bash scripts/check-gamedata-owners.sh`
- [ ] `python3 scripts/gen-engine-ops.py --check`
- [ ] `bash scripts/check-plugins-typecheck.sh` / `cd packages/sdk && npm test`
- [ ] Push `cursor/sdkhooks-vp-touch-a8c9`, PR onto `cursor/sdkhooks-virtuals-a8c9`

---

## PRs B–E (after A is pushed)

Execute as stacked branches. Parallelize B (lifecycle VP) and C (SetTransmit) — different shim sites.

### PR B — lifecycle virtuals

Same VP primitive. New SH_DECL per ABI:
- `this_void` → Spawn, Think, PreThink, PostThink, VPhysicsUpdate, GroundEntChangedPost
- `Use` → `(CBaseEntity *activator, CBaseEntity *caller, int useType, float value)` — confirm CS2 ABI before shipping; else degrade
- `GetMaxHealth` → mutate `{ maxHealth }`
- `ShouldCollide` → `bool` return, not HookResult
- `CanBeAutobalanced` → `(origRet: bool) => bool`, requires Client resolve

Each type: signature row keyed by wiki name, `vtable-member` class (`CBaseEntity` vs `CCSPlayerPawn`). Isolate tests per family (one file region in `sdkhooks.rs`).

### PR C — SetTransmit

No new SourceHook. In `Hook_CheckTransmit` POST, after `Transmit.setVisibleTo` bit clears: if SetTransmit table non-empty, for each viewer, for each hooked entity whose bit is still set, `dispatch` `(entity, Client.fromSlot(v))`. Handled/Stop → clear bit. Empty table → skip (today’s cost). Types + prelude member `SetTransmit`. Isolate: fake snapshot of (entity, slot) pairs.

### PR D — weapons + remaining damage

Weapon* / Reload: VP on the hooked instance’s class; missing ItemServices virtual → type stays in `SDKHookType`, `SDKHook` returns false.  
OnTakeDamagePost / Alive: fan-out from existing DTA path (post after trampoline; Alive only if RE finds a distinct function — otherwise degrade by name). TraceAttack / FireBulletsPost: own signature or degrade.

### PR E — natives

```ts
SDKHooks.takeDamage(entity, inflictor, attacker, damage, damageType?, weapon?, force?, pos?, bypassHooks?)
SDKHooks.dropWeapon(client, weapon, target?, velocity?, bypassHooks?)
```

`bypassHooks` default true: thread-local latch; DTA/WeaponDrop dispatch returns immediately. `takeDamage` fills `CTakeDamageInfo` and calls existing DTA trampoline (or TakeDamage). `dropWeapon`: new sdkhooks signature, not vtable 24. Returns boolean. Prelude `__s2pkg_sdkhooks.SDKHooks`. Do not un-stub `pawn.dropActiveWeapon`.

---

## Live gate (PR A minimum)

On `s2script-cs2`: boot log `gamedata OK Touch (slot N)` (or FAIL named). Cookbook or a throwaway plugin: `SDKHook(trigger, SDKHookType.Touch, ...)`, walk a bot in, RCON/console shows the callback. `RestartCount=0`.
