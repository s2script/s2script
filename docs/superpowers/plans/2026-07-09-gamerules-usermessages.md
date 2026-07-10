# Game Rules + General UserMessages — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `GameRules` accessor (read `CCSGameRules` state) and a general `UserMessage` sender (any protobuf user message + typed CS2 sugar `Fade`/`Shake`/`HintText`, wiring `sm_blind`), plus the reusable `Entity.findByClass` primitive game rules needs.

**Architecture:** Two new engine primitives (via `S2EngineOps` ops ABI-appended after `entity_spawn_kv`): `entity_find_by_class` (shim iterates the entity-identity list by designer-name) and a `user_message_*` family (generalize the proven SayText2 protobuf-reflection send path). Game rules = find the `cs_gamerules` proxy via `findByClass`, then read `CCSGameRules` fields through `m_pGameRules` with the existing serial-gated `readVia` pointer-chain nav. UserMessage sugar + GameRules field names live only in the CS2 game layer.

**Tech Stack:** Rust core (`core/src/v8host.rs`, rusty_v8), C++ shim (`shim/src/s2script_mm.cpp`, hl2sdk cs2 + protobuf reflection), TypeScript plugins, `games/cs2/js/pawn.js` game-package runtime.

## Global Constraints

- **The core owns every engine touchpoint; dependencies point game → core, never core → game.** `entity_find_by_class` and the `user_message_*` send machinery are engine-generic (Source2 concepts) → core/shim. `CCSGameRules`/`CCSGameRulesProxy`/`cs_gamerules`/`CUserMessageFade`/`CUserMessageShake` names and all sugar are CS2 → `games/cs2/js/pawn.js` + `packages/cs2/index.d.ts`. Both boundary gates (`scripts/check-core-boundary.sh`, `scripts/test-boundary-nameleak.sh`) must stay green.
- **ABI-append discipline (mandatory).** Every new op is appended **after `entity_spawn_kv`** (the current last op) — never inserted mid-struct — byte-identical across FIVE touchpoints: (1) the C header typedef + struct field in `shim/include/s2script_core.h`; (2) the Rust `type XFn` + `pub X: Option<XFn>` in `core/src/v8host.rs`; (3) **both** in-isolate test op-structs in `core/src/v8host.rs` (the two literals that currently end `entity_spawn_kv: None,` — near lines 8316 and 8789 — get the new fields as `None`); (4) the shim `ops.X = &impl;` assignment in `shim/src/s2script_mm.cpp`; (5) the `mock_event_ops()` helper if a behavioral test uses it. Task 1 appends `entity_find_by_class`; Task 2 appends the six `user_message_*` ops after it. **Run tasks in order** so the append order is deterministic.
- **Degrade-never-crash.** Every new native wraps its body in `std::panic::catch_unwind(AssertUnwindSafe(...))` and sets a safe default first (`[]` / `false` / `null`). Every shim op null-guards its interfaces and returns a no-op default. A missing op → the JS degrades silently.
- **Tests run serial:** `.cargo/config.toml` sets `RUST_TEST_THREADS=1`. Run core tests with `cd core && cargo test`.
- **No raw pointer crosses to JS.** `findByClass` returns serial-gated `EntityRef`s built via `build_entity_ref`; the usermessage/entity pointers stay shim-side.

---

## File Structure

- `shim/include/s2script_core.h` — 7 new op typedefs + struct fields (Task 1: 1, Task 2: 6).
- `core/src/v8host.rs` — 7 Rust op mirrors + both test op-structs; 7 new natives; `Entity.findByClass` + `UserMessage` prelude JS; `__s2pkg_usermessages`; in-isolate tests.
- `shim/src/s2script_mm.cpp` — 7 shim op impls + `ops.` assignments.
- `packages/entity/index.d.ts` — `Entity.findByClass` type (Task 1).
- `packages/usermessages/{package.json,index.d.ts}` — new types-only package (Task 2).
- `games/cs2/js/pawn.js` — `GameRules` accessor (Task 3) + `Fade`/`Shake`/`HintText` sugar (Task 4); export into `__s2pkg_cs2`.
- `packages/cs2/index.d.ts` — `GameRules`/`Fade`/`Shake`/`HintText` types (Tasks 3–4).
- `plugins/funcommands/src/plugin.ts` — `sm_blind` (Task 4).
- `plugins/gamerules-usermsg-demo/{package.json,tsconfig.json,src/plugin.ts}` — demo (Task 5).

---

## Task 1: `entity_find_by_class` op + `Entity.findByClass`

**Files:**
- Modify: `shim/include/s2script_core.h` (typedef + struct field after `entity_spawn_kv`)
- Modify: `core/src/v8host.rs` (op mirror + both test structs + native + prelude + test)
- Modify: `shim/src/s2script_mm.cpp` (`s2_entity_find_by_class` + `ops.` assignment)
- Modify: `packages/entity/index.d.ts`

**Interfaces:**
- Produces: op `int entity_find_by_class(const char* className, int* outIndices, int* outSerials, int maxCount)` (returns total match count; fills first `maxCount`); native `__s2_entity_find_by_class(className) -> EntityRef[]`; `Entity.findByClass(className: string): EntityRef[]` (in `__s2pkg_entity`).

- [ ] **Step 1: C header** — in `shim/include/s2script_core.h`, after the `entity_spawn_kv` typedef and after its struct field, add:

