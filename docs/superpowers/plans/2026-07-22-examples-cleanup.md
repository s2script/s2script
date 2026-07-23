# examples/ cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse `examples/` from 39 directories to 6 curated teaching examples plus 3 relocated tools, add a monorepo example, and add a coverage gate so the corpus cannot rot.

**Architecture:** Three flagship examples teach structure (`hello-plugin`, `entity-playground`, `monorepo-plugin`); a `greeter-plugin`/`greeter-consumer` pair teaches the runtime cross-plugin contract; one `cookbook` plugin holds the long tail as one file per API, collapsing ~25 single-module demos into one package. Dev tooling that was never an example (`schema-dump`, `s2bench`, `crash-test`) moves to `tools/`. The typecheck gate is widened to `tools/*/` so no coverage is lost, and a new gate fails the build if any shipped SDK module has no consumer anywhere.

**Tech Stack:** TypeScript (pure ESM plugins), `@s2script/sdk` (`s2s` CLI, esbuild + tsc), `node:test` for SDK tests, bash gate scripts.

**Spec:** `docs/superpowers/specs/2026-07-22-examples-cleanup-design.md`

## Global Constraints

- **Plugins are pure ESM.** `module: ESNext`, no `require`. Explicit `.ts` import extensions are permitted (`allowImportingTsExtensions: true`) and are what relative intra-plugin imports use.
- **Every example and tool must pass `./scripts/check-plugins-typecheck.sh`** — full strict tsc against the shipped `.d.ts`. This is the binding gate for every task.
- **Core boundary is untouched.** No task modifies `core/` or `shim/`. `make check-boundary` must stay green.
- **One branch, one PR** — per `docs/superpowers/specs/2026-07-22-ci-consolidation-design.md` §3 (Graphite stacking retired). Branch: `examples/cleanup`, off `main`, in a dedicated worktree.
- **Commit after every task.** Do not batch commits across tasks.
- **Historical docs are not updated.** Files under `docs/superpowers/plans/` and `docs/superpowers/specs/` that reference deleted directories are a record of what was true then; leave them alone.
- **Twelve modules are covered only by `examples/`** and must retain a consumer: `entity`, `http`, `ws`, `net`, `sound`, `trace`, `transmit`, `translations`, `usercmd`, `usermessages`, `cookies`, plus the `@s2script/zones` plugin interface.
- **Command naming:** cookbook recipes register commands prefixed `cb_`. `ctx.commands.register(name, …)` uses the name verbatim — there is no automatic `sm_` prefix.

---

## Task 0: Worktree setup

**Files:** none (environment only)

- [ ] **Step 1: Create the worktree off `main`**

The current branch is `ci/consolidation` — unrelated in-flight work. Do NOT build on it.

```bash
cd /home/gkh/projects/s2script
git fetch origin
git worktree add -b examples/cleanup ../s2script-examples-cleanup origin/main
cd ../s2script-examples-cleanup
```

- [ ] **Step 2: Install dependencies**

```bash
npm ci
```

Expected: `added N packages`. `node_modules/` is gitignored.

- [ ] **Step 3: Establish the green baseline**

```bash
./scripts/check-plugins-typecheck.sh
```

Expected: ends with `PASS: all plugins and examples typecheck`. If this fails before any edit, HALT and report — the baseline is broken and nothing below is trustworthy.

---

## Task 1: CLI `mainFields` fix

Workspace-sibling packages that declare `main` fail to bundle because `platform: "neutral"` defaults esbuild's `mainFields` to empty. Task 8's monorepo example works around this with `exports`, but authors reflexively write `main`, and the failure message is undiagnosable. One-line fix, real regression test.

**Files:**
- Create: `packages/sdk/test/fixtures/workspace-sibling/package.json`
- Create: `packages/sdk/test/fixtures/workspace-sibling/src/plugin.ts`
- Create: `packages/sdk/test/fixtures/workspace-sibling/packages/util/package.json`
- Create: `packages/sdk/test/fixtures/workspace-sibling/packages/util/src/index.ts`
- Create: `packages/sdk/test/fixtures/workspace-sibling/node_modules/@fixture/util` (symlink)
- Modify: `packages/sdk/src/build.ts` (the `esbuild.build({...})` call)
- Test: `packages/sdk/test/build.test.mjs`

**Interfaces:**
- Consumes: `buildPlugin(dir, packagesDir)` from `packages/sdk/src/build.ts`
- Produces: nothing new — behaviour change only. Task 8 depends on workspace-sibling resolution working.

- [ ] **Step 1: Create the fixture plugin**

`packages/sdk/test/fixtures/workspace-sibling/package.json` — note the sibling declares `main`, which is the case being fixed:

```json
{
  "name": "@fixture/workspace-sibling",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "workspaces": ["packages/*"]
}
```

`packages/sdk/test/fixtures/workspace-sibling/src/plugin.ts`:

```ts
import { plugin } from "@s2script/sdk/plugin";
import { label } from "@fixture/util";

export default plugin((ctx) => {
  ctx.commands.register("fixture_ws", (cmd) => { cmd.reply(label()); });
});
```

`packages/sdk/test/fixtures/workspace-sibling/packages/util/package.json` — `main`, deliberately NOT `exports`:

```json
{
  "name": "@fixture/util",
  "version": "0.1.0",
  "main": "src/index.ts"
}
```

`packages/sdk/test/fixtures/workspace-sibling/packages/util/src/index.ts`:

```ts
export function label(): string {
  return "workspace-sibling-resolved";
}
```

- [ ] **Step 2: Create the node_modules symlink npm workspaces would create**

```bash
cd packages/sdk/test/fixtures/workspace-sibling
mkdir -p node_modules/@fixture
ln -s ../../packages/util node_modules/@fixture/util
cd -
```

Verify the symlink resolves:

```bash
cat packages/sdk/test/fixtures/workspace-sibling/node_modules/@fixture/util/package.json
```

Expected: prints the `@fixture/util` manifest.

- [ ] **Step 3: Write the failing test**

Append to `packages/sdk/test/build.test.mjs`:

```js
test("bundles a workspace sibling that declares `main` (platform:neutral mainFields)", async () => {
  const out = await buildPlugin(join(here, "fixtures", "workspace-sibling"), packagesDir);
  const zip = new AdmZip(out);
  const js = zip.readAsText("plugin.js");
  assert.ok(
    js.includes("workspace-sibling-resolved"),
    "the sibling package's source must be inlined into the bundle"
  );
  assert.ok(
    !js.includes('require("@fixture/util")'),
    "the sibling must be bundled, not left as an external require"
  );
});
```

- [ ] **Step 4: Run it to confirm it fails**

```bash
npm test --workspace @s2script/sdk 2>&1 | grep -A6 "workspace sibling"
```

Expected: FAIL, with esbuild reporting `Could not resolve "@fixture/util"` and `The "main" field here was ignored. Main fields must be configured explicitly when using the "neutral" platform.`

- [ ] **Step 5: Apply the one-line fix**

In `packages/sdk/src/build.ts`, the `esbuild.build` call currently reads:

```ts
  const result = await esbuild.build({
    entryPoints: [entryPoint],
    bundle: true,
    platform: "neutral",
    format: "cjs",
    external,
    target: "es2020",
    write: false,
  });
```

Add `mainFields` with an explanatory comment:

```ts
  const result = await esbuild.build({
    entryPoints: [entryPoint],
    bundle: true,
    platform: "neutral",
    format: "cjs",
    external,
    target: "es2020",
    // platform:"neutral" defaults mainFields to EMPTY, so a workspace-sibling package that
    // declares `main` (rather than `exports`) fails to resolve with an undiagnosable error.
    // Set it explicitly so a monorepo plugin bundles whichever field the author wrote.
    mainFields: ["module", "main"],
    write: false,
  });
```

- [ ] **Step 6: Run the test to confirm it passes**

```bash
npm test --workspace @s2script/sdk 2>&1 | tail -20
```

Expected: all tests pass, including the new one. Confirm no pre-existing test regressed — the summary line must show `fail 0`.

- [ ] **Step 7: Commit**

```bash
git add packages/sdk/src/build.ts packages/sdk/test/build.test.mjs packages/sdk/test/fixtures/workspace-sibling
git commit -m "fix(sdk): resolve workspace-sibling packages that declare \`main\`

platform:\"neutral\" defaults esbuild mainFields to empty, so a monorepo
plugin importing a sibling by bare specifier failed with an undiagnosable
error. Set mainFields explicitly; regression test covers the \`main\` case
(\`exports\` already worked)."
```

---

## Task 2: Relocate tools out of examples/

`schema-dump`, `s2bench`, and `crash-test` are dev/treadmill tooling, not teaching examples. Move them and widen the typecheck glob so no coverage is lost.

**Files:**
- Move: `examples/schema-dump/` → `tools/schema-dump/`
- Move: `examples/s2bench/` → `tools/s2bench/`
- Move: `examples/crash-test/` → `tools/crash-test/`
- Modify: `tools/*/tsconfig.json` (relative depth is unchanged — verify)
- Modify: `scripts/check-plugins-typecheck.sh`

