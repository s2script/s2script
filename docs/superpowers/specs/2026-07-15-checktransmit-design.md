# CheckTransmit — per-client entity visibility filtering (`@s2script/transmit`)

**Slice:** `feat/checktransmit` · **Date:** 2026-07-15 · **Status:** design approved for planning

## 1. Problem & consumer

The TTT port is blocked on per-client entity visibility: role-icon entities (one `point_worldtext`
hat per player) must be visible to some viewers and hidden from others — Detectives' icons visible
to all, Traitors' icons visible **only to other traitors** (the pairwise mask *is* the buddy
system). Without it every role icon transmits to every client and the gamemode is unplayable.

Reference consumer: `TTT/CS2/GameHandlers/RoleIconsHandler.cs:87-90, 195-211` — keeps a
`ulong[64]` per-viewer bitmask (bit *i* = "viewer may see player *i*'s icons"), and in a
`CheckTransmit` listener removes every unauthorized icon from each viewer's transmit list, with a
fast path `if (visible == ulong.MaxValue) return;`.

**Scope (deliberately narrow):** entity-visibility filtering only. No player transmit-hiding
conveniences, no PVS-force-add (`m_pTransmitAlways`), no per-snapshot JS callback. TTT hides icons
only; the primitive ships scoped to that, extensible later.

## 2. The engine fact (RE approach)

### 2.1 Hook point — verified, not borrowed

`CheckTransmit` is a virtual on **`ISource2GameEntities`** (interface string
`Source2GameEntities001`, `INTERFACEVERSION_SERVERGAMEENTS`) — **not** `ISource2GameClients` as the
gap analysis guessed. Verified in our vendored hl2sdk (`third_party/hl2sdk/public/eiface.h:485-501`):

```cpp
#define INTERFACEVERSION_SERVERGAMEENTS   "Source2GameEntities001"
abstract_class ISource2GameEntities : public IAppSystem {
  virtual ~ISource2GameEntities() = 0;
  virtual void CheckTransmit( CCheckTransmitInfo **pInfoInfoList, int nInfoCount,
                              CBitVec<16384> &unionTransmitEdicts, CBitVec<16384> &,
                              const Entity2Networkable_t **pNetworkables,
                              const uint16 *pEntityIndicies, int nEntityIndices ) = 0;
  ...
};
```

The engine calls it once per snapshot build with one `CCheckTransmitInfo*` per client being
snapshotted; the game fills each info's transmit bitvec; a **post** SourceHook mutates the bits
after the game has decided (both hint frameworks hook post).

**Acquisition + hook mechanism** reuses the proven `Source2GameClients001` pattern verbatim
(`shim/src/s2script_mm.cpp` Load(), ~2396): interface string from `gamedata` (`interfaces` block,
new key `Source2GameEntities`) with the hl2sdk constant as fallback, acquired via
`ismm->GetServerFactory(false)`, hooked with a **declared** SourceHook
(`SH_DECL_HOOK7_void(ISource2GameEntities, CheckTransmit, ...)`) against the vendored header —
exactly how the seven existing client hooks work. The vtable index therefore comes from our pinned,
patch-capable hl2sdk (part of the update treadmill), not from another framework's gamedata.

**Hints consulted (per re-strategy Rule 3 — hints, never numbers):**
- CounterStrikeSharp (`src/core/managers/entity_manager.cpp:31-66`): manual vphook on
  `*(void**)gameEntities` with offset from *their* gamedata, post, 7 args matching our header.
- SwiftlyS2 (`src/core/entrypoint.cpp:74`, `src/engine/vgui/vgui.cpp:123-149`): **declared**
  `SH_DECL_HOOK7_void(ISource2GameEntities, CheckTransmit, ...)` against hl2sdk, post — the exact
  mechanism we adopt, corroborating that the vendored header's vtable layout is live-correct.

### 2.2 `CCheckTransmitInfo` layout — data, validated at first fire

hl2sdk's `CCheckTransmitInfo` (`public/iservernetworkable.h:37`) is explicitly incomplete
("TODO: This is incomplete") — it only guarantees `m_pTransmitEntity` (`CBitVec<16384>*`) at
offset 0. Two facts we additionally need are **not** in the SDK:

| Fact | Offset | CSSharp says | Swiftly says |
|---|---|---|---|
| which client this info is for | **+576** | `int32 m_nPlayerSlot` | `CEntityIndex m_nClientEntityIndex` |
| full-update flag | **+580** | `bool m_bFullUpdate` | `bool m_bFullUpdate` |

Both frameworks agree on the offsets but **disagree on the semantics of +576** (slot vs. entity
index = slot+1). Per the doctrine these are borrowed constants and therefore go in
**`gamedata/core.gamedata.jsonc`** (`offsets` block: `CheckTransmitInfo.clientEntityIndex`,
`CheckTransmitInfo.fullUpdate`, keyed `linuxsteamrt64`) and are **validated on the first hook
fires, before any mutation is ever applied**:

- For each info in the first validated snapshot(s): read `i32` at +576. Every value must be in
  `[0, 65)` and must map to a **connected client slot** (tracked by our existing lifecycle hooks)
  under a single consistent interpretation — either `v == slot` for all infos or `v - 1 == slot`
  for all infos. The winning interpretation is cached (and logged) once it holds for a full
  snapshot with ≥1 client.
- `m_pTransmitEntity` (at +0) must be non-null and must have **bit 0 set** (the worldspawn,
  entity index 0, always transmits) — a strong cheap sanity check that offset 0 really is the
  transmit bitvec.
- Until validation passes the hook **observes only, never mutates**. If validation fails
  (either check, persisting across snapshots), the descriptor **disables itself with a named
  reason** — `GAMEDATA descriptor 'CheckTransmitInfo' FAILED: <reason>` — `Transmit.*` natives
  start returning `false`, and the framework keeps running (degrade per-descriptor, never crash).

This is the strongest self-validation available for call-context-only layout facts: they cannot be
resolved at boot (no info object exists until a snapshot is built), so "validated at load" becomes
"validated at first fire, fail-closed until then".

### 2.3 Fallbacks

- `Source2GameEntities001` missing from the server factory → WARN, no hook, `Transmit.*` natives
  return `false`. Everything else unaffected.
- Ops not assigned (old shim + new core) → core's existing op-missing degrade path; natives
  return `false`.
- Layout validation failure → §2.2 named disable; hook stays installed but inert (or is removed).

## 3. API shape — declarative rules, zero JS in the hot path

### 3.1 The perf argument (why not `Transmit.onCheck(cb)`)

`CheckTransmit` runs **per client per snapshot** (~tickrate). A JS callback API would cross the V8
boundary `clients × 64/s` times and hand JS a materialized entity list each call — cost paid even
by clients that filter nothing, plus `HOST.borrow_mut()` re-entrancy exposure on the hottest
engine path we hook. TTT itself only *changes* visibility on discrete events (role assignment,
death, taser reveal, round start); the per-snapshot work is a pure function of slowly-changing
state. So the state, not the callback, is the right thing to ship across the boundary:

**JS registers rules; the native side evaluates them.** Per snapshot, zero JS executes.

- TTT's `visible == ulong.MaxValue` fast path becomes structural: an entity with no rule has no
  entry; a client allowed by every rule costs one AND+branch per rule entry.
- Cost model: `O(rules)` per client per snapshot of pure bit ops. TTT worst case ≈ 64 icon
  entities × 64 clients = 4096 mask tests per snapshot — sub-microsecond territory. Measured in
  the live gate via built-in counters (§6).

### 3.2 The module — `@s2script/transmit` (engine-generic)

`ISource2GameEntities`/`CCheckTransmitInfo`/`CBitVec<16384>` are Source 2 engine facts, true on
any Source 2 title → the capability is **core + an engine-generic capability package**, nothing
in `games/`. Litmus test passes.

