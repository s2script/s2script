# Slice 5C.2 — The Player Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `Player` abstraction (the CS2 controller) to `@s2script/cs2` — generated `CCSPlayerController` accessors + `.slot` + `.pawn` navigation + `Player.fromSlot`/`Player.all`, plus a `Pawn.controller` reverse hop — so plugins reference the persistent player, not just the body.

**Architecture:** Pure JS + types in the game-package layer. `Player` mirrors `Pawn`: a constructor over the controller `EntityRef`, the generated `CCSPlayerController` accessors applied to its prototype, then hand-written typed nav (`player.pawn`, `pawn.controller`) that shadows the raw generated handle fields (`configurable:true`). No core/shim/`package-addon` change, no sniper rebuild.

**Tech Stack:** The injected `games/cs2/js/pawn.js` runtime + `packages/cs2/index.d.ts` types; the CLI `node:test` vm-compose harness; the Docker CS2 live gate.

## Global Constraints

Every task's requirements implicitly include these (spec §11):

- **Core stays engine-generic.** All Player/Pawn code + CS2 identifiers live in `games/cs2` + `packages/cs2`; NOTHING enters `core/src`. Both gates green: `bash scripts/check-core-boundary.sh` (EXIT 0), `bash scripts/test-boundary-nameleak.sh` (PASS).
- **Never expose a raw pointer across time.** `Player`/`Pawn` are `EntityRef`-backed; every access serial-gated → `T | null`; `.pawn`/`.controller` return typed wrappers over `readHandle` (an `EntityRef`), never a raw pointer; a stored `Player` degrades to `null` on reuse.
- **Layout is data.** Fields resolve live via the generated accessors + `__s2_schema_offset`; no offsets baked.
- **Deterministic codegen stays green.** 5C.2 touches no generated file; `bash scripts/check-schema-generated.sh` stays green.
- **Naming:** PascalCase types (`Player`), camelCase props/fns (`player.pawn`, `Player.fromSlot`, `player.slot`).
- **Commit trailer:** every commit ends EXACTLY with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. Commit only on `slice-5c2-player-model`; do NOT push.

**Deferred — do NOT build:** the engine-identity follow (`player.userId` + `Player.fromUserId`, `player.name`, `player.steamId` — need S2EngineOps natives); `fromClient` (1-based bridge); the full SM `GetClient*` surface; the `@s2script/cs2` internal split; a `maxplayers`/`connected` engine op; the base-plugin suite (6); the registry (5.5); config/permissions; the `tsc` gate; `enum`/`Vector`/string codegen; the 5B.3 codegen post-merge TODOs.

**Key field facts (from the committed schema catalog):** the controller→pawn handle is `m_hPlayerPawn` on `CCSPlayerController`; the pawn→controller handle is `m_hController` on `CBasePlayerPawn`. The generated `CCSPlayerController` has a raw `pawn` (m_hPawn) + `playerPawn` (m_hPlayerPawn); the generated `CCSPlayerPawn` has a raw `controller` (m_hController). The typed `player.pawn`/`pawn.controller` shadow the raw `pawn`/`controller`.

---

## Task 1: The `Player` runtime + types + vm test

**Files:**
- Modify: `games/cs2/js/pawn.js` (add `Player` + `Pawn.prototype.controller` + export `{ Pawn, Player }`)
- Modify: `packages/cs2/index.d.ts` (the `Player` interface/const + `Pawn.controller`)
- Modify: `packages/cli/test/schema-runtime.test.mjs` (a new vm test for the Player wiring)

**Interfaces:**
- Consumes: the generated `globalThis.__s2pkg_cs2_schema.applyAccessors(proto, className)`; `EntityRef` from `__s2require("@s2script/entity")`; `__s2_schema_offset`, `__s2_ent_current_serial`; `EntityRef.readHandle`/`isValid`.
- Produces: `globalThis.__s2pkg_cs2 = { Pawn, Player }`; `Player.fromSlot`/`all`; `player.slot`/`pawn`; `pawn.controller`.