```c
/* entity_find_by_class: fill outIndices/outSerials with the (index,serial) of every entity whose
 * CEntityIdentity::m_designerName == className, up to maxCount; returns the TOTAL match count. */
typedef int (*s2_entity_find_by_class_fn)(const char* className, int* outIndices, int* outSerials, int maxCount);
```
and in `struct S2EngineOps` after `s2_entity_spawn_kv_fn entity_spawn_kv;`:
```c
    s2_entity_find_by_class_fn entity_find_by_class;
```

- [ ] **Step 2: Rust op mirror** — in `core/src/v8host.rs`, mirror the `EntitySpawnKvFn` typedef + field. Add near the other `type *Fn` aliases:
```rust
type EntityFindByClassFn =
    unsafe extern "C" fn(*const std::os::raw::c_char, *mut i32, *mut i32, i32) -> i32;
```
and in `pub struct S2EngineOps` after `pub entity_spawn_kv: Option<EntitySpawnKvFn>,`:
```rust
    pub entity_find_by_class: Option<EntityFindByClassFn>,
```
Then add `entity_find_by_class: None,` to **both** in-isolate test op-struct literals (the two that currently end with `entity_spawn_kv: None,`).

- [ ] **Step 3: Write the failing test** — in the `core/src/v8host.rs` test module, add (mirror `entity_create_native_degrades_to_null_without_op`'s harness exactly — `init`/`set_engine_ops(None)`/`create_plugin_context`/`eval_in_context_string`/`shutdown`):
```rust
#[test]
fn find_by_class_degrades_to_empty_array_without_op() {
    let _ = init(dummy_logger());
    set_engine_ops(None);
    create_plugin_context("p");
    let out = eval_in_context_string("p", r#"
        const refs = __s2pkg_entity.Entity.findByClass("cs_gamerules");
        String(Array.isArray(refs) && refs.length === 0)
    "#);
    assert_eq!(out, "true");
    shutdown();
}
```
(No engine ops installed → the native degrades to `[]`.)

- [ ] **Step 4: Run it — expect FAIL** — `cd core && cargo test find_by_class_degrades` → FAIL (`__s2_entity_find_by_class` / `Entity.findByClass` undefined).

- [ ] **Step 5: Core native** — in `core/src/v8host.rs`, add the native (model it on `s2_entity_create`, but loop the shim's out-arrays and build an `EntityRef[]`):
```rust
/// Native `__s2_entity_find_by_class(className) -> EntityRef[]`. Over the `entity_find_by_class` op.
/// Returns serial-gated EntityRefs (each re-validated via entity_resolve_ptr, like s2_entity_create).
/// Degrades to an empty array with no op / null className. The out-buffer is bounded at 1024.
fn s2_entity_find_by_class(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let empty = v8::Array::new(scope, 0);
        rv.set(empty.into());
        let name = args.get(0).to_rust_string_lossy(scope);
        let cname = match std::ffi::CString::new(name) { Ok(c) => c, Err(_) => return };
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(func) = ops.and_then(|o| o.entity_find_by_class) else { return };
        const CAP: usize = 1024;
        let mut idxs = vec![0i32; CAP];
        let mut sers = vec![0i32; CAP];
        let total = unsafe { func(cname.as_ptr(), idxs.as_mut_ptr(), sers.as_mut_ptr(), CAP as i32) };
        let n = (total.max(0) as usize).min(CAP);
        let arr = v8::Array::new(scope, 0);
        let mut w: u32 = 0;
        for i in 0..n {
            let (index, serial) = (idxs[i], sers[i]);
            if !entity_resolve_ptr(index, serial).is_null() {
                let r = build_entity_ref(scope, index, serial);
                arr.set_index(scope, w, r);
                w += 1;
            }
        }
        rv.set(arr.into());
    }));
}
```
Register it with the other entity natives: `set_native(scope, global_obj, "__s2_entity_find_by_class", s2_entity_find_by_class);` (next to `__s2_entity_create`).

- [ ] **Step 6: Prelude `Entity.findByClass`** — in `core/src/v8host.rs`, the prelude `Entity` object (exported in `__s2pkg_entity = { EntityRef, createEntity, Entity }`) gains a method. Add to the `Entity` object literal:
```js
    findByClass: function (className) {
      return __s2_entity_find_by_class(String(className));
    },
```

- [ ] **Step 7: Run the test — expect PASS** — `cd core && cargo test find_by_class_degrades` → PASS.

- [ ] **Step 8: Shim impl** — in `shim/src/s2script_mm.cpp`, near `s2_ent_by_index`, add (uses the existing chunk-walk + `CUtlSymbolLarge::String()` + `CEntityHandle::GetSerialNumber()`, both inline-confirmed):
```cpp
// Engine-op: find every entity whose designer-name == className (exact). Iterates the entity-identity
// list (the s2_ent_by_index chunk walk), reads CEntityIdentity::m_designerName (a CUtlSymbolLarge),
// writes (index,serial) for the first maxCount matches, returns the TOTAL match count. Engine-generic.
static int s2_entity_find_by_class(const char* className, int* outIndices, int* outSerials, int maxCount) {
    if (!className || !outIndices || !outSerials) return 0;
    CGameEntitySystem* es = GetEntitySystem();
    if (!es) return 0;
    int found = 0;
    for (int idx = 0; idx < MAX_TOTAL_ENTITIES; ++idx) {
        int chunk = idx / MAX_ENTITIES_IN_LIST;
        int slot  = idx % MAX_ENTITIES_IN_LIST;
        CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[chunk];
        if (!chunk_base) continue;
        CEntityIdentity* id = &chunk_base[slot];
        if (id->m_flags & EF_IS_INVALID_EHANDLE) continue;
        if (!id->m_pInstance) continue;
        const char* dn = id->m_designerName.String();
        if (!dn || strcmp(dn, className) != 0) continue;
        if (found < maxCount) {
            CEntityHandle h = id->GetRefEHandle();
            outIndices[found] = h.GetEntryIndex();
            outSerials[found] = h.GetSerialNumber();
        }
        ++found;
    }
    return found;
}
```
Then wire it where the other `ops.` assignments live: `ops.entity_find_by_class = &s2_entity_find_by_class;`. (Confirm `CEntityIdentity` exposes `m_designerName` — it does in `public/entity2/entityclass.h:121`; if the include isn't already present in this TU, it is via the existing entity-system includes used by `s2_ent_by_index`.)

- [ ] **Step 9: Types** — in `packages/entity/index.d.ts`, add to the `Entity` namespace/interface (next to `createEntity`):
```ts
  /** Find every entity whose designer-name (class) exactly matches `className`. Returns serial-gated refs. */
  findByClass(className: string): EntityRef[];
```

- [ ] **Step 10: Commit**
```bash
git add shim/include/s2script_core.h core/src/v8host.rs shim/src/s2script_mm.cpp packages/entity/index.d.ts
git commit -F - <<'EOF'
feat(entity): entity_find_by_class op + Entity.findByClass

Iterate the entity-identity list by CEntityIdentity::m_designerName; return
serial-gated EntityRefs. Engine-generic (core/shim). Degrades to [] with no op.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 2: UserMessage op family + `@s2script/usermessages`

**Files:**
- Modify: `shim/include/s2script_core.h` (6 typedefs + fields after `entity_find_by_class`)
- Modify: `core/src/v8host.rs` (6 op mirrors + both test structs + 6 natives + `UserMessage` prelude + `__s2pkg_usermessages` + test)
- Modify: `shim/src/s2script_mm.cpp` (6 impls + `ops.` assignments)
- Create: `packages/usermessages/package.json`, `packages/usermessages/index.d.ts`

**Interfaces:**
- Consumes: nothing from Task 1 at runtime (independent op family; the ABI-append comes after Task 1's op).
- Produces: ops `user_message_create(name)->int`, `user_message_set_int(field,int64)->int`, `user_message_set_float(field,double)->int`, `user_message_set_string(field,str)->int`, `user_message_set_bool(field,int)->int`, `user_message_send(slots*,count)->int`; natives `__s2_user_message_create/set_int/set_float/set_string/set_bool/send`; `UserMessage` class in `__s2pkg_usermessages` with `setInt/setFloat/setString/setBool/set/send/sendAll`.

- [ ] **Step 1: C header** — in `shim/include/s2script_core.h`, after `entity_find_by_class`, add 6 typedefs + 6 struct fields **in this order**:
```c
typedef int (*s2_user_message_create_fn)(const char* name);
typedef int (*s2_user_message_set_int_fn)(const char* field, int64_t value);
typedef int (*s2_user_message_set_float_fn)(const char* field, double value);
typedef int (*s2_user_message_set_string_fn)(const char* field, const char* value);
typedef int (*s2_user_message_set_bool_fn)(const char* field, int value);
typedef int (*s2_user_message_send_fn)(const int* slots, int slotCount);
```
struct fields (same order):
```c
    s2_user_message_create_fn     user_message_create;
    s2_user_message_set_int_fn    user_message_set_int;
    s2_user_message_set_float_fn  user_message_set_float;
    s2_user_message_set_string_fn user_message_set_string;
    s2_user_message_set_bool_fn   user_message_set_bool;
    s2_user_message_send_fn       user_message_send;
```

- [ ] **Step 2: Rust op mirrors** — in `core/src/v8host.rs`, add the 6 `type *Fn` aliases + the 6 `pub ...: Option<...>` fields (after `entity_find_by_class`), and add all 6 as `None,` to **both** in-isolate test op-struct literals:
```rust
type UserMessageCreateFn    = unsafe extern "C" fn(*const std::os::raw::c_char) -> i32;
type UserMessageSetIntFn     = unsafe extern "C" fn(*const std::os::raw::c_char, i64) -> i32;
type UserMessageSetFloatFn   = unsafe extern "C" fn(*const std::os::raw::c_char, f64) -> i32;
type UserMessageSetStringFn  = unsafe extern "C" fn(*const std::os::raw::c_char, *const std::os::raw::c_char) -> i32;
type UserMessageSetBoolFn    = unsafe extern "C" fn(*const std::os::raw::c_char, i32) -> i32;
type UserMessageSendFn       = unsafe extern "C" fn(*const i32, i32) -> i32;
```

- [ ] **Step 3: Write the failing test** — (same harness as Task 1 Step 3):
```rust
#[test]
fn user_message_degrades_without_op() {
    let _ = init(dummy_logger());
    set_engine_ops(None);
    create_plugin_context("p");
    let out = eval_in_context_string("p", r#"
        const m = new __s2pkg_usermessages.UserMessage("CUserMessageFade");
        m.setInt("duration", 1024).set("flags", 18).set("amplitude", 1.5);
        // no ops installed -> create returns 0 -> send returns false, no throw
        String(m.send([0]) === false && m.sendAll() === false)
    "#);
    assert_eq!(out, "true");
    shutdown();
}
```

- [ ] **Step 4: Run it — expect FAIL** — `cd core && cargo test user_message_degrades` → FAIL.

- [ ] **Step 5: Core natives** — add 6 natives (each `catch_unwind`, default `false`/`0`, over its op). Shapes:
```rust
fn s2_user_message_create(scope, args, mut rv) { /* default 0 */
    rv.set_int32(0);
    let name = args.get(0).to_rust_string_lossy(scope);
    let cn = match std::ffi::CString::new(name) { Ok(c)=>c, Err(_)=>return };
    if let Some(f) = ENGINE_OPS.with(|o| o.get()).and_then(|o| o.user_message_create) {
        rv.set_int32(unsafe { f(cn.as_ptr()) });
    }
}
// set_int(field, value:i64), set_float(field, value:f64), set_string(field, value:str),
// set_bool(field, value:bool->i32): each CString the field, read the value, call the op, return its int.
// send(slotsArrayOrNull): if arg0 is null/undefined -> func(null, -1) (broadcast);
//   else collect the array's integers into Vec<i32> -> func(vec.as_ptr(), vec.len() as i32). Return bool == 1.
```
(Write each in full following the wrapper pattern of `s2_entity_spawn` / `s2_client_console_print`. For `send`, read `args.get(0)`; if it `is_null_or_undefined()` call with `(std::ptr::null(), -1)`, else if it `is_array()` iterate `v8::Local::<v8::Array>` indices into a `Vec<i32>` and pass `(vec.as_ptr(), vec.len() as i32)`. Set `rv.set_bool(result == 1)`.) Register all 6 with `set_native`.

- [ ] **Step 6: Prelude `UserMessage`** — add the builder + `globalThis.__s2pkg_usermessages = { UserMessage: UserMessage };`:
```js
  function UserMessage(name) { this._name = String(name); this._fields = []; }
  UserMessage.prototype.setInt    = function (f, v) { this._fields.push([0, String(f), v]); return this; };
  UserMessage.prototype.setFloat  = function (f, v) { this._fields.push([1, String(f), v]); return this; };
  UserMessage.prototype.setString = function (f, v) { this._fields.push([2, String(f), String(v)]); return this; };
  UserMessage.prototype.setBool   = function (f, v) { this._fields.push([3, String(f), v ? 1 : 0]); return this; };
  UserMessage.prototype.set = function (f, v) {
    if (typeof v === "boolean") return this.setBool(f, v);
    if (typeof v === "string")  return this.setString(f, v);
    if (typeof v === "number")  return Number.isInteger(v) ? this.setInt(f, v) : this.setFloat(f, v);
    return this;
  };
  UserMessage.prototype._flush = function (slotsOrNull) {
    if (__s2_user_message_create(this._name) !== 1) return false;
    for (var i = 0; i < this._fields.length; i++) {
      var fld = this._fields[i];
      if (fld[0] === 0)      __s2_user_message_set_int(fld[1], fld[2]);
      else if (fld[0] === 1) __s2_user_message_set_float(fld[1], fld[2]);
      else if (fld[0] === 2) __s2_user_message_set_string(fld[1], fld[2]);
      else                   __s2_user_message_set_bool(fld[1], fld[2]);
    }
    return __s2_user_message_send(slotsOrNull) === true;
  };
  UserMessage.prototype.send    = function (slots) { return this._flush(Array.isArray(slots) ? slots : [slots]); };
  UserMessage.prototype.sendAll = function () { return this._flush(null); };
```

- [ ] **Step 7: Run the test — expect PASS** — `cd core && cargo test user_message_degrades` → PASS.

- [ ] **Step 8: Shim impls** — in `shim/src/s2script_mm.cpp`, near `s2_client_print`, add the target state + 6 impls (mirrors the SayText2 reflection at :715–728 and the `s_currentEvent` single-target model). **Leak note:** mirror the existing 6.1c leak-TODO — do NOT `Deallocate` `pData` (ownership after `PostEventAbstract` is unconfirmed; a double-free is worse than a bounded per-send leak).
```cpp
// --- General user messages (generalize the SayText2 reflection path) ---
static INetworkMessageInternal* s_umInfo = nullptr;   // the message factory
static CNetMessage*             s_umData = nullptr;    // the allocated CNetMessage
static google::protobuf::Message* s_umMsg = nullptr;   // its protobuf Message view

static int s2_user_message_create(const char* name) {
    s_umInfo = nullptr; s_umData = nullptr; s_umMsg = nullptr;   // drop any prior unsent (bounded leak-TODO)
    if (!name || !s_pNetworkMessages) return 0;
    INetworkMessageInternal* info = s_pNetworkMessages->FindNetworkMessagePartial(name);
    if (!info) return 0;
    CNetMessage* data = info->AllocateMessage();
    if (!data) return 0;
    google::protobuf::Message* m = reinterpret_cast<google::protobuf::Message*>(data->AsProto());
    if (!m) return 0;
    s_umInfo = info; s_umData = data; s_umMsg = m;
    return 1;
}
static int s2_user_message_set_int(const char* field, int64_t value) {
    if (!s_umMsg || !field) return 0;
    const auto* d = s_umMsg->GetDescriptor(); const auto* r = s_umMsg->GetReflection();
    if (!d || !r) return 0;
    const auto* f = d->FindFieldByName(field); if (!f) return 0;
    using FD = google::protobuf::FieldDescriptor;
    switch (f->cpp_type()) {
        case FD::CPPTYPE_INT32:  r->SetInt32(s_umMsg, f, (int32_t)value);  break;
        case FD::CPPTYPE_UINT32: r->SetUInt32(s_umMsg, f, (uint32_t)value); break;
        case FD::CPPTYPE_INT64:  r->SetInt64(s_umMsg, f, value);           break;
        case FD::CPPTYPE_UINT64: r->SetUInt64(s_umMsg, f, (uint64_t)value); break;
        case FD::CPPTYPE_ENUM:   r->SetEnumValue(s_umMsg, f, (int)value);  break;
        case FD::CPPTYPE_BOOL:   r->SetBool(s_umMsg, f, value != 0);       break;
        case FD::CPPTYPE_FLOAT:  r->SetFloat(s_umMsg, f, (float)value);    break;
        case FD::CPPTYPE_DOUBLE: r->SetDouble(s_umMsg, f, (double)value);  break;
        default: return 0;
    }
    return 1;
}
static int s2_user_message_set_float(const char* field, double value) {
    if (!s_umMsg || !field) return 0;
    const auto* d = s_umMsg->GetDescriptor(); const auto* r = s_umMsg->GetReflection();
    if (!d || !r) return 0;
    const auto* f = d->FindFieldByName(field); if (!f) return 0;
    using FD = google::protobuf::FieldDescriptor;
    if (f->cpp_type() == FD::CPPTYPE_FLOAT)  { r->SetFloat(s_umMsg, f, (float)value);  return 1; }
    if (f->cpp_type() == FD::CPPTYPE_DOUBLE) { r->SetDouble(s_umMsg, f, value);        return 1; }
    return 0;
}
static int s2_user_message_set_string(const char* field, const char* value) {
    if (!s_umMsg || !field) return 0;
    const auto* d = s_umMsg->GetDescriptor(); const auto* r = s_umMsg->GetReflection();
    if (!d || !r) return 0;
    const auto* f = d->FindFieldByName(field); if (!f) return 0;
    if (f->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_STRING) return 0;
    r->SetString(s_umMsg, f, value ? value : "");
    return 1;
}
static int s2_user_message_set_bool(const char* field, int value) {
    if (!s_umMsg || !field) return 0;
    const auto* d = s_umMsg->GetDescriptor(); const auto* r = s_umMsg->GetReflection();
    if (!d || !r) return 0;
    const auto* f = d->FindFieldByName(field); if (!f) return 0;
    if (f->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_BOOL) return 0;
    r->SetBool(s_umMsg, f, value != 0);
    return 1;
}
static int s2_user_message_send(const int* slots, int slotCount) {
    if (!s_umMsg || !s_umInfo || !s_umData || !s_pGameEventSystem) {
        s_umInfo = nullptr; s_umData = nullptr; s_umMsg = nullptr; return 0;
    }
    uint64 clients = 0;
    if (slotCount < 0) {                                   // broadcast to all live non-bot slots
        for (int s = 0; s < 64; ++s)
            if (s_pEngine && s_pEngine->GetPlayerNetInfo(CPlayerSlot(s))) clients |= (1ull << (uint64)s);
    } else if (slots) {
        for (int i = 0; i < slotCount; ++i) {
            int s = slots[i];
            if (s < 0 || s >= 64) continue;
            if (s_pEngine && !s_pEngine->GetPlayerNetInfo(CPlayerSlot(s))) continue;   // skip bots (would crash)
            clients |= (1ull << (uint64)s);
        }
    }
    int ok = 0;
    if (clients != 0) {
        s_pGameEventSystem->PostEventAbstract(-1, false, 64, &clients, s_umInfo, s_umData, 0, BUF_RELIABLE);
        ok = 1;
    }
    s_umInfo = nullptr; s_umData = nullptr; s_umMsg = nullptr;   // clear the single target after send
    return ok;
}
```
Wire all 6: `ops.user_message_create = &s2_user_message_create;` … `ops.user_message_send = &s2_user_message_send;`.

- [ ] **Step 9: Package** — create `packages/usermessages/package.json` (mirror `packages/entity/package.json`: name `@s2script/usermessages`, `types: "index.d.ts"`, private) and `packages/usermessages/index.d.ts`:
```ts
/** A general protobuf user-message builder. Build then send in one synchronous burst. */
export class UserMessage {
  constructor(name: string);
  setInt(field: string, value: number): this;
  setFloat(field: string, value: number): this;
  setString(field: string, value: string): this;
  setBool(field: string, value: boolean): this;
  /** Infer the setter from the JS value type. */
  set(field: string, value: number | string | boolean): this;
  /** Send to one slot or a list of slots. Returns true if delivered to >=1 real client. */
  send(slots: number | number[]): boolean;
  /** Broadcast to all connected non-bot clients. */
  sendAll(): boolean;
}
```

- [ ] **Step 10: Commit**
```bash
git add shim/include/s2script_core.h core/src/v8host.rs shim/src/s2script_mm.cpp packages/usermessages
git commit -F - <<'EOF'
feat(usermsg): general UserMessage sender (@s2script/usermessages)

Generalize the SayText2 protobuf-reflection path: create(name) -> set*(by
reflection cpp_type) -> send(recipients). 6 ops appended after
entity_find_by_class. Bot-skip guarded; scalar fields (covers Fade/Shake/
Hud/Text). Engine-generic; degrades to no-op with no op.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 3: `GameRules` accessor (CS2)

**Files:**
- Modify: `games/cs2/js/pawn.js` (the `GameRules` object + export)
- Modify: `packages/cs2/index.d.ts` (`GameRules` types)

**Interfaces:**
- Consumes: `Entity.findByClass` (Task 1) via `globalThis.__s2pkg_entity`; `EntityRef.readBoolVia/readInt32Via/readFloat32Via`; `__s2_schema_offset`.
- Produces: `GameRules.get(): GameRulesView | null` exported in `__s2pkg_cs2`.

- [ ] **Step 1: Add the accessor** — in `games/cs2/js/pawn.js` (inside the IIFE, before the `__s2pkg_cs2` export at :598), add:
```js
  // GameRules — read CCSGameRules via the cs_gamerules proxy's m_pGameRules pointer.
  // Serial-gated at the proxy root (readVia); offsets live-resolved per access (self-healing across map
  // changes — the proxy dies and re-resolves). All getters read null if the proxy is gone.
  function GameRulesView(proxyRef) { this.ref = proxyRef; }
  function grPath() { var o = __s2_schema_offset("CCSGameRulesProxy", "m_pGameRules"); return o < 0 ? null : [o]; }
  function grBool(field)  { return { get: function () { var p = grPath(); if (!p) return null; var o = __s2_schema_offset("CCSGameRules", field); return o < 0 ? null : this.ref.readBoolVia(p, o); } }; }
  function grInt(field)   { return { get: function () { var p = grPath(); if (!p) return null; var o = __s2_schema_offset("CCSGameRules", field); return o < 0 ? null : this.ref.readInt32Via(p, o); } }; }
  function grFloat(field) { return { get: function () { var p = grPath(); if (!p) return null; var o = __s2_schema_offset("CCSGameRules", field); return o < 0 ? null : this.ref.readFloat32Via(p, o); } }; }
  Object.defineProperties(GameRulesView.prototype, {
    warmupPeriod:          grBool("m_bWarmupPeriod"),
    freezePeriod:          grBool("m_bFreezePeriod"),
    roundTime:             grInt("m_iRoundTime"),
    freezeTime:            grInt("m_iFreezeTime"),
    totalRoundsPlayed:     grInt("m_totalRoundsPlayed"),
    gamePhase:             grInt("m_gamePhase"),
    bombPlanted:           grBool("m_bBombPlanted"),
    roundsPlayedThisPhase: grInt("m_nRoundsPlayedThisPhase"),
    gameRestart:           grBool("m_bGameRestart"),
    gameStartTime:         grFloat("m_flGameStartTime"),
    matchWaitingForResume: grBool("m_bMatchWaitingForResume"),
    hasMatchStarted:       grBool("m_bHasMatchStarted")
  });
  var GameRules = {
    get: function () {
      var ent = globalThis.__s2pkg_entity;
      var refs = ent && ent.Entity ? ent.Entity.findByClass("cs_gamerules") : null;
      if (!refs || refs.length === 0) return null;
      return new GameRulesView(refs[0]);
    }
  };
```

- [ ] **Step 2: Export** — add `GameRules: GameRules` to the `Object.assign({}, ..., { Pawn: Pawn, Player: Player, ... })` at pawn.js:598.

- [ ] **Step 3: Types** — in `packages/cs2/index.d.ts`, add:
```ts
export interface GameRulesView {
  readonly warmupPeriod: boolean | null;
  readonly freezePeriod: boolean | null;
  readonly roundTime: number | null;
  readonly freezeTime: number | null;
  readonly totalRoundsPlayed: number | null;
  readonly gamePhase: number | null;
  readonly bombPlanted: boolean | null;
  readonly roundsPlayedThisPhase: number | null;
  readonly gameRestart: boolean | null;
  readonly gameStartTime: number | null;
  readonly matchWaitingForResume: boolean | null;
  readonly hasMatchStarted: boolean | null;
}
export const GameRules: { get(): GameRulesView | null };
```

- [ ] **Step 4: Verify game-layer JS is valid** — `node -e "require('./games/cs2/js/pawn.js')"` won't run standalone (it needs the prelude globals), so instead run the freshness/boundary gates: `bash scripts/check-core-boundary.sh` and `bash scripts/test-boundary-nameleak.sh` → both green (no CS2 name leaked into core; `CCSGameRules`/`cs_gamerules` appear only in pawn.js).

- [ ] **Step 5: Commit**
```bash
git add games/cs2/js/pawn.js packages/cs2/index.d.ts
git commit -F - <<'EOF'
feat(cs2): GameRules accessor (CCSGameRules via the cs_gamerules proxy)

GameRules.get() finds the proxy via Entity.findByClass, reads CCSGameRules
fields through m_pGameRules with the serial-gated readVia nav (offsets
live-resolved, self-healing). CS2 field names stay in the game layer.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 4: CS2 UserMessage sugar (Fade/Shake/HintText) + `sm_blind`

**Files:**
- Modify: `games/cs2/js/pawn.js` (`Fade`/`Shake`/`HintText` + export)
- Modify: `packages/cs2/index.d.ts` (sugar types)
- Modify: `plugins/funcommands/src/plugin.ts` (`sm_blind`)

**Interfaces:**
- Consumes: `UserMessage` (Task 2) via `globalThis.__s2pkg_usermessages`.
- Produces: `Fade.to/blind`, `Shake.to`, `HintText.to` in `__s2pkg_cs2`; `sm_blind` command.

- [ ] **Step 1: Sugar** — in `games/cs2/js/pawn.js` before the `__s2pkg_cs2` export, add:
```js
  // CS2 user-message sugar over the generic @s2script/usermessages builder.
  var FFADE_IN = 1, FFADE_OUT = 2, FFADE_MODULATE = 4, FFADE_STAYOUT = 8, FFADE_PURGE = 16;
  function _um(name) { return new (globalThis.__s2pkg_usermessages.UserMessage)(name); }
  var Fade = {
    // opts: { duration, holdTime?, color?, flags? }. duration/holdTime are engine fade units
    // (tuned at the human visual test); color is a packed RGBA fixed32 (default opaque black).
    to: function (slot, opts) {
      opts = opts || {};
      return _um("CUserMessageFade")
        .setInt("duration",  opts.duration  != null ? opts.duration  : 1024)
        .setInt("hold_time", opts.holdTime  != null ? opts.holdTime  : 0)
        .setInt("flags",     opts.flags     != null ? opts.flags     : (FFADE_OUT | FFADE_PURGE))
        .setInt("color",     opts.color     != null ? opts.color     : 0xFF000000)
        .send(slot);
    },
    blind: function (slot, duration) {
      var d = duration != null ? duration : 2000;
      return Fade.to(slot, { duration: d, holdTime: d, flags: FFADE_OUT | FFADE_PURGE, color: 0xFF000000 });
    }
  };
  var Shake = {
    // opts: { amplitude, frequency, duration }. command 0 = start.
    to: function (slot, opts) {
      opts = opts || {};
      return _um("CUserMessageShake")
        .setInt("command",     opts.command   != null ? opts.command   : 0)
        .setFloat("amplitude", opts.amplitude != null ? opts.amplitude : 10.0)
        .setFloat("frequency", opts.frequency != null ? opts.frequency : 1.5)
        .setFloat("duration",  opts.duration  != null ? opts.duration  : 1.0)
        .send(slot);
    }
  };
  // HintText: the plan's shim spike resolves the exact scalar CS2 hint message. If a clean
  // CUserMessageHudMsg/TextMsg hint resolves, wire it here; otherwise leave HintText.to as a
  // documented no-op-returning stub (Fade + Shake are the load-bearing sugar) and note it in the
  // demo. Implement against whatever FindNetworkMessagePartial("...") resolves during Task 2's shim work.
  var HintText = {
    to: function (slot, text) {
      // Best-effort: try TextMsg-family; returns false if the message/field don't resolve.
      var m = _um("CUserMessageTextMsg");
      // field wiring confirmed during the live gate; if TextMsg is unavailable this is a no-op send.
      return m.setInt("dest", 4 /* HUD_PRINTCENTER-ish; tuned live */).setString("param", String(text)).send(slot);
    }
  };
```

- [ ] **Step 2: Export** — add `Fade: Fade, Shake: Shake, HintText: HintText` to the `__s2pkg_cs2` `Object.assign` at pawn.js:598.

- [ ] **Step 3: Types** — in `packages/cs2/index.d.ts`:
```ts
export const Fade: {
  to(slot: number, opts: { duration?: number; holdTime?: number; color?: number; flags?: number }): boolean;
  blind(slot: number, duration?: number): boolean;
};
export const Shake: {
  to(slot: number, opts: { command?: number; amplitude?: number; frequency?: number; duration?: number }): boolean;
};
export const HintText: { to(slot: number, text: string): boolean };
```

- [ ] **Step 4: `sm_blind`** — in `plugins/funcommands/src/plugin.ts`, add `Fade` to the `@s2script/cs2` import and register (inside `onLoad`, next to `sm_freeze`):
```ts
  Commands.registerAdmin("sm_blind", ADMFLAG.SLAY, (ctx) => {
    const secs = ctx.args.length > 1 ? parseFloat(ctx.args[1]) : 2;
    const durMs = (isFinite(secs) && secs > 0 ? secs : 2) * 1000;
    forEachPawn(ctx, "sm_blind <target> [seconds]", "Blinded", (p, _pw) => {
      Fade.blind(p.slot, durMs);
    });
  });
```
(Confirm `forEachPawn`'s callback gives the `Player` — it does: `(p: Player, pw: Pawn)`. `p.slot` is the 0-based slot `Fade.blind` needs.)

- [ ] **Step 5: Typecheck** — `bash scripts/check-plugins-typecheck.sh` → funcommands (and all plugins) pass full-strict (`Fade` resolves from `@s2script/cs2`).

- [ ] **Step 6: Commit**
```bash
git add games/cs2/js/pawn.js packages/cs2/index.d.ts plugins/funcommands/src/plugin.ts
git commit -F - <<'EOF'
feat(cs2): Fade/Shake/HintText usermessage sugar + sm_blind

Typed CS2 helpers over @s2script/usermessages; sm_blind (ADMFLAG.SLAY) into
funcommands closes that deferral. Fade/Shake fields verified all-scalar.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 5: Demo plugin + full typecheck

**Files:**
- Create: `plugins/gamerules-usermsg-demo/package.json`, `tsconfig.json`, `src/plugin.ts`

**Interfaces:**
- Consumes: everything above (`Entity.findByClass`, `GameRules`, `UserMessage`, `Fade`).

- [ ] **Step 1: Scaffold** — copy the shape of an existing minimal plugin (e.g. `plugins/items-demo/`): `package.json` (name `gamerules-usermsg-demo`, `s2script.apiVersion`, deps `@s2script/commands`, `@s2script/entity`, `@s2script/usermessages`, `@s2script/cs2`), `tsconfig.json` (extends `../../tsconfig.base.json`).

- [ ] **Step 2: `src/plugin.ts`**
```ts
import { Commands } from "@s2script/commands";
import { Entity } from "@s2script/entity";
import { GameRules, Fade, Player } from "@s2script/cs2";

export function onLoad(): void {
  Commands.register("sm_gamerules", (ctx) => {
    const gr = GameRules.get();
    const proxies = Entity.findByClass("cs_gamerules").length;
    if (!gr) { ctx.reply(`[gr] no cs_gamerules proxy (findByClass=${proxies})`); return; }
    ctx.reply(`[gr] warmup=${gr.warmupPeriod} freeze=${gr.freezePeriod} roundTime=${gr.roundTime} ` +
              `rounds=${gr.totalRoundsPlayed} phase=${gr.gamePhase} proxies=${proxies}`);
  });

  Commands.register("sm_umsg", (ctx) => {
    const slot = ctx.args.length > 1 ? parseInt(ctx.args[1], 10) : (ctx.callerSlot >= 0 ? ctx.callerSlot : 0);
    const ok = Fade.blind(slot, 1500);
    ctx.reply(`[umsg] Fade.blind(slot=${slot}) -> ${ok}`);
  });

  console.log("[gamerules-usermsg-demo] onLoad — sm_gamerules/sm_umsg registered");
}
```

- [ ] **Step 3: Build** — `npx s2script build plugins/gamerules-usermsg-demo` → produces `plugins/gamerules-usermsg-demo/dist/*.s2sp` (the typecheck gate runs; it must pass full-strict).

- [ ] **Step 4: Typecheck all** — `bash scripts/check-plugins-typecheck.sh` → all plugins green.

- [ ] **Step 5: Commit**
```bash
git add plugins/gamerules-usermsg-demo
git commit -F - <<'EOF'
feat(demo): gamerules-usermsg-demo (findByClass + GameRules + Fade)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Build, live gate, and merge (after the Workflow)

Not a Workflow task — the human-in-the-loop integration step.

- [ ] **Core tests** — `cd core && cargo test` → all green (serial).
- [ ] **Sniper rebuild** — `docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh` (compiles core `.so` + shim `.so`; the ABI-append means both need it).
- [ ] **Re-deploy plugins** (the sniper build wipes `dist/addons/s2script`): recreate `dist/addons/s2script/configs` as gkh, then `cp plugins/*/dist/*.s2sp dist/addons/s2script/plugins/` (active only, not `disabled/`).
- [ ] **Restart** — `cd docker && docker compose restart cs2` (NOT `--force-recreate`; re-run `/patch-gameinfo.sh` only if `gameinfo.gi` was reset).
- [ ] **Live gate** (de_inferno / de_dust2, `bot_quota 2`, `scripts/rcon.py`):
  - `=== GAMEDATA VALIDATION: N ok, 0 FAILED ===` (N unchanged unless a sig was added — this slice adds none), `RestartCount=0`.
  - `sm_gamerules` → plausible live values (`warmup`/`freeze` boolean, `roundTime` the config, `rounds`≥0, `phase` an int) and `proxies=1`. **Bots-provable.**
  - `Entity.findByClass("cs_gamerules").length === 1`; a nonsense class → 0.
  - `sm_umsg <slot>` / `sm_blind <bot>` → executes without crash; the send path constructs + reflects (bots-provable). Server keeps ticking.
- [ ] **Document** the human-client deferral: the *visual* blackout/shake/hint on a real client (same ceiling as SayText2's visible chat line).

## Self-Review notes (author)

- Spec coverage: findByClass (Task 1), GameRules (Task 3), generic UserMessage (Task 2), Fade/Shake/HintText + sm_blind (Task 4), demo (Task 5), live gate — all covered.
- HintText is explicitly best-effort (spec-flagged); Fade + Shake are the load-bearing sugar. Not a placeholder — a documented fallback with the resolution step named (Task 2 shim spike / live gate).
- Fade `duration`/`color` units are engine-specific and tuned at the human visual test; the *mechanism* (construct/reflect/send) is what the bot gate proves. Called out in Task 4 + the live-gate deferral.
- Type consistency: `GameRulesView` (Task 3 type) ↔ `GameRules.get()` return; `UserMessage` methods (Task 2 `.d.ts`) ↔ prelude + sugar usage.