**Interfaces:**
- Consumes: nothing
- Produces: `tools/` directory, included in the typecheck gate. Task 9's coverage gate scans `tools/` too.

- [ ] **Step 1: Move the three directories**

```bash
mkdir -p tools
git mv examples/schema-dump tools/schema-dump
git mv examples/s2bench tools/s2bench
git mv examples/crash-test tools/crash-test
```

- [ ] **Step 2: Verify tsconfig relative depth still resolves**

Each moved `tsconfig.json` extends `../../tsconfig.base.json`. `tools/<name>/` is the same depth as `examples/<name>/`, so the path is unchanged. Confirm:

```bash
cat tools/s2bench/tsconfig.json
```

Expected: `"extends": "../../tsconfig.base.json"`. If a moved tool has no `tsconfig.json`, create one:

```json
{
  "extends": "../../tsconfig.base.json",
  "include": ["src", "../../packages/sdk/globals.d.ts"]
}
```

- [ ] **Step 3: Widen the typecheck glob**

In `scripts/check-plugins-typecheck.sh`, the loop currently reads:

```bash
for d in examples/*/ plugins/*/ plugins/disabled/*/; do
```

Change it to include `tools/*/`:

```bash
for d in examples/*/ plugins/*/ plugins/disabled/*/ tools/*/; do
```

Also update the file's header comment, which currently says "Typecheck every example and plugin against the shipped engine .d.ts (the Slice-5E.1 gate)":

```bash
# Typecheck every example, plugin, and dev tool against the shipped engine .d.ts (the Slice-5E.1 gate).
# Fails if any has a type error — a .d.ts regression that breaks them is caught here.
```

- [ ] **Step 4: Run the gate**

```bash
./scripts/check-plugins-typecheck.sh 2>&1 | tail -20
```

Expected: `PASS: all plugins and examples typecheck`, and the output includes `=== typecheck tools/schema-dump/ ===`, `tools/s2bench/`, `tools/crash-test/`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(examples): relocate dev tooling to tools/

schema-dump (gamedata regeneration after a CS2 update), s2bench (op-timing
benchmark), and crash-test (deliberate-crash harness) were never examples.
Move them to tools/ and widen the typecheck gate glob so coverage is
unchanged."
```

---

## Task 3: hello-plugin

Replace `demo-plugin` with a first-plugin example. The current one is a Slice-5E.3 live-gate rig whose comments cite reload-handoff internals; it teaches the reader about a test, not about writing a plugin. Keep the hot-reload handoff (it is genuinely a headline feature) but present it as a feature, not as a gate.

**Files:**
- Create: `examples/hello-plugin/package.json`
- Create: `examples/hello-plugin/tsconfig.json`
- Create: `examples/hello-plugin/src/plugin.ts`
- Delete: `examples/demo-plugin/`

**Interfaces:**
- Consumes: `plugin`, `PluginContext` from `@s2script/sdk/plugin`; `Player` from `@s2script/cs2`
- Produces: nothing consumed by later tasks

- [ ] **Step 1: Create the package manifest**

`examples/hello-plugin/package.json`:

```json
{
  "name": "@example/hello-plugin",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "dependencies": {
    "@s2script/sdk": "^0.1.0",
    "@s2script/cs2": "^0.5.0"
  }
}
```

- [ ] **Step 2: Create the tsconfig**

`examples/hello-plugin/tsconfig.json`:

```json
{
  "extends": "../../tsconfig.base.json",
  "include": ["src", "../../packages/sdk/globals.d.ts"]
}
```

- [ ] **Step 3: Write the plugin**

`examples/hello-plugin/src/plugin.ts`:

```ts
// hello-plugin — the smallest complete s2script plugin. Start here.
//
// It shows the four things every plugin does:
//   1. define itself with plugin((ctx) => …)   — the factory runs once at load
//   2. register a command                      — ctx.commands.register
//   3. subscribe to a game event               — ctx.events.on
//   4. survive a hot reload                    — return { state, onUnload }
//
// Build it:   npx s2s build examples/hello-plugin
// Then drop dist/*.s2sp into addons/s2script/plugins/ on a running server.
import { plugin } from "@s2script/sdk/plugin";
import { Player } from "@s2script/cs2";

// State that survives a hot reload. Edit this file on a running server and the
// host hands `state()`'s return value to the next instance as `ctx.previous`.
interface State { greeted: number; }

export default plugin((ctx) => {
  // ctx.previous is undefined on a first load, and the previous instance's
  // state() return on a reload.
  const prev = ctx.previous as State | undefined;
  let greeted = prev?.greeted ?? 0;

  console.log(`[hello] loaded (greeted so far: ${greeted})`);

  // A command any client can run, from chat or console.
  ctx.commands.register("hello", (cmd) => {
    cmd.reply(`hello! I have greeted ${greeted} spawns since first load.`);
  });

  // A game event. The GameEvent is only valid synchronously — read what you
  // need inside the handler, never stash it.
  ctx.events.on("player_spawn", (ev) => {
    greeted += 1;
    const player = Player.fromSlot(ev.getPlayerSlot("userid"));
    console.log(`[hello] spawn #${greeted}: ${player?.playerName ?? "unknown"}`);
  });

  return {
    // Best-effort cleanup. The ledger is the real teardown authority — you do
    // not have to unregister what you registered through ctx.
    onUnload() {
      console.log(`[hello] unloading after ${greeted} greetings`);
    },
    // Handed to the next instance as ctx.previous. Serialized as JSON
    // (EntityRef-aware), so no BigInt — carry 64-bit values as strings.
    state(): State {
      return { greeted };
    },
  };
});
```

- [ ] **Step 4: Delete the old demo-plugin**

```bash
git rm -r examples/demo-plugin
```

- [ ] **Step 5: Typecheck**

```bash
./scripts/check-plugins-typecheck.sh 2>&1 | grep -E "hello-plugin|PASS|FAIL"
```

Expected: `=== typecheck examples/hello-plugin/ ===` followed by `OK`, and a final `PASS`.

- [ ] **Step 6: Build it to a .s2sp**

```bash
node --experimental-strip-types --no-warnings -e "
import('./packages/sdk/src/build.ts').then(({buildPlugin}) =>
  buildPlugin('examples/hello-plugin', 'packages')).then(p => console.log('OK', p))
"
```

Expected: `OK …/examples/hello-plugin/dist/_example_hello-plugin.s2sp`

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(examples): hello-plugin replaces demo-plugin

demo-plugin was a Slice-5E.3 reload-handoff live-gate rig whose comments
described the test, not the API. hello-plugin teaches the four things every
plugin does — factory, command, event, reload handoff — as features."
```

---

## Task 4: entity-playground

Merge the five entity demos into one example. `entity` is the largest module in the SDK and the one most people need; it deserves a real example rather than five 40-line fragments.

**Files:**
- Create: `examples/entity-playground/package.json`
- Create: `examples/entity-playground/tsconfig.json`
- Create: `examples/entity-playground/src/plugin.ts`
- Delete: `examples/beam-demo/`, `examples/ekv-demo/`, `examples/entityio-demo/`, `examples/entity-listeners-demo/`, `examples/entity-name-demo/`

**Interfaces:**
- Consumes: `createEntity`, `Entity` from `@s2script/sdk/entity`; `Pawn`, `Beam`, `BeamHandle` from `@s2script/cs2`; `Vector` from `@s2script/sdk/math`; `delay` from `@s2script/sdk/timers`; `Server` from `@s2script/sdk/server`
- Produces: nothing consumed by later tasks

**Note on scope:** the spec (§7) flags that this may want splitting if it exceeds ~150 lines. Write it first, then judge. If it lands over ~180 lines, split the lifecycle-listener half into `examples/entity-lifecycle/` and say so in the task report — do not silently ship a 250-line "example".

- [ ] **Step 1: Create the package manifest**

`examples/entity-playground/package.json`:

```json
{
  "name": "@example/entity-playground",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "dependencies": {
    "@s2script/sdk": "^0.1.0",
    "@s2script/cs2": "^0.5.0"
  }
}
```

- [ ] **Step 2: Create the tsconfig**

`examples/entity-playground/tsconfig.json`:

```json
{
  "extends": "../../tsconfig.base.json",
  "include": ["src", "../../packages/sdk/globals.d.ts"]
}
```

- [ ] **Step 3: Write the plugin**

`examples/entity-playground/src/plugin.ts`. This is a rewrite, not a concatenation: the source demos carry live-gate commentary ("bot-provable", "no human client needed", "Task 1 wired …") that must be dropped, and the `globalThis as any` offset/timer escapes in `ekv-demo` must be replaced with the real imports.

