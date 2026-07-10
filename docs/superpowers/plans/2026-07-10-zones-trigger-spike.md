# Zones sub-slice 1 — trigger-zone spike — Implementation Plan

> **Execution:** controller-driven (interactive) with a live iteration loop — a spike needs live touch/no-touch feedback to walk the recipe ladder. NOT a fire-and-forget workflow.

**Goal:** Prove a runtime `trigger_multiple` with a programmatic AABB fires touch for a player, wired to `OnZoneEnter`/`OnZoneLeave`. Hardcoded/command-placed zone; no persistence/interface.

**Architecture:** A CS2 helper `TriggerZone` in `games/cs2/js/pawn.js` (the `Beam`-helper pattern — create + raw collision schema writes + teleport + spawn), plus a `zones-spike` plugin (`examples/`) that calls it, hooks `OnStartTouch`/`OnEndTouch` via `Entity.onOutput`, and (fallback) polls `m_hTouchingEntities`.

**Tech Stack:** `games/cs2/js/pawn.js` + `packages/cs2/index.d.ts` (CS2 layer), `examples/zones-spike` (plugin: `@s2script/entity` + `@s2script/cs2` + `@s2script/commands`). JS-only in the happy path — redeploy the concatenated `js/pawn.js` + restart (NO sniper); plugin changes hot-reload.

## Global Constraints

- All zone logic is game-layer; the raw collision schema writes (CS2/Source2 field names `m_Collision`/`m_vecMins`/`SOLID_BBOX`) stay in `pawn.js` (the `Beam` precedent) — never core. Boundary gates stay green.
- A `pawn.js` change needs the game package redeployed (`scripts/package-addon.sh` concatenates `schema.generated.js`+`nav.generated.js`+`pawn.js` → `dist/addons/s2script/js/pawn.js`) + a container restart (the game package registers once at boot — NOT hot-reloadable). Plugin `.s2sp` changes hot-reload (no restart).
- Degrade-never-crash: every helper null-guards `createEntity`/offset resolution; a stale ref reads null.
- Only introduce a new engine op if the iteration ladder (spec §Iteration ladder) proves schema-writes + existing primitives can't make the trigger fire touch — and only the precise one the SDK dictates (`collision_rules_changed`/`entity_set_size`).

## File Structure

- `games/cs2/js/pawn.js` — the `TriggerZone` helper (`create(min,max,opts) → handle`, `touching(ref) → EntityRef[]`); export into `__s2pkg_cs2`.
- `packages/cs2/index.d.ts` — `TriggerZone` types + a `ZoneBox` shape.
- `examples/zones-spike/{package.json,tsconfig.json,src/plugin.ts}` — `sm_zonetest` / `sm_zonetest_clear`.

---

## Task 1: The `TriggerZone` CS2 helper (pawn.js)

- [ ] **Step 1** — In `games/cs2/js/pawn.js`, add near `Beam` (mirrors its create→write→teleport→spawn shape). Collision fields are on the embedded `m_Collision`, so combine offsets:

```js
  // TriggerZone — a runtime trigger_multiple with a programmatic AABB (zones spike). Mirrors Beam:
  // createEntity + raw schema writes + teleport + spawn. Detection is the caller's (Entity.onOutput on
  // OnStartTouch/OnEndTouch, or poll TriggerZone.touching). Game-world-owned; caller owns remove().
  var SOLID_BBOX = 2;
  function collOffset(field) {
    var base = __s2_schema_offset("CBaseModelEntity", "m_Collision");   // embedded CCollisionProperty
    var rel  = __s2_schema_offset("CCollisionProperty", field);
    return (base >= 0 && rel >= 0) ? (base + rel) : -1;
  }
  function writeVecAt(ref, off, x, y, z) {
    if (off < 0) return false;
    var ok = ref.writeFloat32(off, +x) && ref.writeFloat32(off + 4, +y) && ref.writeFloat32(off + 8, +z);
    if (ok) ref.notifyStateChanged(off);
    return !!ok;
  }
  var TriggerZone = {
    // min/max = world-space corners ({x,y,z}). opts.spawnflags (default 1 = clients), opts.setBboxBeforeSpawn.
    create: function (min, max, opts) {
      opts = opts || {};
      var ent = globalThis.__s2pkg_entity;
      var cx = (min.x + max.x) / 2, cy = (min.y + max.y) / 2, cz = (min.z + max.z) / 2;
      var hx = Math.abs(max.x - min.x) / 2, hy = Math.abs(max.y - min.y) / 2, hz = Math.abs(max.z - min.z) / 2;
      var sf = opts.spawnflags != null ? opts.spawnflags : 1;
      var ref = ent.createEntity("trigger_multiple", { spawnflags: String(sf), wait: "0", StartDisabled: "0" });
      if (!ref) return null;
      var applyBbox = function () {
        var stOff = collOffset("m_nSolidType"); if (stOff >= 0) { ref.writeUInt8(stOff, SOLID_BBOX); ref.notifyStateChanged(stOff); }
        writeVecAt(ref, collOffset("m_vecMins"), -hx, -hy, -hz);
        writeVecAt(ref, collOffset("m_vecMaxs"),  hx,  hy,  hz);
        var dOff = __s2_schema_offset("CBaseTrigger", "m_bDisabled"); if (dOff >= 0) { ref.writeBool(dOff, false); ref.notifyStateChanged(dOff); }
      };
      if (opts.setBboxBeforeSpawn) applyBbox();
      ref.teleport([cx, cy, cz]);
      ref.spawn();
      if (!opts.setBboxBeforeSpawn) applyBbox();      // default: after DispatchSpawn (Spawn may recompute)
      if (opts.enableInput) ref.acceptInput("Enable");
      return { ref: ref, center: { x: cx, y: cy, z: cz }, remove: function () { return ref.remove(); } };
    },
    // Currently-touching entities via m_hTouchingEntities (CUtlVector<CHandle>) — the engine-collision poll fallback.
    touching: function (ref) {
      var off = __s2_schema_offset("CBaseTrigger", "m_hTouchingEntities");
      if (off < 0 || !ref) return [];
      return ref.readHandleVector([], off, 64) || [];
    }
  };
```
Note `writeBool` exists on `EntityRef`; if `readHandleVector`'s signature is `(ptrOffs, vectorOff, maxCount)`, `[]` = no pointer chain (the vector is directly on the trigger). Verify both signatures in `packages/entity/index.d.ts` before finalizing.

- [ ] **Step 2** — Export into `__s2pkg_cs2`: add `TriggerZone: TriggerZone` to the `Object.assign({}, ..., { … })` at the pawn.js export site.

- [ ] **Step 3** — `packages/cs2/index.d.ts`:
```ts
export interface ZoneBox { x: number; y: number; z: number; }
export interface TriggerZoneHandle { ref: EntityRef; center: ZoneBox; remove(): boolean; }
export const TriggerZone: {
  create(min: ZoneBox, max: ZoneBox, opts?: { spawnflags?: number; setBboxBeforeSpawn?: boolean; enableInput?: boolean }): TriggerZoneHandle | null;
  touching(ref: EntityRef): EntityRef[];
};
```
(`EntityRef` is already imported/exported in `packages/cs2/index.d.ts`.)

---

## Task 2: The `zones-spike` plugin

- [ ] **Step 1** — `examples/zones-spike/package.json` (mirror `examples/clientlist-convar-mapstart-demo`): name `@demo/zones-spike`, `s2script.apiVersion "1.x"`. `tsconfig.json` extends the base.

- [ ] **Step 2** — `src/plugin.ts`:
```ts
import { Commands } from "@s2script/commands";
import { Entity } from "@s2script/entity";
import { Pawn, Player, TriggerZone, TriggerZoneHandle } from "@s2script/cs2";
import { HookResult } from "@s2script/events";

let zone: TriggerZoneHandle | null = null;
const inside = new Set<number>();   // entity indices currently inside (poll-fallback state)

function activatorName(activator: { index: number } | null): string {
  // best-effort resolve the touching entity -> a Player name
  if (!activator) return "unknown";
  const p = Player.all().find((pl) => { const pw = pl.pawn; return pw && pw.ref.index === activator.index; });
  return p ? `${p.playerName} (slot ${p.slot})` : `ent#${activator.index}`;
}

