# Slice 5E.3 — reload state-handoff (design)

**Goal:** Let a hot-reloaded plugin carry runtime state from its old instance to its new one. On a
file-watch **Reload** of the same plugin id, the old instance's `onUnload()` may return a `State`
object; the host holds it across the teardown→load gap and passes it to the new instance's
`onLoad(prev)`. A file edit no longer wipes in-memory state.

**Status:** design approved (charter shape `onUnload(): State → onLoad(prev)`; State via the existing
inter-plugin structured-copy incl. live EntityRef revival; handoff on any same-id reload; no
`@s2script/lifecycle` types package — the hooks are documented conventions).

**Branch base:** `main` (… + 5E.2 merged).
**Cadence:** subagent-driven, merge-to-main-locally, Docker CS2 live gate.

Second of the three lifecycle sub-slices ("do em all"): config (5E.2 done) → **reload-handoff (this)** →
permissions.

---

## 1. Shape — the author contract

```typescript
// old instance, before teardown
export function onUnload(): State { return { count, trackedPawn: pawn.ref }; }
// new instance (same id, same or newer version)
export function onLoad(prev?: State): void { if (prev) { count = prev.count; } }
```

- `onUnload(): State | void` — returning a value opts into handoff; returning nothing (or the plugin
  having no `onUnload`) means no state is carried. `State` is **author-defined** (any
  structured-copyable shape).
- `onLoad(prev?: State)` — `prev` is the revived state on a reload, or `undefined` on a first load.
- `onLoad`/`onUnload` are **exported-function conventions** the host calls by name; TypeScript does not
  bind them to a host interface, so there is **no new `@s2script/lifecycle` types package** — the author
  types their own `State` and the README + demo document the shape.

## 2. Mechanism — capture, hold, revive (entirely in-core)

The reload gap disposes the old context before the new one loads, so `State` must leave the old context
as a **host-held representation** and be rebuilt in the new context. This reuses the existing
inter-plugin marshalling verbatim:

- **Serialize (`iface_to_json`, already exists):** `JSON.stringify(value, __s2_entref_replacer)` in the
  **old** context → an `Option<String>` (the EntityRef replacer tags any `EntityRef` as
  `{index, serial}`; a non-serializable value — function, cycle — → `None`). The resulting **string
  trivially survives context disposal** — it is a plain Rust `String`.
- **Revive (`iface_from_json`, already exists):** `JSON.parse(blob, __s2_entref_reviver)` in the **new**
  context → a fresh Local; the reviver rebinds tagged EntityRefs to **this** context's
  `__s2_ent_ref_*` natives, validating against the **shared** entity system (a serial-gated live ref;
  reads `null` if the entity was destroyed during the gap).
- **Hold:** a host-side thread-local `PENDING_HANDOFF: RefCell<HashMap<String, String>>` (`id → blob`).

### Data flow (loader-orchestrated)

| Loader action | Sequence | Handoff |
|---|---|---|
| **Load** (new file) | `load_plugin_js` | no pending → `onLoad(undefined)` |
| **Reload** (mtime change, same id) | `unload_plugin(id)` **captures** → `load_plugin_js(id)` **consumes** | `onLoad(prev)` |
| **Vanished** (file removed) | `unload_plugin(id)` captures → loader **clears** pending | discarded |
| **shutdown** | `unload_all` per plugin | `PENDING_HANDOFF` cleared (map reset) |

- `unload_plugin(id)` always captures `onUnload()`'s return into `PENDING_HANDOFF[id]` (it cannot know
  reload-vs-final; the caller decides consumption).
- `load_plugin_js(id, …)`, right before calling `onLoad`, checks `PENDING_HANDOFF[id]`: present → revive
  + pass as `onLoad`'s single arg, then **remove** the entry (consume-once); absent → `onLoad()` (no
  arg → JS `prev === undefined`).
- The loader's **Vanished** branch calls a new `clear_pending_handoff(id)` after `unload_plugin` so a
  final removal never leaves a stale blob. `shutdown` resets the whole map.

## 3. State contents

Whatever the inter-plugin wire already round-trips: any JSON value — primitives, `string`, arrays, nested
plain objects — and **`EntityRef`** (revives live + serial-gated). **NOT `bigint`:** `iface_to_json` is
`JSON.stringify`, which throws on a `BigInt`, so a `bigint` anywhere in `State` makes `iface_to_json`
return `None` → the WHOLE handoff is silently discarded (degrade-safe, but total). Carry a 64-bit value
as a decimal `string` (the framework convention — the reason 5B.4's `uint64`/`int64` accessors return
strings). A `Player`/`Pawn` is `EntityRef`-backed
but is a *prototype-wrapped* object — an author carrying one should store the `.ref` (an `EntityRef`) or a
plain `{slot}` and re-wrap in `onLoad`; carrying the wrapper object directly serializes only its own-enumerable
fields (documented). Non-serializable values (functions, cycles) → the whole capture is `None` →
`onLoad(undefined)` + a WARN.