```ts
// entity-playground — creating, configuring, wiring, and watching entities.
//
// Commands (run from rcon or console):
//   ent_create    spawn an entity, read its fields back, then remove it
//   ent_kv        spawn entities configured by keyvalues, proven two ways
//   ent_io        fire an input and catch the output it produces
//   ent_names     list every trigger_multiple on the map by targetname
//   ent_beam      draw a beam between two points for 3 seconds
//
// Everything an entity API hands you is an EntityRef — a serial-gated handle,
// never a raw pointer. Reads return `T | null`: if the entity died, you get
// null, not garbage and not a crash. Hold refs across time freely.
import { plugin } from "@s2script/sdk/plugin";
import { createEntity, Entity } from "@s2script/sdk/entity";
import { Server } from "@s2script/sdk/server";
import { Beam } from "@s2script/cs2";
import { Vector } from "@s2script/sdk/math";
import { delay } from "@s2script/sdk/timers";

// Schema offsets are resolved live from the engine's SchemaSystem — never
// hardcoded. A field moving in a CS2 patch must not require a code change.
declare const __s2_schema_offset: (cls: string, field: string) => number;

export default plugin((ctx) => {
  // --- Lifecycle listeners -------------------------------------------------
  // The useful case is reacting to ENGINE-driven lifecycle: map entities,
  // weapons, grenades, ragdolls. `entity` is a serial-gated EntityRef and may
  // be null for a barely-constructed create or a dying delete; `className` is
  // always valid. Counters keep a map-load burst readable.
  let created = 0, spawned = 0, deleted = 0;
  ctx.entities.onCreate("*", (_e, cls) => { if (++created <= 10) console.log(`[ent] created ${cls}`); });
  ctx.entities.onSpawn("*", (e, cls) => { if (++spawned <= 10) console.log(`[ent] spawned ${cls} valid=${!!e?.isValid()}`); });
  ctx.entities.onDelete("*", (_e, cls) => { if (++deleted <= 10) console.log(`[ent] deleted ${cls}`); });

  // Hook a named output on a class. Return a HookResult to suppress it.
  ctx.entities.onOutput("logic_relay", "OnTrigger", (ev) => {
    console.log(`[ent] OnTrigger caller=${ev.caller ? `valid=${ev.caller.isValid()}` : "null"}`);
  });
  ctx.entities.onOutput("math_counter", "OnHitMax", () => {
    console.log("[ent] OnHitMax — the counter reached the max its keyvalues set");
  });

  // --- Create, read back, remove -------------------------------------------
  ctx.commands.register("ent_create", (cmd) => {
    const text = createEntity("point_worldtext");
    if (!text) { cmd.reply("createEntity failed"); return; }
    text.spawn();
    text.teleport(new Vector(0, 0, 100));
    cmd.reply(`created point_worldtext #${text.index} valid=${text.isValid()}`);
    delay(3000).then(() => cmd.reply(`removed -> ${text.remove()}`));
  });

  // --- Keyvalue-configured spawn -------------------------------------------
  // createEntity(className, keyvalues) builds a CEntityKeyValues and dispatches
  // the spawn with it, so the entity's OWN Spawn() parses the keys. Proven two
  // ways: read the parsed fields back through the schema, and let an int
  // keyvalue drive the entity's own logic until it fires an output.
  ctx.commands.register("ent_kv", (cmd) => {
    const text = createEntity("point_worldtext", { message: "configured-by-keyvalues", enabled: true, fullbright: true });
    if (text) {
      const msg = text.readString(__s2_schema_offset("CPointWorldText", "m_messageText"), 512);
      const fullbright = text.readBool(__s2_schema_offset("CPointWorldText", "m_bFullbright"));
      cmd.reply(`worldtext message=${JSON.stringify(msg)} fullbright=${fullbright}`);
    }

    const counter = createEntity("math_counter", { startvalue: 5, min: 1, max: 10 });
    if (counter) {
      const max = counter.readFloat32(__s2_schema_offset("CMathCounter", "m_flMax"));
      cmd.reply(`counter max=${max}; adding 5 to its start of 5 -> expect OnHitMax`);
      counter.acceptInput("Add", "5");
    }

    delay(3000).then(() => { text?.remove(); counter?.remove(); });
  });

  // --- Entity I/O ----------------------------------------------------------
  // acceptInput queues an I/O event that the game's own pump routes to the
  // entity's outputs, which our onOutput subscriber above catches next tick.
  // Passing activator/caller gives the output hook live EntityRefs to report.
  ctx.commands.register("ent_io", (cmd) => {
    const relay = createEntity("logic_relay");
    if (!relay) { cmd.reply("createEntity failed"); return; }
    relay.spawn();
    const ok = relay.acceptInput("Trigger", "", relay, relay);
    cmd.reply(`fired Trigger ok=${ok} — watch the log for the output next tick`);
  });

  // --- Finding entities ----------------------------------------------------
  // EntityRef.name reads CEntityIdentity::m_name (the map's targetname).
  ctx.commands.register("ent_names", (cmd) => {
    const triggers = Entity.findByClass("trigger_multiple");
    cmd.reply(`${triggers.length} trigger_multiple on ${Server.mapName}`);
    for (const t of triggers) console.log(`[ent]   #${t.index} name=${JSON.stringify(t.name)}`);
  });

  // --- Beams ---------------------------------------------------------------
  ctx.commands.register("ent_beam", (cmd) => {
    const handle = Beam.draw(new Vector(0, 0, 100), new Vector(200, 0, 100), { color: [0, 255, 0, 255], width: 3 });
    if (!handle) { cmd.reply("beam failed"); return; }
    cmd.reply(`beam drawn ref valid=${handle.ref.isValid()}`);
    delay(3000).then(() => cmd.reply(`beam removed -> ${handle.remove()}`));
  });

  console.log("[ent] entity-playground loaded — try ent_create, ent_kv, ent_io, ent_names, ent_beam");
});
```

- [ ] **Step 4: Delete the five absorbed demos**

```bash
git rm -r examples/beam-demo examples/ekv-demo examples/entityio-demo \
          examples/entity-listeners-demo examples/entity-name-demo
```

- [ ] **Step 5: Typecheck**

```bash
./scripts/check-plugins-typecheck.sh 2>&1 | grep -E "entity-playground|PASS|FAIL"
```

Expected: `OK` for `examples/entity-playground/`, final `PASS`.

If `__s2_schema_offset` is rejected: it is an internal native, not part of the typed surface. The `declare const` above is the correct escape hatch (`tools/s2bench/src/plugin.ts` uses the same pattern). If `Beam`/`BeamHandle` types have drifted, read `packages/cs2/index.d.ts` for the current signature and adjust — do not delete the beam section.

- [ ] **Step 6: Check the line count against the split threshold**

```bash
wc -l examples/entity-playground/src/plugin.ts
```

If over 180, split the lifecycle-listener block into `examples/entity-lifecycle/` (own package.json + tsconfig, same pattern as Step 1–2) and note it in the task report.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(examples): entity-playground merges five entity demos

beam, ekv, entityio, entity-listeners, and entity-name were five fragments
of one subject. One example teaches create/spawn/keyvalues/IO/outputs/
lifecycle/find, with the live-gate commentary and the globalThis-as-any
offset escapes removed."
```

---

## Task 5: cookbook scaffold + first three recipes

Establish the recipe contract and prove it with three recipes before converting the rest. Do not convert all ~20 in one task — a broken contract discovered at recipe 18 is expensive.

**Files:**
- Create: `examples/cookbook/package.json`
- Create: `examples/cookbook/tsconfig.json`
- Create: `examples/cookbook/src/recipe.ts`
- Create: `examples/cookbook/src/plugin.ts`
- Create: `examples/cookbook/src/recipes/index.ts`
- Create: `examples/cookbook/src/recipes/http.ts`
- Create: `examples/cookbook/src/recipes/sound.ts`
- Create: `examples/cookbook/src/recipes/ws.ts`
- Delete: `examples/http-demo/`, `examples/sound-demo/`, `examples/ws-demo/`

**Interfaces:**
- Consumes: `PluginContext` from `@s2script/sdk/plugin`
- Produces: **the `Recipe` contract**, which every recipe in Task 6 implements:
  ```ts
  export interface Recipe {
    readonly name: string;
    readonly describe: string;
    register(ctx: PluginContext): void;
  }
  ```
  and `RECIPES: readonly Recipe[]` exported from `src/recipes/index.ts`.

- [ ] **Step 1: Create the package manifest**

`examples/cookbook/package.json`. Dependencies cover every module the recipes import across Tasks 5 and 6:

```json
{
  "name": "@example/cookbook",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "dependencies": {
    "@s2script/sdk": "^0.1.0",
    "@s2script/cs2": "^0.5.0"
  },
  "s2script": {
    "optionalPluginDependencies": {
      "@s2script/zones": "^0.3.0"
    }
  }
}
```

`@s2script/zones` is a plugin-published interface, not an SDK module, and is **optional** so the cookbook loads standalone. See Task 6 Step 4.

- [ ] **Step 2: Create the tsconfig**

`examples/cookbook/tsconfig.json`:

```json
{
  "extends": "../../tsconfig.base.json",
  "include": ["src", "../../packages/sdk/globals.d.ts"]
}
```

- [ ] **Step 3: Define the recipe contract**

`examples/cookbook/src/recipe.ts`:

```ts
import type { PluginContext } from "@s2script/sdk/plugin";

/**
 * One cookbook recipe: a self-contained demonstration of a single API,
 * registered under the cookbook's shared plugin context.
 *
 * Recipes must be side-effect-light at registration — register commands and
 * subscriptions, do not start work. Commands are prefixed `cb_` so the whole
 * cookbook is greppable in a console autocomplete.
 */
export interface Recipe {
  /** Short id, matching the file name (e.g. "http"). */
  readonly name: string;
  /** One line shown by `cb_list`. */
  readonly describe: string;
  /** Register this recipe's commands and subscriptions. */
  register(ctx: PluginContext): void;
}
```

- [ ] **Step 4: Write the first three recipes**

Each is the body of the corresponding demo, lifted out of its `plugin((ctx) => …)` wrapper and given the `Recipe` shape. Read the source file before converting — the bodies below are the current content, but if a demo has drifted, the demo is the source of truth for behaviour.

`examples/cookbook/src/recipes/http.ts` (from `examples/http-demo/src/plugin.ts`):

```ts
import type { Recipe } from "../recipe.ts";
import { fetch } from "@s2script/sdk/http";

/**
 * fetch() runs off-thread on the shared tokio runtime and resolves back on a
 * game frame, so awaiting it never blocks the server. Bodies are capped at 10MB.
 */
export const httpRecipe: Recipe = {
  name: "http",
  describe: "fetch() an HTTP endpoint without blocking the server",
  register(ctx) {
    ctx.commands.register("cb_http", (cmd) => {
      cmd.reply("fetching…");
      fetch("https://api.github.com/repos/s2script/s2script")
        .then((res) => res.json())
        .then((body: unknown) => {
          const repo = body as { stargazers_count?: number };
          cmd.reply(`ok — stars: ${repo.stargazers_count ?? "?"}`);
        })
        .catch((e: unknown) => cmd.reply(`failed: ${String(e)}`));
    });
  },
};
```

`examples/cookbook/src/recipes/sound.ts`:

```ts
import type { Recipe } from "../recipe.ts";
import { Sound } from "@s2script/sdk/sound";
import { Pawn, Sounds } from "@s2script/cs2";

/**
 * Sounds must be registered during the precache window — emitting an
 * unprecached sound is silently dropped. Sound.emit broadcasts from the world;
 * pawn.emitSound plays from an entity and can target specific recipients.
 */
export const soundRecipe: Recipe = {
  name: "sound",
  describe: "precache and emit a sound (cb_sound [name] [slot])",
  register(ctx) {
    ctx.server.onPrecache((pc) => {
      const ok = pc.add("soundevents/soundevents_s2script_demo.vsndevts");
      console.log(`[cookbook] precache add() -> ${ok}`);
    });

    ctx.commands.register("cb_sound", (cmd) => {
      const name = cmd.args[0] || Sounds.Ping;
      // With a slot: emit from that slot's pawn, to that slot only.
      if (cmd.args.length > 1) {
        const slot = parseInt(cmd.args[1], 10);
        const pawn = Pawn.forSlot(Number.isNaN(slot) ? -1 : slot);
        if (!pawn) { cmd.reply(`no pawn at slot ${cmd.args[1]}`); return; }
        cmd.reply(`emitSound('${name}') from slot ${slot} -> guid=${pawn.emitSound(name, { recipients: [slot] })}`);
        return;
      }
      // Without: a global broadcast from the world.
      cmd.reply(`Sound.emit('${name}') broadcast -> guid=${Sound.emit(name)}`);
    });
  },
};
```

`examples/cookbook/src/recipes/ws.ts`. **Note a deliberate behaviour change**, the one sanctioned exception to "port verbatim": `ws-demo` uses an async factory that dials the echo service *at load*. The cookbook registers 20 recipes at once, so connecting at load would make merely loading the plugin perform network I/O against a public host. The recipe connects on command instead, which also keeps `register` synchronous:

```ts
import type { Recipe } from "../recipe.ts";
import { WebSocket } from "@s2script/sdk/ws";

/**
 * WebSockets run off-thread on the shared tokio runtime; callbacks are
 * marshalled back onto a game frame. The frame counter proves the tick keeps
 * advancing while the socket connects and echoes.
 */
export const wsRecipe: Recipe = {
  name: "ws",
  describe: "connect a websocket without blocking the tick (cb_ws)",
  register(ctx) {
    let frames = 0;
    ctx.server.onGameFrame(() => { frames += 1; });

    ctx.commands.register("cb_ws", (cmd) => {
      const start = frames;
      cmd.reply("connecting…");
      WebSocket.connect("wss://ws.postman-echo.com/raw")
        .then((ws) => {
          ws.onMessage((data) => {
            console.log(`[cookbook] echo=${data}; tick advanced ${frames - start} frames meanwhile`);
            ws.close();
          });
          ws.onClose((code, reason) => console.log(`[cookbook] ws closed code=${code} reason=${reason}`));
          ws.onError((e) => console.log(`[cookbook] ws error=${e}`));
          ws.send("hello-from-s2script");
          cmd.reply("connected + sent — watch the log for the echo");
        })
        .catch((e: unknown) => cmd.reply(`connect failed: ${String(e)}`));
    });
  },
};
```

- [ ] **Step 5: Write the recipe registry**

`examples/cookbook/src/recipes/index.ts`:

```ts
import type { Recipe } from "../recipe.ts";
import { httpRecipe } from "./http.ts";
import { soundRecipe } from "./sound.ts";
import { wsRecipe } from "./ws.ts";

/** Every recipe the cookbook registers. Add new ones here. */
export const RECIPES: readonly Recipe[] = [
  httpRecipe,
  soundRecipe,
  wsRecipe,
];
```

- [ ] **Step 6: Write the cookbook plugin entry**

`examples/cookbook/src/plugin.ts`:

```ts
// cookbook — one file per API, all registered under a single plugin.
//
// Browse src/recipes/ for the API you want; each file is self-contained and
// readable on its own. Run `cb_list` on a server to see everything registered.
//
// This is a DEMO plugin: it registers a lot of commands and is not part of the
// shipped release. Copy a recipe into your own plugin rather than loading this.
import { plugin } from "@s2script/sdk/plugin";
import { RECIPES } from "./recipes/index.ts";

export default plugin((ctx) => {
  for (const recipe of RECIPES) {
    recipe.register(ctx);
  }

  ctx.commands.register("cb_list", (cmd) => {
    cmd.reply(`${RECIPES.length} recipes:`);
    for (const r of RECIPES) cmd.reply(`  cb_${r.name} — ${r.describe}`);
  });

  console.log(`[cookbook] loaded ${RECIPES.length} recipes — run cb_list`);
});
```

- [ ] **Step 7: Delete the three absorbed demos**

```bash
git rm -r examples/http-demo examples/sound-demo examples/ws-demo
```

- [ ] **Step 8: Typecheck**

```bash
./scripts/check-plugins-typecheck.sh 2>&1 | grep -E "cookbook|PASS|FAIL"
```

Expected: `OK` for `examples/cookbook/`, final `PASS`.

- [ ] **Step 9: Build it**

```bash
node --experimental-strip-types --no-warnings -e "
import('./packages/sdk/src/build.ts').then(({buildPlugin}) =>
  buildPlugin('examples/cookbook', 'packages')).then(p => console.log('OK', p))
"
```

Expected: `OK …/examples/cookbook/dist/_example_cookbook.s2sp`. This proves multi-file relative-`.ts` imports bundle correctly before 20 more recipes depend on it.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat(examples): cookbook scaffold + http/sound/ws recipes