export function onLoad(): void {
  // Hook the touch outputs (primary, event-driven path). Wildcard-safe: only our trigger exists in the spike.
  Entity.onOutput("trigger_multiple", "OnStartTouch", (ev) => {
    console.log(`[zonestest] OnStartTouch (output): ${activatorName(ev.activator)}`);
    return HookResult.Continue;
  });
  Entity.onOutput("trigger_multiple", "OnEndTouch", (ev) => {
    console.log(`[zonestest] OnEndTouch (output): ${activatorName(ev.activator)}`);
    return HookResult.Continue;
  });

  // sm_zonetest [slot] [half] — create a box centered on a bot's origin (default slot 0), half-extent (default 96).
  Commands.register("sm_zonetest", (ctx) => {
    if (zone) { ctx.reply("[zonestest] a zone already exists — sm_zonetest_clear first"); return; }
    const slot = ctx.args.length > 0 ? parseInt(ctx.args[0], 10) : 0;
    const half = ctx.args.length > 1 ? parseFloat(ctx.args[1]) : 96;
    const pw = Pawn.forSlot(slot);
    const o = pw ? pw.origin : null;
    if (!o) { ctx.reply(`[zonestest] no pawn/origin for slot ${slot}`); return; }
    const min = { x: o.x - half, y: o.y - half, z: o.z - half };
    const max = { x: o.x + half, y: o.y + half, z: o.z + half };
    zone = TriggerZone.create(min, max, { spawnflags: 1 });
    ctx.reply(`[zonestest] zone ${zone ? "created ref#" + zone.ref.index : "FAILED"} @ (${o.x.toFixed(0)},${o.y.toFixed(0)},${o.z.toFixed(0)}) half=${half}`);
    inside.clear();
  });

  // Poll fallback: each ~0.5s, diff m_hTouchingEntities -> enter/leave (proves engine collision even if outputs don't fire).
  let frame = 0;
  // (wire OnGameFrame in the plugin — throttle to ~every 32 frames)
  // NOTE: import { OnGameFrame } from "@s2script/frame" and subscribe here.

  Commands.register("sm_zonetest_clear", (ctx) => {
    if (!zone) { ctx.reply("[zonestest] no zone"); return; }
    const ok = zone.remove(); zone = null; inside.clear();
    ctx.reply(`[zonestest] removed -> ${ok}`);
  });

  console.log("[zones-spike] onLoad — sm_zonetest / sm_zonetest_clear registered");
}
```
(Finalize the `OnGameFrame` poll in-code: on each throttled tick, `const cur = new Set(TriggerZone.touching(zone.ref).map(r => r.index))`; log indices newly-in vs newly-out vs `inside`; update `inside`. The poll is the engine-collision proof if the output hooks don't fire.)

- [ ] **Step 3** — Build: `( cd packages/cli && node build.mjs ) && node packages/cli/dist/cli.js build examples/zones-spike`; `bash scripts/check-plugins-typecheck.sh` green.

---

## Deploy + live gate + iterate (the spike loop)

- [ ] **Deploy** — redeploy the game package js + the plugin, restart:
  - `bash scripts/package-addon.sh` (or copy the concatenated `js/pawn.js`) then `cp examples/*/dist/*.s2sp plugins/*/dist/*.s2sp dist/addons/s2script/plugins/` and `cd docker && docker compose restart cs2`. (No sniper in the happy path.)
- [ ] **Gate** — `bot_quota 4`; `sm_zonetest 0 96` (box on a bot) then watch, and `sm_zonetest 0 300` (a big box likely overlapping several bots). Observe:
  - `[zonestest] OnStartTouch (output): <name>` — the ideal (event-driven works), OR
  - the poll logs `entered ent#N` — engine collision works, output hook doesn't (ship poll-derived; note the gap).
- [ ] **Iteration ladder** (spec §) — if NEITHER fires, redeploy pawn.js with, in order: `spawnflags` variants (try the CS2 client/all set — probe values); `enableInput: true`; `setBboxBeforeSpawn: true`; then the `m_hTouchingEntities` read alone. If the list stays empty on a known-overlapping bot after all of these → **STOP + report**: runtime bbox triggers need a collision game-fn (candidate: a new `collision_rules_changed`/`entity_set_size` op, or a `CBaseTrigger::Enable`/`CollisionRulesChanged` sig) — decide with the user before adding an engine primitive.
- [ ] **Report** — the working recipe (spawnflags, bbox order, enable, output-vs-poll), `RestartCount=0`, no crash; whether sub-slice 2 needs a new op. Commit the spike (pawn.js helper + plugin) with the findings; update the spec's "working recipe" section.

## Self-review

- Spec coverage: the trigger recipe (Task 1), the dual detection (output hook + poll, Task 2), the hardcoded/command-placed box + the live gate + the ladder — all covered.
- The one unknown (spawnflags value / whether a collision call is needed) is the spike's deliverable, walked via the ladder — not a placeholder but the empirical target.
- Types: `TriggerZone.create/touching` (`.d.ts`) ↔ the pawn.js helper ↔ the plugin usage; `TriggerZoneHandle.ref/center/remove`.