## 4. Degrade-never-crash

- `onUnload()` throws → no capture (the existing onUnload-error WARN), new `onLoad(undefined)`.
- `onUnload()` returns a non-serializable value → `iface_to_json` → `None` → no pending → `onLoad(undefined)` + a named WARN.
- Revival fails (should not — the host produced the string; defends against corruption) → `onLoad(undefined)` (the reviver's TryCatch absorbs it silently; benign).
- `onLoad(prev)` throws → the existing load-time onLoad-error WARN; the instance is loaded, handoff consumed, no crash.
- An `EntityRef` in `State` whose entity was destroyed during the gap → a serial-gated `null` read (never garbage, never a crash).
- No path throws into the engine; a broken handoff degrades to `onLoad(undefined)`.

## 5. Components & data flow (boundary: engine-generic)

| Concern | Lives in | Why |
|---|---|---|
| Capture `onUnload` return → serialize → `PENDING_HANDOFF`; revive → `onLoad(prev)`; `clear_pending_handoff`; map reset on shutdown | core (`v8host.rs`) | engine-generic; every plugin has lifecycle |
| Reload-consume vs Vanished-clear orchestration | core (`loader.rs`) | the loader owns the reload state machine |
| `iface_to_json` / `iface_from_json` + the per-context `__s2_entref_replacer`/`reviver` | core (already exists — Slice 4.5 + 5A fast-follow) | reused verbatim |

**No shim change, no new engine-op, no native** — the whole handoff is a JS value serialized in-isolate,
held in a Rust map, and revived in-isolate. **One sniper rebuild** (core `.so` only; the shim is
untouched). Both boundary gates stay green (no game symbol enters core).

## 6. Testing & live gate

- **In-isolate (core):** round-trip through unload→hold→load — primitives, nested objects,
  and an `EntityRef` (revives to a live serial-gated ref); first-load → `onLoad(undefined)`; degrade
  (onUnload throws → undefined; non-serializable return → undefined + WARN; onLoad(prev) throws → WARN,
  no crash); **Vanished clears pending** (a removal-then-fresh-load sees `undefined`, not the stale blob).
- **Live gate (de_inferno):** a demo keeps a **counter** (increment each `onLoad`, seed from `prev`) and
  a **tracked pawn `EntityRef`** in `State`. Touch the `.s2sp` to force a Reload → the counter increments
  across reloads (proves handoff) and the tracked pawn survives as a live ref; the first load logs
  `prev=undefined`; kill the tracked entity between reloads → the revived ref reads `null` (degrade, no
  crash). Delete the `.s2sp` then re-add → `onLoad(undefined)` (Vanished cleared the pending blob), server
  ticking throughout.

## 7. Rough task decomposition (~4)

1. **Core capture + hold:** `PENDING_HANDOFF` thread-local; `unload_plugin` captures `onUnload()`'s
   return via `iface_to_json` into the map; `clear_pending_handoff(id)` + shutdown reset. In-isolate
   tests (capture a serializable return; non-serializable → None; onUnload-throws → no entry).
2. **Core revive + inject:** `load_plugin_js` consumes `PENDING_HANDOFF[id]` via `iface_from_json` and
   passes it as `onLoad`'s arg (else `onLoad()`); consume-once. In-isolate tests (round-trip primitives/
   nested; EntityRef revives live; first-load undefined; onLoad(prev) throws → WARN degrade).
3. **Loader orchestration:** the Reload path already does `unload_plugin`→`load_plugin_js` (consume falls
   out); the **Vanished** path calls `clear_pending_handoff(id)`. In-isolate/loader tests (Reload hands
   off; Vanished discards).
4. **Demo + one sniper build (core) + live gate + docs.**

## 8. Explicitly out of scope (do not build ahead)

Crash-survival (only an intentional reload hands off — a hard crash loses state); disk persistence; a
host KV store (`state.set`/`get`); version-shape migration helpers (the author owns migration across
versions, like config); handoff across **different** ids (a rename is a new plugin — no handoff); carrying
non-`EntityRef` live handles (raw Globals never cross); a typed `@s2script/lifecycle` hook contract
(the hooks stay documented conventions). The **permissions** sub-slice is next, not here. Note later
needs as TODOs and stop.
