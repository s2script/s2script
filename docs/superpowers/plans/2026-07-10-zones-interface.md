# Zones sub-slice 3 — the plugin-developer interface — Implementation Plan

> **Execution:** controller-authored (a producer edit + a new consumer plugin, game-layer, no sniper). Live-gated on bots.

**Goal:** `plugins/zones` publishes `@s2script/zones@1.0.0` (methods + `enter`/`leave`/`stay` events); a `examples/zones-consumer-demo` consumes it and heals players in a `heal` zone (proving a separate plugin reacts to zone events).

## Global Constraints

- Game-layer only; no core/shim/sniper. Both boundary gates + full-strict typecheck green.
- Interface methods are SYNCHRONOUS (registry-backed); the DB write fires-and-forgets (a Promise can't cross the structured-copy wire).
- Event payloads are wire-safe plain data `{ zone, slot, userId }` — never a Player object.
- git `-F -` heredoc + Claude-Session trailer.

## File Structure

- `plugins/zones/src/plugin.ts` — add `publishInterface` + emit in the poll; add `import { publishInterface, PublishHandle } from "@s2script/interfaces"`.
- `plugins/zones/package.json` — (publishInterface is a runtime global; no dep change needed, but note `@s2script/interfaces` in devDeps for types if the build wants it — mirror greeter-plugin).
- `examples/zones-consumer-demo/{package.json,tsconfig.json,src/plugin.ts,src/zones.d.ts}` — the consumer.

## Task 1: Producer — publish the interface + emit events

- [ ] **Step 1** — In `plugins/zones/src/plugin.ts`, add the import + a module-level handle:
```ts
import { publishInterface, PublishHandle } from "@s2script/interfaces";
// ...
let iface: PublishHandle | null = null;
```

- [ ] **Step 2** — In `onLoad`, after the DB IIFE is kicked off, publish the interface (synchronous methods over the registry). Add a `userIdOf(slot)` helper (`Player.fromSlot(slot)?.userId ?? -1`):
```ts
  iface = publishInterface("@s2script/zones", "1.0.0", {
    createZone(name: string, min: Vec3, max: Vec3): boolean {
      const nm = sanitizeName(name);
      if (!nm) return false;
      const box = normBox(min, max);
      if (box.min.x === box.max.x || box.min.y === box.max.y || box.min.z === box.max.z) return false;
      upsertZone(nm, box).catch(() => {});   // registry updates inside upsertZone (after dbReady); fire-and-forget the DB
      zones.set(nm, { name: nm, min: box.min, max: box.max, inside: zones.get(nm)?.inside ?? new Set<number>() }); // immediate registry
      return true;
    },
    deleteZone(name: string): boolean {
      const nm = sanitizeName(name);
      if (!zones.has(nm)) return false;
      zones.delete(nm);
      if (db) db.execute("DELETE FROM zones WHERE map = ? AND name = ?", [currentMap, nm]).catch(() => {});
      return true;
    },
    getZones(): { name: string; min: Vec3; max: Vec3 }[] {
      return Array.from(zones.values()).map((z) => ({ name: z.name, min: z.min, max: z.max }));
    },
    isInZone(slot: number, name: string): boolean {
      const z = zones.get(sanitizeName(name));
      return !!z && z.inside.has(slot);
    },
    zonesFor(slot: number): string[] {
      const out: string[] = [];
      for (const z of zones.values()) if (z.inside.has(slot)) out.push(z.name);
      return out;
    },
  });
  console.log("[zones] publishing @s2script/zones@1.0.0");
```
Note: `createZone` sets the registry immediately (so `isInZone`/the poll see it at once) AND calls `upsertZone` (which awaits `dbReady` then re-sets the registry + writes the DB) — the double registry-set is harmless (same value); the immediate one avoids a race where the poll runs before `dbReady`.

- [ ] **Step 3** — In the detection poll, replace the `console.log` ENTER/LEAVE with emits + add `stay`. `userId` via a small cache to avoid a `fromSlot` per event (resolve once per player per tick):
```ts
  OnGameFrame.subscribe(() => {
    if ((frame++ & 7) !== 0 || zones.size === 0) return;
    const players = Player.all();
    const uid = new Map<number, number>();
    for (const p of players) uid.set(p.slot, p.userId);
    for (const z of zones.values()) {
      const cur = new Set<number>();
      for (const p of players) {
        const pw = p.pawn; if (!pw) continue;
        const o = pw.origin; if (!o) continue;
        if (contains(z, o.x, o.y, o.z)) {
          cur.add(p.slot);
          if (!z.inside.has(p.slot) && iface) iface.emit("enter", { zone: z.name, slot: p.slot, userId: uid.get(p.slot) ?? -1 });
          if (iface) iface.emit("stay", { zone: z.name, slot: p.slot, userId: uid.get(p.slot) ?? -1 });
        }
      }
      for (const s of z.inside) if (!cur.has(s) && iface) iface.emit("leave", { zone: z.name, slot: s, userId: uid.get(s) ?? -1 });
      z.inside = cur;
    }
  });
```

- [ ] **Step 4** — build `plugins/zones` + typecheck. (`publishInterface`/`PublishHandle` from `@s2script/interfaces` — the CLI externalizes `@s2script/*`; add it to the plugin's imports as greeter-plugin does. No package.json dep needed for the runtime; if typecheck wants it, the `paths` resolution covers it.)

## Task 2: Consumer — react to zone events

- [ ] **Step 1** — `examples/zones-consumer-demo/package.json` (mirror greeter-consumer: name `@demo/zones-consumer-demo`, `s2script.pluginDependencies: { "@s2script/zones": "^1.0.0" }` if the manifest models it — else just import). `tsconfig.json` extends base + includes `src`.

- [ ] **Step 2** — `src/zones.d.ts` (hand-written ambient interface stub, mirroring greeter.d.ts):
```ts
declare module "@s2script/zones" {
  interface Vec3 { x: number; y: number; z: number; }
  interface ZoneEvent { zone: string; slot: number; userId: number; }
  interface Zones {
    createZone(name: string, min: Vec3, max: Vec3): boolean;
    deleteZone(name: string): boolean;
    getZones(): { name: string; min: Vec3; max: Vec3 }[];
    isInZone(slot: number, name: string): boolean;
    zonesFor(slot: number): string[];
    on(event: "enter" | "leave" | "stay", handler: (p: ZoneEvent) => void): number;
    off(event: string, handler: (...args: any[]) => void): void;
  }
  const _default: Zones;
  export = _default;
}
```

- [ ] **Step 3** — `src/plugin.ts`:
```ts
import { on, getZones } from "@s2script/zones";   // hard dep proxy
import { Player } from "@s2script/cs2";

const healTick = new Map<number, number>();   // slot -> last heal frame (throttle)
let frame = 0;

export function onLoad(): void {
  on("enter", (p) => {
    const nm = Player.fromSlot(p.slot)?.playerName ?? `slot ${p.slot}`;
    console.log(`[zones-consumer] ENTER ${p.zone}: ${nm}`);
  });
  on("leave", (p) => {
    const nm = Player.fromSlot(p.slot)?.playerName ?? `slot ${p.slot}`;
    console.log(`[zones-consumer] LEAVE ${p.zone}: ${nm}`);
  });
  // The real reaction: heal players standing in a "heal" zone (throttled ~1/s).
  on("stay", (p) => {
    if (p.zone !== "heal") return;
    frame++;
    const last = healTick.get(p.slot) ?? 0;
    if (frame - last < 8) return;   // ~1 Hz at the producer's ~8 Hz stay
    healTick.set(p.slot, frame);
    const pw = Player.fromSlot(p.slot)?.pawn;
    if (pw && pw.health != null && pw.health < 100) { pw.health = Math.min(100, pw.health + 5); }
  });
  try { console.log(`[zones-consumer] onLoad — subscribed; getZones()=${getZones().length}`); }
  catch (e) { console.log(`[zones-consumer] onLoad — subscribed (producer absent: ${String(e)})`); }
}
```

- [ ] **Step 4** — build both + `check-plugins-typecheck.sh` green.

## Deploy + live gate

- [ ] Deploy both `.s2sp` (hot-reload: `cp plugins/zones/dist/*.s2sp examples/zones-consumer-demo/dist/*.s2sp dist/addons/s2script/plugins/`), no sniper.
- [ ] **Gate** (de_inferno, `bot_quota 4`, rcon):
  - Boot: `[zones] publishing @s2script/zones@1.0.0` + `[zones-consumer] onLoad — subscribed; getZones()=…`.
  - `sm_zone_add heal <coords around a bot>` → the CONSUMER logs `[zones-consumer] ENTER heal: <name>` (the event crossed the interface). `sm_slap <bot> 40` to lower a bot's health, then confirm it **climbs back toward 100** while the bot is in `heal` (the consumer's heal reaction) — read via a health probe (a temporary log or a `sm_who`-adjacent check; or add a debug `sm_zone_hp <slot>` in the consumer if needed).
  - Move/`bot_stop 0` so a bot exits → `[zones-consumer] LEAVE heal: <name>`.
  - `RestartCount=0`, no crash.
- [ ] Merge, push, document (CLAUDE.md + memory), mark the zone system's 3 sub-slices done.

## Self-review

- Spec coverage: the 5 interface methods, the 3 emitted events (wire-safe payload), the consumer subscribe + heal behavior, the hand-written `.d.ts` — all present.
- Wire-safety: payloads are `{ zone, slot, userId }` (plain); methods return plain values (boolean/array).
- The producer's registry-immediate + fire-and-forget-DB split in `createZone` avoids the async-return-over-wire problem.