- [ ] **Step 1: Write the failing vm test.** Add to `packages/cli/test/schema-runtime.test.mjs` (it already reads `genJs`/`pawnJs` at module scope + evals them in a `node:vm` sandbox). Add a new `test()` reusing that pattern with stub natives, asserting the Player wiring:

```js
test("Player model: fromSlot/all, generated accessors, .pawn + .controller nav (offline vm)", () => {
  // Stub EntityRef: isValid() true, typed reads return fixed values, readHandle returns a fresh ref (nav).
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return true; };
  EntityRef.prototype.readInt32 = function () { return 2; };          // e.g. teamNum = 2
  EntityRef.prototype.readFloat32 = function () { return 0.25; };
  EntityRef.prototype.readBool = function () { return false; };
  EntityRef.prototype.readHandle = function () { return new EntityRef(this.index + 100, 7); }; // a live nav target
  const stdEntity = { EntityRef };
  const ctx = {
    __s2require: (n) => (n === "@s2script/entity" ? stdEntity : null),
    __s2_schema_offset: () => 8,               // any non-negative offset
    __s2_ent_current_serial: () => 7,
    __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);
  const { Player, Pawn } = ctx.__s2pkg_cs2;
  assert.equal(typeof Player, "function");

  const p = Player.fromSlot(0);                // controller ref (1, 7)
  assert.ok(p, "fromSlot(0) returns a Player");
  assert.equal(p.slot, 0, "slot is 0-based (ref.index - 1)");
  assert.equal(p.teamNum, 2, "generated CCSPlayerController accessor reads through the controller ref");
  const body = p.pawn;                         // readHandle(m_hPlayerPawn) -> a Pawn
  assert.ok(body, "player.pawn is a Pawn");
  assert.equal(body.health, 2, "the pawn's generated accessor reads through the pawn ref");
  const back = body.controller;               // readHandle(m_hController) -> a Player
  assert.ok(back, "pawn.controller is a Player");
  assert.equal(typeof back.teamNum, "number", "the round-tripped controller has generated accessors");

  const all = Player.all();                    // stub isValid() always true → 64 players
  assert.equal(all.length, 64, "Player.all() iterates the slot range and keeps valid controllers");
  assert.equal(all[0].slot, 0);
  assert.equal(all[63].slot, 63);
});

test("Player.fromSlot degrades to null when the controller is invalid (offline vm)", () => {
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return false; };   // invalid slot
  const ctx = {
    __s2require: (n) => (n === "@s2script/entity" ? { EntityRef } : null),
    __s2_schema_offset: () => 8, __s2_ent_current_serial: () => -1, __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);
  const { Player } = ctx.__s2pkg_cs2;
  assert.equal(Player.fromSlot(0), null, "invalid controller → null");
  assert.deepEqual(Player.all(), [], "Player.all() is empty when no controller is valid");
});
```