One plugin, one file per API. Establishes the Recipe contract and proves
multi-file relative-.ts imports bundle before the remaining recipes land."
```

---

## Task 6: Remaining cookbook recipes

Convert the rest. The conversion is mechanical — Task 5 fixed the shape — but three merges and the zones recipe need judgement.

**Files:**
- Create: `examples/cookbook/src/recipes/{net,db,cookies,trace,zones,transmit,translations,usercmd,usermessages,menu,items,player-state,team,clients,events,server,gamerules}.ts`
- Create: `examples/cookbook/.s2script/types/@s2script/zones/index.d.ts`
- Modify: `examples/cookbook/src/recipes/index.ts`
- Delete: 19 example directories (listed in Step 6)

**Interfaces:**
- Consumes: `Recipe` from `../recipe.ts`; `RECIPES` array in `src/recipes/index.ts`
- Produces: nothing consumed by later tasks

- [ ] **Step 1: Convert the one-to-one recipes**

For each row: read the source, lift the `plugin((ctx) => { … })` body into a `Recipe` object exactly as Task 5 did, rename its commands to `cb_<name>`, and drop any live-gate commentary ("bot-provable", "no human client needed", "Task N wired…", "Slice N.N"). Keep explanatory comments that teach the API.

| Source | New recipe file | Export const | Command |
|---|---|---|---|
| `examples/net-demo/src/plugin.ts` | `recipes/net.ts` | `netRecipe` | `cb_net` |
| `examples/trace-demo/src/plugin.ts` | `recipes/trace.ts` | `traceRecipe` | `cb_trace` |
| `examples/transmit-demo/src/plugin.ts` | `recipes/transmit.ts` | `transmitRecipe` | `cb_transmit` |
| `examples/translations-demo/src/plugin.ts` | `recipes/translations.ts` | `translationsRecipe` | `cb_translations` |
| `examples/usercmd-demo/src/plugin.ts` | `recipes/usercmd.ts` | `usercmdRecipe` | `cb_usercmd` |
| `examples/usermsg-demo/src/plugin.ts` | `recipes/usermessages.ts` | `usermessagesRecipe` | `cb_usermsg` |
| `examples/menu-demo/src/plugin.ts` | `recipes/menu.ts` | `menuRecipe` | `cb_menu` |
| `examples/clientprefs-demo/src/plugin.ts` | `recipes/cookies.ts` | `cookiesRecipe` | `cb_cookies` |
| `examples/respawn-demo/src/plugin.ts` | `recipes/player-state.ts` | `playerStateRecipe` | `cb_respawn` |
| `examples/round-control-demo/src/plugin.ts` | `recipes/events.ts` | `eventsRecipe` | `cb_round` |
| `examples/clientlist-convar-mapstart-demo/src/plugin.ts` | `recipes/server.ts` | `serverRecipe` | `cb_server` |
| `examples/gamerules-usermsg-demo/src/plugin.ts` | `recipes/gamerules.ts` | `gamerulesRecipe` | `cb_gamerules` |

- [ ] **Step 2: Convert the three merges**

Each merge is one recipe file registering both source demos' commands, with a comment naming the distinction.

**`recipes/db.ts`** (`dbRecipe`) from `db-demo` + `db-remote-demo`: `cb_db` runs the SQLite query, `cb_db_remote` runs the MySQL/Postgres one. Lead comment: the same `@s2script/db` API drives all three backends; only the connection config differs, and every call is off-thread behind a Promise.

**`recipes/items.ts`** (`itemsRecipe`) from `items-demo` + `weapon-demo`: `cb_items` gives/strips/enumerates, `cb_weapon` does the weapon-specific part.

**`recipes/team.ts`** (`teamRecipe`) from `changeteam-demo` + `switchteam-demo`: `cb_changeteam` and `cb_switchteam`. Lead comment — this distinction is the whole point of keeping both:

```ts
// Two different operations, easy to confuse:
//   changeTeam  — the engine's ChangeTeam: kills the pawn, respects team limits
//   switchTeam  — an immediate move that keeps the player alive and armed
// Use switchTeam for team balancing mid-round; changeTeam for a real join.
```

**`recipes/clients.ts`** (`clientsRecipe`) from `clients-demo` + `voice-demo`: `cb_clients` lists connected clients and their lifecycle state, `cb_voice` demonstrates `ctx.clients.onVoice`.

- [ ] **Step 3: Vendor the zones contract**

`@s2script/zones` is published by `plugins/zones/`, not by the SDK. The consumer typechecks against a byte-copy of the producer's contract, which `s2s build` hashes into `manifest.compiledAgainst`.

```bash
mkdir -p examples/cookbook/.s2script/types/@s2script/zones
cp plugins/zones/api.d.ts examples/cookbook/.s2script/types/@s2script/zones/index.d.ts
```

- [ ] **Step 4: Write the zones recipe as an OPTIONAL dependency**

`examples/zones-consumer-demo` hard-deps `@s2script/zones` via `ctx.use`. A hard dep in the cookbook would make the whole cookbook refuse to load unless `plugins/zones` is loaded — unacceptable for a teaching artifact. Use the optional path instead (declared in Task 5 Step 1's `optionalPluginDependencies`).

`examples/cookbook/src/recipes/zones.ts`:

```ts
import type { Recipe } from "../recipe.ts";
import type { Zones, ZoneEvent } from "@s2script/zones";
import { Player } from "@s2script/cs2";

/**
 * Consuming another PLUGIN's interface (not an SDK module). @s2script/zones is
 * published by plugins/zones. Declared under optionalPluginDependencies, so
 * tryUse() returns null when that plugin isn't loaded and the cookbook still
 * works — a hard dep (ctx.use) would refuse to load the whole plugin instead.
 *
 * Types come from the verified contract copy at
 * .s2script/types/@s2script/zones/index.d.ts — a byte-copy of the producer's
 * api.d.ts that s2s build hashes into manifest.compiledAgainst, so a drifted
 * contract is refused at load rather than marshalled across. Refresh with:
 *   cp plugins/zones/api.d.ts examples/cookbook/.s2script/types/@s2script/zones/index.d.ts
 */
export const zonesRecipe: Recipe = {
  name: "zones",
  describe: "react to zone enter/leave from the zones plugin (optional dep)",
  register(ctx) {
    const zones = ctx.tryUse<Zones>("@s2script/zones");
    if (!zones) {
      console.log("[cookbook] zones recipe idle — @s2script/zones is not loaded");
      return;
    }
    zones.on("enter", (p: ZoneEvent) => {
      const name = Player.fromSlot(p.slot)?.playerName ?? `slot ${p.slot}`;
      console.log(`[cookbook] ENTER ${p.zone}: ${name}`);
    });
    zones.on("leave", (p: ZoneEvent) => {
      const name = Player.fromSlot(p.slot)?.playerName ?? `slot ${p.slot}`;
      console.log(`[cookbook] LEAVE ${p.zone}: ${name}`);
    });
  },
};
```

- [ ] **Step 5: Register every new recipe**

Extend `examples/cookbook/src/recipes/index.ts` with all 17 new imports and array entries, keeping the array alphabetical by `name`:

```ts
import type { Recipe } from "../recipe.ts";
import { clientsRecipe } from "./clients.ts";
import { cookiesRecipe } from "./cookies.ts";
import { dbRecipe } from "./db.ts";
import { eventsRecipe } from "./events.ts";
import { gamerulesRecipe } from "./gamerules.ts";
import { httpRecipe } from "./http.ts";
import { itemsRecipe } from "./items.ts";
import { menuRecipe } from "./menu.ts";
import { netRecipe } from "./net.ts";
import { playerStateRecipe } from "./player-state.ts";
import { serverRecipe } from "./server.ts";
import { soundRecipe } from "./sound.ts";
import { teamRecipe } from "./team.ts";
import { traceRecipe } from "./trace.ts";
import { translationsRecipe } from "./translations.ts";
import { transmitRecipe } from "./transmit.ts";
import { usercmdRecipe } from "./usercmd.ts";
import { usermessagesRecipe } from "./usermessages.ts";
import { wsRecipe } from "./ws.ts";
import { zonesRecipe } from "./zones.ts";

/** Every recipe the cookbook registers. Add new ones here. */
export const RECIPES: readonly Recipe[] = [
  clientsRecipe, cookiesRecipe, dbRecipe, eventsRecipe, gamerulesRecipe,
  httpRecipe, itemsRecipe, menuRecipe, netRecipe, playerStateRecipe,
  serverRecipe, soundRecipe, teamRecipe, traceRecipe, translationsRecipe,
  transmitRecipe, usercmdRecipe, usermessagesRecipe, wsRecipe, zonesRecipe,
];
```

- [ ] **Step 6: Delete the absorbed demos**

```bash
git rm -r examples/net-demo examples/trace-demo examples/transmit-demo \
  examples/translations-demo examples/usercmd-demo examples/usermsg-demo \
  examples/menu-demo examples/clientprefs-demo examples/respawn-demo \
  examples/round-control-demo examples/clientlist-convar-mapstart-demo \
  examples/gamerules-usermsg-demo examples/db-demo examples/db-remote-demo \
  examples/items-demo examples/weapon-demo examples/changeteam-demo \
  examples/switchteam-demo examples/clients-demo examples/voice-demo \
  examples/zones-consumer-demo examples/admin-groups-demo examples/liveness-gate
```

`admin-groups-demo` and `liveness-gate` are deletions with no destination — `admin` is exercised by the shipped `plugins/` suite, and `liveness-gate` is a live-gate rig for the shipped E1 slice (spec §4.3).

- [ ] **Step 7: Typecheck**

```bash
./scripts/check-plugins-typecheck.sh 2>&1 | tail -20
```

Expected: `PASS`. If the zones recipe fails to resolve `@s2script/zones`, confirm Step 3's contract copy exists and that `optionalPluginDependencies` (not `pluginDependencies`) names it in `examples/cookbook/package.json`.

- [ ] **Step 8: Build**

```bash
node --experimental-strip-types --no-warnings -e "
import('./packages/sdk/src/build.ts').then(({buildPlugin}) =>
  buildPlugin('examples/cookbook', 'packages')).then(p => console.log('OK', p))
