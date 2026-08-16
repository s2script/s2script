# Pickup gates — CanAcquire (PR1) / CanUse+CanEquip (PR2)

**Status:** Locked 2026-08-14 (grill). Ready to implement.
**Audience:** core/shim + `@s2script/cs2` maintainers.
**Builds on:** declarative inbound hooks (ARCHITECTURE §2.0.7); `LiveTable` / `EntityRef` (E1); handle-vs-pointer research (`2026-08-14-handle-vs-pointer-research.md`).
**Stack:** new, from `main`. Independent of #106 and #101.

---

## 1. Why

Plugins can give, strip, and paint items. They cannot **refuse a pickup**. Store, restrict, and inventory-sim plugins need the engine’s gate (`CCSPlayer_ItemServices::CanAcquire`), not a race after `giveNamedItem`.

A Handle is the wrong primitive: `CEconItemView*` has no host identity. The view is block-scoped scalars. See the research note.

## 2. What does not change

`EntityRef` / `LiveTable` / ledger. No JS pointer. No `Handle<T>` for the item view. No `Engine.hook` for this — the **game package declares**, plugins subscribe. `engine:hooks` is not required to listen.

## 3. PRs

| PR | Ships |
|---|---|
| **1** | Return-value thunk `this_i64_i32_i64` + `ctx.items.onCanAcquire` / `onCanAcquirePost` + gamedata + live-gate fixture |
| **2** | Bool thunk + `onCanUse` / `onCanUsePost` / `onCanEquip` / `onCanEquipPost` |

## 4. PR1 author surface

```ts
ctx.items.onCanAcquire((acq: CanAcquireView) => {
  if (blocked(acq.defIndex)) {
    acq.result = AcquireResult.InvalidItem;
    return HookResult.Handled;
  }
});

ctx.items.onCanAcquirePost((acq: CanAcquireView) => {
  // readonly, including result
  // acq.skipped === true iff Pre skipped the original
});
```

| Field | Type | Notes |
|---|---|---|
| `player` | `Player \| null` | Still fires if the ItemServices→pawn hop misses. WARN once per hook, not per call. |
| `defIndex` | `number` | `CEconItemView.m_iItemDefinitionIndex` (u16). Read in the thunk; never a pointer. |
| `method` | `AcquireMethod` | The i32 register arg. Read-only. |
| `result` | `AcquireResult` | Writable on Pre. Seed `Allowed` (0). Readonly on Post. |
| `skipped` | `boolean` | Post only. True when Pre skipped the original. |

`pawn.giveNamedItem` **fires** this hook. A give from *inside* a handler is skipped and named (isolate re-entrancy). There is **no** `bypassWith` on this hook (the opposite of `onTerminateRound`).

## 5. Collapse (Pre)

- **Continue** — no vote.
- **Changed** — vote `result`, still call original.
- **Handled / Stop** — skip original. Unset `result` counts as a non-Allow (use `InvalidItem` as the implicit deny code if the handler never wrote `result`).
- Then `mostRestrictive(plugin votes, engine return if called)`.
- Any non-`Allowed` beats `Allowed`. Two non-Allows: HookResult precedence, then registration order. No deny-reason ranking.

Post is a **spectator**. Readonly view. `HookResult` ignored. Always runs if subscribed, including after a Pre skip (`skipped: true`). Cannot un-skip.

## 6. Shape

`this_i64_i32_i64` — `i32(void* self, int64 itemView, int32 method, int64 unknown)`.

- `itemView` and `unknown` are opaque i64 pass-through. No JS accessor.
- `method` is the one addressable i32 param.
- Return is `i32` (`AcquireResult`). New: the thunk **returns** this to the engine.
- `self` is `CCSPlayer_ItemServices*`, not an entity. Player is recovered by walking live pawns and comparing `m_pItemServices` to `self` (field name is gamedata). Miss ⇒ `player: null`.

Linux signature (CS2Fixes / cs2-signatures, 2026-08-12, self-validate at install):

```
55 48 89 E5 41 57 41 56 41 55 49 89 CD 41 54 49 89 FC 53 48 89 F3 48 83 EC
```

Mandatory `validate.prologue` on the hook target. Uniqueness-alone is not enough.

## 7. Enums (`@s2script/cs2`)

`AcquireResult.Allowed = 0`. Every other documented engine code is a deny. Numeric values match the engine (copied from the live enum / CS2Fixes / ModSharp `EAcquireResult`); a drift is a treadmill item, not a silent remap.

`AcquireMethod` is the pickup/buy/… discriminator the engine passes in the i32 slot.

## 8. Live-gate (PR1)

1. Natural pickup fires the handler.
2. `giveNamedItem` fires the handler.
3. `Handled` + deny withholds the item.
4. `giveNamedItem` from inside a handler is skipped and named.
5. No subscriber ⇒ boot log shows the detour was never installed.
6. Post sees `skipped: true` on a Pre-Denied give.

Fixture lives under `tools/` (hookgate style). No cookbook, no restrict plugin.

## 9. Out

CanUse / CanEquip (PR2). Extra econ fields on the view. Cookbook / base restrict plugin. Steam. Inbound net messages. JS-visible Handles.