- [ ] **Step 2: Run to verify failure** — `cd packages/cli && node --experimental-strip-types --no-warnings --test test/schema-runtime.test.mjs` → FAIL (`ctx.__s2pkg_cs2.Player` is `undefined` — pawn.js doesn't define `Player` yet).

- [ ] **Step 3: Add `Player` + `Pawn.controller` to `games/cs2/js/pawn.js`.** Inside the existing IIFE, AFTER `if (schema) schema.applyAccessors(Pawn.prototype, "CCSPlayerPawn");` and BEFORE the final `globalThis.__s2pkg_cs2 = …`, insert:

```js
  // --- Slice 5C.2: the Player (controller) model ---
  function Player(ref) { this.ref = ref; }                       // ref = the CONTROLLER EntityRef
  if (schema) schema.applyAccessors(Player.prototype, "CCSPlayerController");  // team, score, ping, ...

  // slot is 0-based (CPlayerSlot); the controller entity index is slot+1.
  Object.defineProperty(Player.prototype, "slot", {
    get: function () { return this.ref.index - 1; }, enumerable: true, configurable: true,
  });

  // player.pawn -> the typed body via m_hPlayerPawn (shadows the raw generated `pawn` = m_hPawn).
  Object.defineProperty(Player.prototype, "pawn", {
    get: function () {
      var off = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");
      if (off < 0) return null;
      var h = this.ref.readHandle(off);
      return h ? new Pawn(h) : null;
    }, enumerable: true, configurable: true,
  });

  var MAX_PLAYERS = 64;
  Player.fromSlot = function (slot) {
    var idx = slot + 1;                                          // controller entity index
    var ref = new EntityRef(idx, __s2_ent_current_serial(idx));
    return ref.isValid() ? new Player(ref) : null;
  };
  Player.all = function () {
    var out = [];
    for (var s = 0; s < MAX_PLAYERS; s++) { var p = Player.fromSlot(s); if (p) out.push(p); }
    return out;
  };

  // pawn.controller -> the typed Player via m_hController (shadows the raw generated `controller`).
  Object.defineProperty(Pawn.prototype, "controller", {
    get: function () {
      var off = __s2_schema_offset("CBasePlayerPawn", "m_hController");
      if (off < 0) return null;
      var h = this.ref.readHandle(off);
      return h ? new Player(h) : null;
    }, enumerable: true, configurable: true,
  });
```

And change the export line to include `Player`:

```js
  globalThis.__s2pkg_cs2 = { Pawn: Pawn, Player: Player };
```

- [ ] **Step 4: Run to verify pass** — same command → PASS (both new tests + the pre-existing pawn/schema-runtime test).

- [ ] **Step 5: Add the types** to `packages/cs2/index.d.ts`. Import `CCSPlayerController`, add the `Player` interface/const, and add `controller` to `Pawn` — both using `Omit` to re-type the shadowed generated handle fields:

```ts
import type { EntityRef } from "@s2script/entity";
export * from "./schema.generated";
import type { CCSPlayerPawn, CCSPlayerController } from "./schema.generated";

/**
 * A CS2 player pawn (the in-world body): the generated CCSPlayerPawn schema fields + the serial-gated ref.
 * `controller` is the typed reverse hop (shadows the raw generated m_hController handle).
 */
export interface Pawn extends Omit<CCSPlayerPawn, "controller"> {
  readonly ref: EntityRef;
  /** The player controlling this pawn, or null if stale/absent. */
  readonly controller: Player | null;
}
export declare const Pawn: {
  /** The Pawn for a player slot, or null if unoccupied / invalidated. */
  forSlot(slot: number): Pawn | null;
};

/**
 * A CS2 player (the persistent controller entity): the generated CCSPlayerController schema fields
 * (team/score/ping/…) + the serial-gated controller ref. `pawn` is the typed body (shadows the raw
 * generated m_hPawn handle). Referenced by slot (0-based); a stored Player degrades to null on reuse.
 */
export interface Player extends Omit<CCSPlayerController, "pawn"> {
  readonly ref: EntityRef;
  /** The 0-based player slot (CPlayerSlot). */
  readonly slot: number;
  /** This player's in-world pawn (the body), or null if dead/absent. */
  readonly pawn: Pawn | null;
}
export declare const Player: {
  /** The Player for a 0-based slot, or null if the slot is unoccupied / the controller is stale. */
  fromSlot(slot: number): Player | null;
  /** Every connected player (slots with a valid controller). */
  all(): Player[];
};
```

- [ ] **Step 6: Full verification + commit**

```bash
cd /home/gkh/projects/s2script/packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs   # all green
cd /home/gkh/projects/s2script
bash scripts/check-schema-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add games/cs2/js/pawn.js packages/cs2/index.d.ts packages/cli/test/schema-runtime.test.mjs
git commit -m "feat(slice5c2): Player (controller) model — fromSlot/all + .pawn nav; Pawn.controller reverse hop

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: Demo + live gate + README/CLAUDE (LIVE-ONLY, controller-driven)

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts`, `README.md`, `CLAUDE.md`

**Interfaces:**
- Consumes: the Task-1 `Player` (`fromSlot`/`all`/`slot`/`pawn`), the generated `player.teamNum`, `pawn.controller`.

**No sniper rebuild** — 5C.2 changed no Rust/core/shim. The live gate needs only the demo `.s2sp` + the repackaged addon JS.

- [ ] **Step 1: Update the demo** `examples/demo-plugin/src/plugin.ts` to exercise the Player model. Sketch:

```ts
import { OnGameFrame } from "@s2script/frame";
import { Player } from "@s2script/cs2";

let ticks = 0;
export function onLoad(): void {
  console.log("[demo] onLoad (player model)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    const players = Player.all();                       // every connected player (controllers)
    console.log("[demo] tick " + ticks + " players=" + players.length);
    for (const p of players) {
      const body = p.pawn;                              // controller -> pawn navigation
      const back = body ? body.controller : null;       // pawn -> controller round-trip
      console.log("  slot=" + p.slot
        + " teamNum=" + p.teamNum                          // generated CCSPlayerController accessor
        + " health=" + (body ? body.health : "none")    // .pawn -> generated CCSPlayerPawn accessor
        + " backSlot=" + (back ? back.slot : "null"));   // reverse hop resolves to the same slot
    }
  });
}
export function onUnload(): void { console.log("[demo] onUnload"); }
```

- [ ] **Step 2: Build the demo `.s2sp` + repackage the addon JS.**

```bash
cd /home/gkh/projects/s2script
node packages/cli/dist/cli.js build examples/demo-plugin      # imports @s2script/frame + @s2script/cs2
cat games/cs2/js/schema.generated.js games/cs2/js/pawn.js > dist/addons/s2script/js/pawn.js   # concat (schema first)
mkdir -p dist/addons/s2script/plugins
cp examples/demo-plugin/dist/_demo_hello.s2sp dist/addons/s2script/plugins/
```
Confirm the packaged `pawn.js` ends with `__s2pkg_cs2 = { Pawn: Pawn, Player: Player }`.

- [ ] **Step 3: Run the live gate on Docker CS2.** Restart the container to reload the injected addon JS (no rebuild — core/shim unchanged); arm `python3 scripts/rcon.py "sv_hibernate_when_empty 0" "bot_quota 1"`; wait past the boot window. Expect:
  - `[demo] tick … players=1` and `slot=0 teamNum=<2|3> health=100 backSlot=0` — `Player.all()` finds the bot, `player.teamNum` reads, `player.pawn` → `health`, and `pawn.controller` round-trips back to the same slot.
  - `bot_kick` → `players=0` (iteration drops the slot); if a player lingers a frame, its reads go `null`; server ticking, no crash.
  Capture the log. If the live infra won't cooperate after reasonable attempts, get the non-live deliverables done and report BLOCKED with the exact commands/errors.

- [ ] **Step 4: README + CLAUDE.**
  - `README.md`: add a `## The player model (Slice 5C.2)` section — the controller/pawn split, `Player.fromSlot`/`all` + `player.pawn` ↔ `pawn.controller`, that `Player` is slot-referenced + serial-gated (safe stored refs), and the captured live log. Note userId/name/steamId are the engine-identity follow.
  - `CLAUDE.md` "## Current state": Slice 5C.2 done (the player model — `Player` controller abstraction + `.pawn`/`.controller` nav + `fromSlot`/`all`, JS-only); "Current focus" → the engine-identity follow (userId/name/steamId via S2EngineOps) or 5C.3 (std breadth). Do NOT alter the standing conventions.

- [ ] **Step 5: Final verification + commit** (no build artifacts):

```bash
cd /home/gkh/projects/s2script/packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-schema-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add examples/demo-plugin/src/plugin.ts README.md CLAUDE.md
git commit -m "feat(slice5c2): live gate PASSED — demo iterates Player.all() + nav; README + CLAUDE

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Acceptance (spec §8)

1. `cargo test -p s2script-core` green (unchanged — no core touch); the CLI `node:test` suite green (+ the new Player vm tests); both boundary gates green; `check-schema-generated.sh` green.
2. `s2script build` produces the demo `.s2sp` using `Player`.
3. Live gate: `Player.all()` iterates players, `player.teamNum`/`player.pawn.health` read, `pawn.controller` round-trips, all `null` on death/disconnect, no crash — with no sniper rebuild.
4. README documents the `Player` model; CLAUDE.md "Current state" updated (5C.2 done).