"
```

Expected: `OK …`. A `WARN: dependency "@s2script/zones" is declared but never ctx.use()d` must NOT appear — the recipe calls `tryUse`, which the build's dependency scan counts.

- [ ] **Step 9: Verify the directory count**

```bash
ls -d examples/*/
```

Expected: exactly seven directories — `cookbook`, `entity-playground`, `entref-consumer`, `entref-producer`, `greeter-consumer`, `greeter-plugin`, `hello-plugin`. The `entref-*` pair is deleted in Task 7 and `monorepo-plugin` is added in Task 8, bringing the final count to six.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat(examples): convert remaining demos to cookbook recipes

19 single-module demos become one file each under examples/cookbook/src/
recipes/. db+db-remote, items+weapon, changeteam+switchteam, and
clients+voice merge into one recipe apiece. zones becomes an OPTIONAL
plugin dep via tryUse so the cookbook loads standalone. admin-groups-demo
and liveness-gate are deleted outright."
```

---

## Task 7: Consolidate the interface pair

`greeter-*` and `entref-*` both demonstrate cross-plugin interfaces. Fold the EntityRef-on-the-wire case — the genuinely distinct part — into the greeter pair and delete the duplicate.

**Files:**
- Modify: `examples/greeter-plugin/api.d.ts`
- Modify: `examples/greeter-plugin/package.json`
- Modify: `examples/greeter-plugin/src/plugin.ts`
- Modify: `examples/greeter-consumer/src/plugin.ts`
- Delete: `examples/greeter-consumer/src/greeter.d.ts`
- Delete: `examples/entref-producer/`, `examples/entref-consumer/`

**Interfaces:**
- Consumes: `ctx.publish`, `ctx.use` from `@s2script/sdk/plugin`; `Pawn` from `@s2script/cs2`
- Produces: nothing consumed by later tasks

- [ ] **Step 1: Extend the published contract**

`examples/greeter-plugin/api.d.ts`:

```ts
/**
 * @demo/greeter — the contract this example publishes.
 *
 * The impl in src/plugin.ts is declared `: Greeter`, so `s2s build` fails if a
 * method drifts from this file. Consumers typecheck against this same file, and
 * the build hashes it into manifest.compiledAgainst — a drifted contract is
 * refused at load rather than marshalled across.
 *
 * Note the interface name here matches the package name, so the manifest's
 * `publishes` block is derived automatically as "self". A contract named
 * differently from its package needs an authored publishes entry with a
 * concrete version.
 */
import type { EntityRef } from "@s2script/sdk/entity";

export interface Greeter {
  /** Greet the player in `slot`. */
  greet(slot: number): string;
  /**
   * The slot's pawn as a live, serial-gated EntityRef — or null if there is
   * no such pawn. The ref survives the crossing as a LIVE ref: the consumer
   * validates it against the SHARED entity system, and it flips to invalid
   * when the pawn dies. Never a raw pointer, never a dead copy.
   */
  pawnRef(slot: number): EntityRef | null;
  /** A producer-side schema read, so the consumer needs no offset of its own. */
  pawnHealth(slot: number): number | null;
}
```

- [ ] **Step 2: Add the cs2 dependency to the producer**

`examples/greeter-plugin/package.json` — `pawnRef`/`pawnHealth` need `Pawn`:

```json
{
  "name": "@demo/greeter",
  "version": "1.0.0",
  "main": "src/plugin.ts",
  "types": "api.d.ts",
  "dependencies": {
    "@s2script/sdk": "^0.1.0",
    "@s2script/cs2": "^0.5.0"
  },
  "s2script": {
    "publishes": {
      "@demo/greeter": "1.0.0"
    }
  }
}
```

- [ ] **Step 3: Implement the new methods in the producer**

`examples/greeter-plugin/src/plugin.ts`:

```ts
// Producer: publishes the typed inter-plugin interface @demo/greeter@1.0.0.
//
// Methods become natives the consumer calls; handle.emit() sends forwarded
// events the consumer subscribes to with on(). Arguments and payloads cross by
// STRUCTURED COPY as JSON — never a live pointer. An EntityRef is the one
// exception: it is tagged crossing the wire and revived bound to the consumer's
// own natives, so it arrives as a live, serial-gated ref.
//
// Carry 64-bit values as decimal strings: a BigInt throws and silently drops
// the whole payload.
import { plugin } from "@s2script/sdk/plugin";
import { Pawn } from "@s2script/cs2";
import type { Greeter } from "../api";

export default plugin((ctx) => {
  console.log("[greeter] onLoad — publishing @demo/greeter");

  // Typed against the contract: tsc fails the build if this drifts from api.d.ts.
  // The version is injected by the host from the manifest — never typed here.
  const impl: Greeter = {
    greet(slot: number): string {
      return `hello, player ${slot}`;
    },
    pawnRef(slot: number) {
      const pawn = Pawn.forSlot(slot);
      return pawn ? pawn.ref : null;
    },
    pawnHealth(slot: number) {
      const pawn = Pawn.forSlot(slot);
      return pawn ? pawn.health : null;
    },
  };

  const handle = ctx.publish("@demo/greeter", impl);

  // Forwarded events: the consumer's on("greeted") fires from here.
  let ticks = 0;
  ctx.server.onGameFrame(() => {
    if (ticks++ % 256 === 0) handle.emit("greeted", { slot: 0, tick: ticks });
  });
});
```

- [ ] **Step 4: Extend the consumer**

`examples/greeter-consumer/src/plugin.ts`:

```ts
// Consumer: hard-deps @demo/greeter. ctx.use returns a proxy that throws
// InterfaceUnavailable while the producer is unloaded, so calls are wrapped —
// a producer reload degrades gracefully instead of crashing this plugin.
// (For a dependency you can live without, declare it under
// optionalPluginDependencies and use ctx.tryUse, which returns null instead.)
import { plugin } from "@s2script/sdk/plugin";
import type { Greeter } from "../../greeter-plugin/api";

export default plugin((ctx) => {
  console.log("[consumer] onLoad");
  const greeter = ctx.use<Greeter>("@demo/greeter");

  // A forwarded event from the producer.
  greeter.on("greeted", (p: { slot: number; tick: number }) =>
    console.log(`[consumer] event greeted: slot=${p.slot} tick=${p.tick}`));

  let ticks = 0;
  ctx.server.onGameFrame(() => {
    if (ticks++ % 256 !== 0) return;
    try {
      console.log(`[consumer] greet -> ${greeter.greet(0)}`);

      // An EntityRef received ACROSS the plugin boundary. isValid() checks it
      // against the SHARED entity system: true while the pawn lives, false once
      // it dies. That flip is cross-plugin host-invalidation, and it needs no
      // schema offset on this side — pawnHealth is the producer's read.
      const ref = greeter.pawnRef(0);
      const alive = ref ? ref.isValid() : false;
      console.log(`[consumer] pawn ref valid=${alive} health=${alive ? greeter.pawnHealth(0) : "null"}`);
    } catch (e) {
      console.log(`[consumer] degraded (producer unloaded?): ${String(e)}`);
    }
  });
});
```

- [ ] **Step 5: Delete the stale ambient declaration and the entref pair**

`examples/greeter-consumer/src/greeter.d.ts` is a legacy hand-written `declare module` superseded by the `../../greeter-plugin/api` type import.

```bash
git rm examples/greeter-consumer/src/greeter.d.ts
git rm -r examples/entref-producer examples/entref-consumer
```

- [ ] **Step 6: Typecheck**

```bash
./scripts/check-plugins-typecheck.sh 2>&1 | grep -E "greeter|PASS|FAIL"
```

Expected: `OK` for both greeter dirs, final `PASS`.

- [ ] **Step 7: Build both, confirming the publish gate still derives**

```bash
node --experimental-strip-types --no-warnings -e "
import('./packages/sdk/src/build.ts').then(async ({buildPlugin}) => {
  for (const d of ['examples/greeter-plugin','examples/greeter-consumer'])
    console.log('OK', await buildPlugin(d, 'packages'));
})
"
```

Expected: two `OK` lines. A `publishes drift` error means Step 1's contract and Step 3's impl disagree — fix the impl.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(examples): fold entref-* into the greeter pair

Both pairs demonstrated cross-plugin interfaces; only the EntityRef-on-the-
wire case was distinct. @demo/greeter gains pawnRef/pawnHealth, the consumer
shows the cross-plugin invalidation flip, and the duplicate pair plus a stale
hand-written ambient declaration are deleted."
```

---

## Task 8: monorepo-plugin

The example nothing in the repo currently provides: how to structure a large plugin across packages. Task 1 made `main` resolve; this example uses `exports` and documents why both work.

**Files:**
- Create: `examples/monorepo-plugin/package.json`
- Create: `examples/monorepo-plugin/tsconfig.json`
- Create: `examples/monorepo-plugin/README.md`
- Create: `examples/monorepo-plugin/src/plugin.ts`
- Create: `examples/monorepo-plugin/packages/core/package.json`
- Create: `examples/monorepo-plugin/packages/core/src/index.ts`
- Create: `examples/monorepo-plugin/packages/commands/package.json`
- Create: `examples/monorepo-plugin/packages/commands/src/index.ts`
- Create: `examples/monorepo-plugin/node_modules/@monorepo-example/{core,commands}` (symlinks)

**Interfaces:**
- Consumes: workspace-sibling resolution from Task 1
- Produces: nothing consumed by later tasks

- [ ] **Step 1: Create the root manifest**

`examples/monorepo-plugin/package.json`:

```json
{
  "name": "@example/monorepo-plugin",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "workspaces": ["packages/*"],
  "dependencies": {
    "@s2script/sdk": "^0.1.0",
    "@monorepo-example/core": "*",
    "@monorepo-example/commands": "*"
  }
}
```

- [ ] **Step 2: Create the tsconfig**

`examples/monorepo-plugin/tsconfig.json` — `include` must cover the sibling sources so editor IntelliSense matches the build:

```json
{
  "extends": "../../tsconfig.base.json",
  "include": ["src", "packages/*/src", "../../packages/sdk/globals.d.ts"]
}
```

- [ ] **Step 3: Write the shared core package**

`examples/monorepo-plugin/packages/core/package.json` — note `exports`, not `main`:

```json
{
  "name": "@monorepo-example/core",
  "version": "0.1.0",
  "exports": { ".": "./src/index.ts" }
}
```

`examples/monorepo-plugin/packages/core/src/index.ts`:

```ts
/** Shared types and helpers every feature package in this plugin depends on. */

export interface Greeting {
  readonly text: string;
  readonly at: number;
}

/** A tiny in-memory store, the sort of thing a feature package shares. */
export class GreetingLog {
  readonly #entries: Greeting[] = [];

  add(text: string, at: number): void {
    this.#entries.push({ text, at });
  }

  get count(): number {
    return this.#entries.length;
  }

  latest(): Greeting | null {
    return this.#entries.at(-1) ?? null;
  }
}
```

- [ ] **Step 4: Write the feature package**

`examples/monorepo-plugin/packages/commands/package.json`:

```json
{
  "name": "@monorepo-example/commands",
  "version": "0.1.0",
  "exports": { ".": "./src/index.ts" }
}
```

`examples/monorepo-plugin/packages/commands/src/index.ts`:

```ts
import type { PluginContext } from "@s2script/sdk/plugin";
import { GreetingLog } from "@monorepo-example/core";

/**
 * A feature package: it receives the plugin context and the shared store, and
 * owns one slice of behaviour. Feature packages import @monorepo-example/core
 * — never each other — so the dependency graph stays a tree.
 */
export function registerCommands(ctx: PluginContext, log: GreetingLog): void {
  let frames = 0;
  ctx.server.onGameFrame(() => { frames += 1; });

  ctx.commands.register("mono_greet", (cmd) => {
    log.add("hello from a workspace package", frames);
    cmd.reply(`logged greeting #${log.count}`);
  });

  ctx.commands.register("mono_latest", (cmd) => {
    const latest = log.latest();
    cmd.reply(latest ? `#${log.count} "${latest.text}" at frame ${latest.at}` : "nothing logged yet");
  });
}
```

- [ ] **Step 5: Write the plugin entry**

`examples/monorepo-plugin/src/plugin.ts`:

```ts
// monorepo-plugin — one plugin split across npm workspace packages.
//
// Use this shape when a plugin outgrows a single src/ directory. The whole
// tree bundles into ONE .s2sp: sibling packages are INLINED at build time,
// not resolved at runtime.
//
// Not to be confused with cross-plugin interfaces (see greeter-plugin):
//   - workspace packages = a BUILD-TIME factoring of one plugin
//   - published interfaces = a RUNTIME contract between two separate plugins
// If two parts must load, unload, and version independently, they are two
// plugins, not two packages.
import { plugin } from "@s2script/sdk/plugin";
import { GreetingLog } from "@monorepo-example/core";
import { registerCommands } from "@monorepo-example/commands";

export default plugin((ctx) => {
  const log = new GreetingLog();
  registerCommands(ctx, log);
  console.log("[monorepo] loaded — try mono_greet and mono_latest");
});
```

- [ ] **Step 6: Create the workspace symlinks**

npm creates these on `npm install`; the example must carry them so the repo's gate works without an install inside the example.

```bash
cd examples/monorepo-plugin
mkdir -p node_modules/@monorepo-example
ln -s ../../packages/core node_modules/@monorepo-example/core
ln -s ../../packages/commands node_modules/@monorepo-example/commands
cd -
git add -f examples/monorepo-plugin/node_modules
```

`-f` is required: `node_modules` is gitignored repo-wide. Confirm both symlinks are tracked:

```bash
git ls-files examples/monorepo-plugin/node_modules
```

Expected: two paths listed.

- [ ] **Step 7: Write the example README**

`examples/monorepo-plugin/README.md`:

```markdown
# monorepo-plugin

One plugin, split across npm workspace packages.

```
package.json          workspaces: ["packages/*"]
src/plugin.ts         the entry point — composes the feature packages
packages/core/        shared types and state
packages/commands/    one slice of behaviour, importing core
```

## Two rules

1. **Sibling packages should declare `exports`**, not `main`:

   ```json
   { "name": "@monorepo-example/core", "exports": { ".": "./src/index.ts" } }
   ```

   `main` also works, but `exports` is the modern field and is what the
   bundler resolves without any configuration.

2. **The whole tree bundles into one `.s2sp`.** Sibling packages are inlined at
   build time. They are not runtime dependencies and do not appear in the
   manifest.

## Not the same as a cross-plugin interface

Workspace packages are a build-time factoring of **one** plugin. If two parts
need to load, unload, and version independently, they are two plugins talking
over a published interface — see `examples/greeter-plugin`.

## Build

```bash
npx s2s build examples/monorepo-plugin
```
```

- [ ] **Step 8: Typecheck**

```bash
./scripts/check-plugins-typecheck.sh 2>&1 | grep -E "monorepo|PASS|FAIL"
```

Expected: `OK` for `examples/monorepo-plugin/`, final `PASS`.

- [ ] **Step 9: Build and verify the sibling code is inlined**

```bash
node --experimental-strip-types --no-warnings -e "
import('./packages/sdk/src/build.ts').then(async ({buildPlugin}) => {
  const out = await buildPlugin('examples/monorepo-plugin', 'packages');
  const AdmZip = (await import('adm-zip')).default;
  const js = new AdmZip(out).readAsText('plugin.js');
  console.log('inlined GreetingLog:', js.includes('GreetingLog') ? 'YES' : 'NO');
  console.log('left as require   :', js.includes('require(\"@monorepo-example/core\")') ? 'YES (BAD)' : 'NO (good)');
})
"
```

Expected:
```
inlined GreetingLog: YES
left as require   : NO (good)
```

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat(examples): monorepo-plugin — one plugin across workspace packages

Shows the structure nothing in the repo showed: npm workspaces, a shared
core package, a feature package, and the whole tree bundling into one .s2sp
with siblings inlined. Documents the boundary against cross-plugin
interfaces, which is the mistake this example exists to prevent."
```

---

## Task 9: Module coverage gate

Without this, the cleanup restores coverage once and then rots as new modules land — which is how 39 directories accumulated. Fails the build when a shipped SDK module has no consumer anywhere.

**Files:**
- Create: `scripts/check-examples-coverage.sh`
- Modify: `CLAUDE.md` (gate-suite list)

**Interfaces:**
- Consumes: the `examples/`, `plugins/`, `tools/` trees as they stand after Tasks 2–8
- Produces: `./scripts/check-examples-coverage.sh`, added to the gate suite

- [ ] **Step 1: Write the gate script**

`scripts/check-examples-coverage.sh`:

```bash
#!/usr/bin/env bash
# Every shipped SDK capability module must have at least one consumer under
# examples/, plugins/, or tools/. Without this, the curated example set silently
# stops covering the API as new modules land — and the typecheck gate can only
# catch regressions in modules something actually imports.
set -euo pipefail
cd "$(dirname "$0")/.."

# Shipped capability modules = packages/sdk/<cap>.d.ts, minus globals.d.ts
# (ambient declarations, not importable as a module).
mapfile -t modules < <(
  find packages/sdk -maxdepth 1 -name '*.d.ts' -printf '%f\n' \
    | sed 's/\.d\.ts$//' \
    | grep -vx -e globals \
    | sort
)

# Every @s2script/sdk/<cap> and @s2script/<pkg> imported anywhere in the corpus.
imported=$(grep -rhoE 'from "@s2script/(sdk/)?[a-z0-9-]+"' \
             examples plugins tools --include='*.ts' 2>/dev/null \
           | sed -E 's|from "@s2script/(sdk/)?||; s|"||' \
           | sort -u)

fail=0
for m in "${modules[@]}"; do
  if ! grep -qx "$m" <<<"$imported"; then
    echo "UNCOVERED: @s2script/sdk/$m has no consumer in examples/, plugins/, or tools/"
    fail=1
  fi
done

if [ "$fail" = 0 ]; then
  echo "PASS: all ${#modules[@]} shipped SDK modules have a consumer"
else
  echo "FAIL: add a cookbook recipe (examples/cookbook/src/recipes/) for each module above"
  exit 1
fi
```

```bash
chmod +x scripts/check-examples-coverage.sh
```

- [ ] **Step 2: Run it**

```bash
./scripts/check-examples-coverage.sh
```

Expected: `PASS: all N shipped SDK modules have a consumer`.