```ts
// @s2script/transmit — per-client entity visibility filtering.
import type { EntityRef } from "@s2script/entity";

export interface TransmitStats {
  snapshots: number;   // CheckTransmit invocations observed
  entries: number;     // live rule entries in the native table
  bitsCleared: number; // total transmit bits cleared since load/reset
  nsLast: number;      // ns spent in our post-hook, last invocation
  nsMax: number;       // worst invocation since load/reset
}

export const Transmit: {
  /**
   * Replace this plugin's visibility rule for `entity`: it will be transmitted
   * ONLY to the given viewer slots (empty array = hidden from everyone).
   * Returns false if the entity ref is stale or the capability is unavailable.
   * Throws RangeError on a slot outside [0, 64).
   */
  setVisibleTo(entity: EntityRef, viewers: readonly number[]): boolean;
  /** Remove this plugin's rule for `entity` (visible to all again, as far as this plugin is concerned). */
  reset(entity: EntityRef): boolean;
  /** Remove all of this plugin's rules. */
  resetAll(): void;
  /** Hot-path counters for measurement/debugging. */
  stats(): TransmitStats;
};
```

Naming follows the locked convention: PascalCase namespace object (`Transmit`, like `Events`,
`Damage`, `Chat`), camelCase methods.

**Semantics:**
- A rule is **per plugin, per entity**, keyed by serial-gated identity (`{index, serial}` — the
  same `EntityRef` contract as everywhere else; a raw pointer never crosses).
- **Multiple plugins AND-merge:** an entity is transmitted to viewer *v* only if *every* plugin
  holding a rule on it allows *v* (same "any suppressor wins" spirit as the `HookResult`
  collapse). Merging happens in core on mutation; the shim holds only the merged mask.
- Viewer sets cross as **plain number arrays** (no BigInt — the JSON/BigInt footgun stays out of
  the API); core folds them into a `u64` mask once at registration.
- Stale ref at registration → `false` (consistent with entity-system degrade semantics). Rules
  whose serial goes stale later become inert and are lazily evicted.
- **Ledgered:** every rule is recorded in the owning plugin's ledger; unload/reload walks the
  ledger, removes the plugin's rules, and recomputes merged masks. Map change needs no special
  handling — all serials change, rules go inert, plugins re-register (TTT rebuilds icons per
  round anyway).

**Deliberately excluded** (scope discipline): a per-snapshot JS event (`onCheck`) — if a future
consumer genuinely needs arbitrary per-snapshot logic it belongs behind the `unsafe` module with
its own spec; `show()`/force-transmit (needs `m_pTransmitAlways`, whose CS2 offset is *not*
corroborated — Swiftly says +32, hl2sdk says +8); hiding-players sugar.

## 4. Architecture & dispatch design

```
JS plugin ──setVisibleTo/reset──▶ core natives (__s2_transmit_*)
             per-plugin rule maps + ledger entries (Rust)
             merged mask per entity = AND over plugins
                    │ ops (S2EngineOps, append-only)
                    ▼
shim rule table: unordered_map<u16 entindex, {u32 serial, u64 mask}>
                    │ read-only in the hot path
                    ▼
Hook_CheckTransmit (POST) — per snapshot:
  [first fires: validate layout per §2.2; fail → named disable]
  for each rule entry:                      // entries outer, infos inner
    resolve slot's live serial ONCE (single lookup, no TOCTOU)
    mismatch → skip (evict if slot holds a DIFFERENT live entity)
    for each CCheckTransmitInfo (viewer slot v):
      if (!(mask >> v & 1) && bitvec->IsBitSet(index)) bitvec->Clear(index)
```

- **Hot path is shim-only.** No core ffi, no `HOST.borrow_mut()`, no V8. This sidesteps the
  known double-borrow re-entrancy class entirely.
- **New ops (append-only at the tail of `S2EngineOps`, byte-identical in `core/src/v8host.rs`
  and `shim/include/s2script_core.h`):**
  - `transmit_set(index: u32, serial: u32, mask: u64) -> bool` — upsert merged mask; validates
    the serial against the live entity; `false` if stale/table-full/capability-disabled.
  - `transmit_clear(index: u32) -> bool` — drop the entry.
  - `transmit_stats(out: *mut u64 /* [5]: snapshots, entries, bitsCleared, nsLast, nsMax */)`.