If a module reports UNCOVERED, that is a real gap — either it was dropped during Task 6 (add the recipe back) or it never had a consumer (add a minimal recipe). Do NOT silence it by filtering the module out of the list. `damage`, `console`, and `plugins` are the likely candidates; each gets a real recipe if uncovered.

- [ ] **Step 3: Prove the gate actually fails**

A gate that cannot fail is not a gate.

```bash
mv examples/cookbook/src/recipes/trace.ts /tmp/trace.ts.bak
./scripts/check-examples-coverage.sh; echo "exit=$?"
mv /tmp/trace.ts.bak examples/cookbook/src/recipes/trace.ts
```

Expected: `UNCOVERED: @s2script/sdk/trace …`, `FAIL: …`, `exit=1`. Then, after restoring, re-run and confirm `PASS` and `exit=0`.

- [ ] **Step 4: Add it to the gate suite in CLAUDE.md**

In the "Gate suite (run before every PR)" block, add the new line after `./scripts/check-plugins-typecheck.sh`:

```bash
./scripts/check-examples-coverage.sh   # every shipped SDK module has a consumer
```

- [ ] **Step 5: Commit**

```bash
git add scripts/check-examples-coverage.sh CLAUDE.md
git commit -m "test(gate): fail when a shipped SDK module has no consumer

The example corpus is what gives the typecheck gate its API coverage. This
gate keeps it honest as new modules land, so the curated set cannot rot back
into partial coverage."
```

---

## Task 10: Documentation

The examples are currently unreachable from `README.md`. A curated showcase nobody links to is wasted work.

**Files:**
- Modify: `README.md`
- Modify: `docs/BUILDING.md` (layout block ~line 25; build-scripts note ~line 149)
- Modify: `CLAUDE.md` (repository-layout block)
- Verify: `docs/INSTALL.md:57`

**Interfaces:**
- Consumes: the final directory layout from Tasks 2–8
- Produces: nothing

- [ ] **Step 1: Add an Examples section to README.md**

Insert after the build/install content, before any appendix:

```markdown
## Examples

Six worked examples under [`examples/`](examples/), smallest first:

| Example | What it teaches |
|---|---|
| [`hello-plugin`](examples/hello-plugin) | The smallest complete plugin — a command, an event, and surviving a hot reload. **Start here.** |
| [`cookbook`](examples/cookbook) | One file per API under `src/recipes/` — HTTP, websockets, sockets, DB, cookies, menus, sounds, traces, usermessages, and more. Copy a recipe into your own plugin. |
| [`entity-playground`](examples/entity-playground) | Creating, configuring, and watching entities: keyvalue-configured spawns, entity I/O, lifecycle listeners, beams. |
| [`greeter-plugin`](examples/greeter-plugin) + [`greeter-consumer`](examples/greeter-consumer) | Two plugins talking over a typed, versioned interface — including an `EntityRef` that stays live across the boundary. |
| [`monorepo-plugin`](examples/monorepo-plugin) | Splitting one plugin across npm workspace packages when it outgrows a single `src/`. |

Build any of them with `npx s2s build examples/<name>`, then drop the resulting
`dist/*.s2sp` into `addons/s2script/plugins/` on a running server.

Dev tooling lives in [`tools/`](tools/) — `schema-dump` (regenerates gamedata
after a CS2 update), `s2bench` (op timing), and `crash-test`.
```

- [ ] **Step 2: Update docs/BUILDING.md**

Line ~25's layout block currently reads `examples/     Demo plugins (not shipped).` Replace with:

```
examples/     Worked examples (not shipped) — see README.md.
tools/        Dev/treadmill tooling: schema-dump, s2bench, crash-test (not shipped).
```

Line ~149's note currently reads that `build-base-plugins.sh` builds `plugins/*` and `plugins/disabled/*` "(demos in `examples/` are not…". Update the parenthetical to `(examples/ and tools/ are not packaged)`.

- [ ] **Step 3: Update the CLAUDE.md repository-layout block**

Change the `examples/` line and add `tools/`:

```
examples/    Worked examples (not shipped): hello-plugin, cookbook, entity-playground,
             greeter-plugin/-consumer, monorepo-plugin.
tools/       Dev/treadmill tooling (not shipped): schema-dump, s2bench, crash-test.
```

- [ ] **Step 4: Verify docs/INSTALL.md:57**

```bash
sed -n '55,59p' docs/INSTALL.md
```

It reads "demos live under `examples/` and are not packaged" — still true. If the wording names a deleted directory, correct it; otherwise leave it.

- [ ] **Step 5: Verify every README link resolves**

```bash
for d in hello-plugin cookbook entity-playground greeter-plugin greeter-consumer monorepo-plugin; do
  [ -d "examples/$d" ] && echo "OK   examples/$d" || echo "DEAD examples/$d"
done
```

Expected: six `OK` lines, no `DEAD`.

- [ ] **Step 6: Commit**

```bash
git add README.md docs/BUILDING.md CLAUDE.md docs/INSTALL.md
git commit -m "docs: link the curated examples from the README front door

README referenced examples zero times. Adds a six-row table, smallest first,
and records the new tools/ directory in the layout blocks."
```

---

## Task 11: Full verification and PR

**Files:** none (verification only)

- [ ] **Step 1: Confirm the final shape**

```bash
ls -d examples/*/ tools/*/
```

Expected exactly nine directories:
```
examples/cookbook/  examples/entity-playground/  examples/greeter-consumer/
examples/greeter-plugin/  examples/hello-plugin/  examples/monorepo-plugin/
tools/crash-test/  tools/s2bench/  tools/schema-dump/
```

- [ ] **Step 2: Run the full gate suite**

Per CLAUDE.md, every gate — not just the ones this work touched.

```bash
make check-boundary
./scripts/check-plugins-typecheck.sh
./scripts/check-examples-coverage.sh
./scripts/check-schema-generated.sh
./scripts/check-nav-generated.sh
./scripts/check-events-generated.sh
./scripts/check-csitem-generated.sh
./scripts/check-licenses-generated.sh
./scripts/test-boundary-nameleak.sh
npm test --workspace @s2script/sdk
```

Every one must pass. If a codegen freshness gate fails, it is unrelated to this work — report it, do not "fix" it by regenerating.

- [ ] **Step 3: Build every surviving example and tool**

```bash
for d in examples/*/ tools/*/; do
  node --experimental-strip-types --no-warnings -e "
    import('./packages/sdk/src/build.ts').then(({buildPlugin}) =>
      buildPlugin('$d', 'packages')).then(p => console.log('OK  $d'))
      .catch(e => { console.log('FAIL $d:', e.message); process.exit(1); })
  " || exit 1
done
```

Expected: nine `OK` lines.

- [ ] **Step 4: Confirm the diff is mostly deletion**

```bash
git diff --stat origin/main | tail -3
```

Sanity check on the spec's §5 claim that the diff is overwhelmingly deletions and moves. Record the numbers for the PR body.

- [ ] **Step 5: Write the PR body to a file**

Never a heredoc for PR bodies — shell escaping mangles tables and code blocks. Write `/tmp/pr-body.md` covering:
- **What** — 39 example dirs → 6 examples + 3 tools; new monorepo example; new coverage gate.
- **Why** — 78 boilerplate files for 1,705 lines of example code; README linked none of it.
- **The constraint** — twelve modules were covered only by `examples/`; all retained, now gate-enforced.
- **The CLI fix** — `platform: "neutral"` zeroed esbuild's `mainFields`; workspace siblings declaring `main` could not resolve.
- **Verification** — the gate-suite output from Step 2 and the build results from Step 3.

- [ ] **Step 6: Push and open the PR**

```bash
git push -u origin examples/cleanup
gh pr create --title "examples: curated showcase + monorepo example" --body-file /tmp/pr-body.md
```

- [ ] **Step 7: Report**

State the final directory count, the gate-suite results verbatim, and anything deferred — in particular whether Task 4 split `entity-playground`, and any module that needed a new recipe in Task 9 Step 2.

---

## Notes for the implementer

- **No live gate in this plan.** Every deliverable is verified by typecheck + build. The examples are not shipped in the release zip, so a CS2 server run is not a merge blocker. If you want one, `examples/cookbook` is the useful target (`cb_list` proves registration).
- **Do not "improve" ported recipe behaviour.** Task 6 is a move, not a rewrite. Changing what a demo does while relocating it makes the diff unreviewable. Strip live-gate commentary and rename commands; leave the logic alone.
- **One sanctioned exception to that rule: work performed at load.** A demo that was the only plugin loaded could open a socket, hit an HTTP endpoint, or start a timer in its factory. Twenty recipes doing that at once makes merely loading the cookbook perform a burst of network I/O. Any recipe whose source did work at load moves that work behind its `cb_*` command (Task 5's `ws` recipe is the worked example). Subscriptions — `onGameFrame`, `onPrecache`, `events.on`, `entities.on*` — still register at load; that is what `register` is for. Note each such conversion in the task report.
- **If a source demo does not typecheck after porting**, the cause is almost always a dropped import or a `globalThis as any` escape that needs the real typed import. Read the original file again before changing any API call.