- Core owns **policy** (per-plugin rules, AND-merge, ledger teardown); shim owns **mechanism**
  (the table + the hook). On any mutation core recomputes the entity's merged mask and pushes one
  `transmit_set`/`transmit_clear`.
- Table capped (4096 entries) — `setVisibleTo` returns `false` beyond it; degrade, never grow
  unboundedly.
- Timing: `clock_gettime(CLOCK_MONOTONIC)` around the post-hook body, accumulated into the
  stats counters — the measurement the live gate reports.

**Not a multiplexed event, and why that's within the contract:** plugins never see the raw
detour; they get a named, typed *capability* whose registrations are ledgered — the same shape as
the command registry and the admin cache (both precedents for registry-style capabilities where a
per-event JS dispatch would be wrong). The single engine touchpoint stays core-owned: exactly one
hook, one owner, N plugins composing through the AND-merge instead of fighting over the bitvec.

## 5. Boundary check

- Core/shim: engine-generic Source 2 only (`ISource2GameEntities` is in eiface.h beside
  `ISource2Server`). No `games/*` import; `make check-boundary` stays green.
- `@s2script/transmit` references only `EntityRef` from `@s2script/entity` (engine-generic).
- The TTT policy (who sees whose icons) lives entirely in the consumer. The demo plugin lives in
  `examples/` (not shipped).

## 6. Live-gate plan

Env: Docker CS2 dev server (`make docker-test`, `/patch-gameinfo.sh`, restart — never
`--force-recreate`), sniper-built binaries only, rcon via `scripts/rcon.py`.

1. **Boot evidence:** `[s2script] interface OK: Source2GameEntities (Source2GameEntities001)` +
   `CheckTransmit hook installed` in console; gamedata banner counts the new offsets.
2. **Validation evidence:** first-fire layout validation log — which interpretation of +576 won
   (slot vs entindex), worldspawn-bit sanity pass. **Known unknown:** whether CheckTransmit fires
   with only bots connected (bots have no netchannel). If it doesn't, snapshot-dependent evidence
   needs one human client; that step is flagged for human verification rather than silently
   skipped (deferred-live-tests convention).
3. **Functional evidence (deterministic, rcon-driven):** demo plugin `transmit-demo` registers
   commands: `!tspawn` spawns a `prop_dynamic` in front of spawn; `!thide <slot>` calls
   `Transmit.setVisibleTo(prop, all slots except <slot>)`; `!tshow` resets; `!tstats` prints
   `Transmit.stats()`. Server-side proof of mutation = `bitsCleared` increasing while a rule is
   active and the entity is in the viewer's PVS (the bit was set by the game, cleared by us);
   client-side visual proof (prop pops out for exactly the filtered client) = human check.
4. **Perf evidence:** `!tstats` with a realistic table (spawn ~16 props, rules on all) →
   report `nsLast`/`nsMax` per snapshot. Budget: < 50 µs/snapshot at that load (expected well
   under; the loop is ~entries × clients bit ops).
5. **Teardown evidence:** remove the demo `.s2sp` → file-watch unload → `!tstats` shows
   `entries: 0` via a second plugin or the rcon status line; reload leaves no duplicate rules.

## 7. Testing

- `cargo test -p s2script-core`: natives with mocked ops — mask folding from viewer arrays
  (empty → 0, [0,63] → bit 0|bit 63, out-of-range → throw), AND-merge across two plugin
  contexts, ledger teardown emits `transmit_clear` for every owned entry, op-missing degrade
  (`false`), stats plumbing.
- Shim logic (layout validation, bitvec mutation) is live-gate territory; the pure helpers
  (interpretation resolution) are written as small testable functions.
- `./scripts/check-plugins-typecheck.sh` covers the new `.d.ts` (5E.1 gate); changeset required
  (`packages/*` changed).

## 8. Risks / STOP conditions

- +576/+580 wrong on our build → validation fails → named disable; STOP and report (do not
  improvise offsets).
- Hook fires but `Clear()` has no visible client effect → STOP and report.
- CheckTransmit doesn't fire with bots-only → limits gate automation; report, request human step.
- Per-snapshot cost > budget → STOP and report with numbers.
