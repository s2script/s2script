# Slice 4 — One `.s2sp` That Hot-Reloads (Context + Ledger) — Design Spec

- **Project:** s2script (TypeScript plugin framework for Source 2; SourceMod's spiritual successor)
- **Date:** 2026-06-30
- **Status:** Approved design, ready for implementation planning
- **Builds on:** Slices 0 (V8 in CS2), 1 (multiplexer + `OnGameFrame`), 2 (tick-integrated async), 3 (schema-backed `pawn.health` in `@s2script/cs2`) — all merged to `main`.
- **Scope:** Slice 4 only — the plugin lifecycle milestone: one `.s2sp` that loads, hot-reloads, and tears down cleanly. See `docs/ARCHITECTURE.md` §2.1, §2.5, §2.7, §3 (Slice 4). Inter-plugin comms (§2.9) are **Slice 4.5**, explicitly out of scope here.

---

## 1. Purpose & what it proves

**The milestone: the whole architecture proven end-to-end on a thin thread.** Author a TypeScript plugin → `s2script build` → a `.s2sp` archive → drop it into `addons/s2script/plugins/` → it loads into its **own V8 context**, exercising Slices 1–3 (`OnGameFrame`, `await delay`, `Pawn.health`) → edit + rebuild + drop → it **hot-reloads without a server restart** → delete the file → **clean ledger teardown**, no leaked subscriptions/timers/async, no crash. This retires "does the whole spine hold together." It **replaces the Slice-0/1/2/3 baked-eval demos** with real plugin loading.

The three novel risks it retires: **context-per-plugin** (per-plugin unload scope via disposing a context), **the ledger as the teardown authority** (teardown doesn't depend on the plugin's own cleanup code), and the **async-liveness guard** (a threadpool/timer continuation whose plugin was unloaded mid-flight is dropped, never run into a disposed context — the use-after-free killer).

## 2. Decided directions

1. **Context-per-plugin** (confirmed): one shared V8 isolate hosts a registry of plugin instances, each with its **own `v8::Context`**. The provisional API is installed per-context. (The demo exercises N=1; the machinery is genuinely per-plugin — that is the milestone.)
2. **The ledger is the teardown authority** (confirmed): every persistent resource a plugin acquires (hook subscriptions, timers, pending async) is auto-recorded in its ledger; teardown walks the ledger in reverse at a frame boundary, then disposes the context.
3. **Async-liveness guard** (confirmed): each timer/job resolver is tagged `(plugin-id, generation)`; the frame drain drops a continuation whose plugin is no longer live.
4. **`s2script build` uses esbuild** (confirmed over Vite): transpile TS→JS + bundle `dependencies` into one `plugin.js` targeting a **bare V8 embed** (`platform=neutral`), with `@s2script/*` marked **external** (runtime-injected); derive a minimal `manifest.json`; zip.
5. **Naming convention locked** (confirmed): **PascalCase for events + types** (`OnGameFrame`, `Pawn`), **camelCase for functions + properties** (`delay`, `nextTick`, `nextFrame`, `threadSleep`, `pawn.health`). This renames Slice-1's `onGameFrame → OnGameFrame` and Slice-2's `Delay/NextTick/NextFrame → delay/nextTick/nextFrame`.
6. **TS authoring, transpile-only** (confirmed): plugins are TypeScript; esbuild transpiles to JS. The blocking `tsc` typecheck gate is **deferred** to a follow-up.
7. **File-watch by polling on the frame drain** (main-thread; no new thread).

## 3. Runtime — context-per-plugin (`core/src/v8host.rs` refactor)

Today `v8host` holds one shared `Host { isolate, context }`. Slice 4 refactors to: **one shared isolate + a plugin registry.**

- **`PluginInstance { id: String, context: v8::Global<v8::Context>, ledger: PluginLedger, generation: u64 }`**, held in a registry keyed by plugin id (`HashMap<String, PluginInstance>`), plus a `NEXT_GENERATION` counter (a reloaded plugin gets a fresh generation, so stale resolvers from the previous incarnation are dropped).
- **Per-context API install.** On load, create a fresh `v8::Context`, store the plugin id in the context's **embedder-data slot** (index 0), and install the injected API (§6) into it: the `@s2script/std` globals (`OnGameFrame`, `delay`, `nextTick`, `nextFrame`, `threadSleep`, `console`) and `@s2script/cs2` (`Pawn`) — plus the CJS `require`/`module` shim.
- **Identifying the calling plugin.** A native reads `scope.get_current_context()` → the embedder-data slot → the plugin id, then routes subscriptions/timers to that plugin's ledger. **This uses V8's current context, not a thread-local**, so it stays correct across the microtask checkpoint (which runs each plugin's continuation in its own context).
- **Entering a plugin's context.** Core enters a plugin's `v8::Context` (a `ContextScope`) whenever it runs that plugin's JS: top-level eval, `onLoad`/`onUnload`, each `OnGameFrame` handler dispatch, and each timer/job continuation resolution.
- **Multiplexer plugin-tagging.** Each `OnGameFrame` subscription now carries its owning plugin id (alongside the `Global<Function>`). `dispatch_onframe` enters each handler's owning context per handler. The Slice-1 priority ladder / `HookResult` collapse / re-entrancy discipline are unchanged; only the per-handler context-entry + the plugin tag are added.
- **Shared truths.** All plugins share the one isolate and the process-global natives/threadpool/`ENGINE_OPS` (Slice 3). Cross-context entity identity (the wrapper cache) is Slice 5; here `Pawn` is a thin per-context accessor over the shared engine-generic natives.

## 4. The plugin ledger & teardown (`core/src/plugin.rs` new; V8-free where possible)

- **`PluginLedger { hook_subs: Vec<SubId>, timers: Vec<TimerId>, pending_jobs: Vec<JobId> }`** (+ the `context`/`generation` on the `PluginInstance`). The list of tracked resource kinds is exactly the thin thread's persistent effects; more kinds arrive with their slices (commands Slice 5, imported interfaces Slice 4.5, handles Slice 5).
- **Auto-recording.** `OnGameFrame.subscribe` records the `SubId`; `delay`/`nextTick`/`nextFrame`/`threadSleep` record the `TimerId`/`JobId`. Recording is keyed off the current context's plugin id (§3) — the plugin never manages the ledger itself. **Absolute rule (CLAUDE.md):** any provisional-API call granting a persistent effect must ledger it.
- **Teardown (reverse-walk), triggered at a frame boundary** (the file-watch runs on the drain, never mid-dispatch — §5):
  1. Mark the plugin **unloading** → the multiplexer skips its handlers (no memory touched).
  2. Call `onUnload` best-effort (a throw is caught; teardown proceeds via the ledger regardless).
  3. Walk the ledger in reverse: unsubscribe each hook sub (→ may trigger lazy-detour removal via the Slice-1/2 `refresh_detour`); cancel each timer (remove from the timer queue + drop its resolver); drop each pending job's resolver (the worker may still be running — see the liveness guard).
  4. Remove the resolver-map entries for the plugin; dispose the context (`Global<Context>` reset → the context's JS objects become collectable).
  5. Remove the `PluginInstance` from the registry.
- **The async-liveness guard.** A timer/job resolver is tagged `(plugin-id, generation)`. In `frame_async_drain`, before resolving a promise or firing a timer, check the registry: the plugin id must be present **and** its generation must match the resolver's. If the plugin is gone (unloaded) or reloaded (generation advanced), **drop the continuation** — do not enter a disposed/replaced context. A threadpool job completing after its plugin unloaded thus resolves nothing (its resolver was dropped at teardown; a late completion for a stale id is a no-op, as in Slice 2).
- **Crash containment.** A throw in top-level/`onLoad` → the partial ledger is walked to roll back whatever was recorded, and the context is disposed (load fails cleanly, server keeps running). A throw in `onUnload` → force teardown via the ledger anyway.

## 5. The loader, watch & hot-reload (`core/src/loader.rs` new + ffi/shim wiring)

- **Watch.** Core polls `addons/s2script/plugins/` for `*.s2sp` on the frame drain (throttled — e.g. every ~64 drains, ~1s). It keeps a snapshot of `{ filename → mtime }`. A new filename → **load**; a vanished filename → **unload**; a changed mtime → **reload** (unload the old instance, then load). All actions run **on the main thread at the frame boundary** — never mid-dispatch — so teardown is always stack-safe.
- **Load.** Read the `.s2sp` zip **in-memory** → extract `manifest.json` + `plugin.js` → parse the derived manifest → validate (`apiVersion` compatible with the host; no id collision with an already-loaded plugin unless this is a reload of that id) → create the context + inject the API (§3/§6) → eval `plugin.js` under the CJS wrapper capturing `module.exports` → call `onLoad()` → mark active. Any failure degrades with a named reason (the file stays on disk; the server keeps running; a corrected drop retries).
- **Hot-reload = unload(old) + load(new)** for the same id. No state handoff this slice (`onUnload → onLoad(prev)` is a deferred TODO); a reload starts fresh.
- **Shim/ABI.** The shim stops baking demo `eval`/`load_cs2` and instead passes core the resolved plugins-dir path (via `dladdr`, like `GamedataPath`); core owns the watch + load. New C ABI: `s2script_core_set_plugins_dir(path)` (or fold into `init`); the per-frame drain already runs (Slice 2 Post), so the watch hooks there.

## 6. The build tool, `.s2sp` format & API injection

- **`s2script build`** — a Node CLI (`tools/s2script-build/`, run via `node` + esbuild; esbuild is available in the environment). Reads the plugin's `package.json`: `name` → plugin id, `version`, `s2script.apiVersion`, `s2script.pluginDependencies`, `s2script.publishes`. Runs esbuild on `main` (the TS entry) with: `--bundle` (inline npm `dependencies`), `--platform=neutral`, `--format=cjs`, `--external:@s2script/*` (runtime-injected), `--target=es2020`. Derives `manifest.json` (`{ id, version, apiVersion, pluginDependencies, publishes }` — a **minimal** subset; the dev's full `package.json` never ships). Zips `{ manifest.json, plugin.js }` → `<id>.s2sp`.
- **`.s2sp` layout** (zip): `manifest.json` (derived), `plugin.js` (bundled CJS). (`plugin.d.ts`, `translations/`, `configs/` are later slices.)
- **API injection (esbuild-external → per-context CJS `require`).** The plugin authors `import { OnGameFrame, delay } from "@s2script/std"; import { Pawn } from "@s2script/cs2";`. esbuild (format=cjs, externals) emits `const { OnGameFrame, delay } = require("@s2script/std"); ...`. The runtime evals `plugin.js` wrapped as `(function (require, module, exports) { <plugin.js> })(s2require, module, {})`, where **`s2require("@s2script/std")`** returns that context's `{ OnGameFrame, delay, nextTick, nextFrame, threadSleep, console }` and **`s2require("@s2script/cs2")`** returns `{ Pawn }`; `module.exports` captures the plugin's `{ onLoad, onUnload }`. These per-context API objects are built by the injected prelude over the internal natives (`__s2_subscribe`, `__s2_delay`, `__s2_schema_offset`, …), which keep their internal names.
- **Author-time type stubs.** Minimal `@s2script/std` + `@s2script/cs2` **type-stub packages** (`packages/std/`, `packages/cs2/` — `package.json` + `index.d.ts`) declare the injected API's types so the demo plugin typechecks in an editor and esbuild resolves the imports as external. They ship no runtime code (the runtime injects it). Scope: only what the demo uses (`OnGameFrame`, `delay`, `Pawn`).

## 7. Naming rename

Apply the §2.5 convention to the provisional API (JS-facing only — the internal `__s2_*` native names are unchanged):
- **Events → PascalCase objects with `.subscribe`:** `onGameFrame(fn, opts) → OnGameFrame.subscribe(fn, opts)`.
- **Functions → camelCase:** `Delay → delay`, `NextTick → nextTick`, `NextFrame → nextFrame` (`threadSleep` already correct).
- **Types/accessors:** `Pawn` (Pascal, unchanged), `pawn.health` (camel, unchanged).
The rename lives in the per-context injected prelude (§6). The old baked-eval demos that used the old names are **removed** (replaced by the real demo plugin), so there is no lingering old-name surface.

## 8. Demo plugin & live gate

- **`examples/demo-plugin/`** — `package.json` (id e.g. `@demo/hello`, `s2script.apiVersion`, `pluginDependencies: @s2script/std + @s2script/cs2`) + `src/plugin.ts`:
  ```ts
  import { OnGameFrame, delay } from "@s2script/std";
  import { Pawn } from "@s2script/cs2";
  export function onLoad() {
    OnGameFrame.subscribe((f) => { /* periodic: read Pawn.forSlot(0)?.health, log */ });
    (async () => { console.log("[demo] before delay"); await delay(1000); console.log("[demo] after delay"); })();
  }
  export function onUnload() { console.log("[demo] onUnload"); }
  ```
- **Live gate (Docker, operator-run by Claude):** `s2script build examples/demo-plugin` → `@demo/hello.s2sp` → drop into the server's `addons/s2script/plugins/` → observe it **loads** (`onLoad`, `OnGameFrame` ticks, `delay` resolves, `Pawn.health` reads); **edit** the plugin (change a log line / behavior) + rebuild + drop → observe **hot-reload** (old handlers stop, new run, no restart); **delete** the `.s2sp` → observe **clean teardown** (no more ticks from it, `onUnload` logged, no crash, the `OnGameFrame` detour removed if it was the only subscriber). Reuses `scripts/build-sniper.sh`, the Docker harness (the 64 GB `cs2-data` copy), `scripts/rcon.py`.

## 9. Testing strategy

- **Unit (`cargo test`, no engine):** the `PluginLedger` (record + reverse-walk order); the registry (load → present; unload → absent; reload → generation advances); the liveness predicate (a resolver whose `(id, generation)` is stale returns "drop").
- **Integration (`cargo test` + V8, `--test-threads=1`):** load a `plugin.js` into a fresh context + call `onLoad`; a subscription made in `onLoad` is tagged to that plugin and fires on dispatch; **unload** → the subscription is gone (dispatch no longer calls it) and the context is disposed; a `delay` continuation whose plugin was unloaded before the deadline is **dropped** (never runs); two contexts don't leak globals into each other.
- **Build (`node`):** `s2script build` a fixture plugin → a valid `.s2sp` (unzip → `manifest.json` has the derived fields; `plugin.js` is a CJS bundle with `@s2script/*` external).
- **Live (sniper + Docker):** the §8 drop / hot-reload / delete gate.

## 10. Acceptance criteria

1. `cargo test -p s2script-core -- --test-threads=1` passes (ledger + registry + liveness unit tests + the context/lifecycle integration tests + all Slice 0–3 tests, renamed API); `make check-boundary` + the name-leak gate stay green; sniper build loadable.
2. `s2script build` turns a `package.json`-rooted TS plugin into a loadable `.s2sp` (derived manifest + CJS `plugin.js`, `@s2script/*` external).
3. A dropped `.s2sp` **loads** into its own context and runs Slices 1–3 (`OnGameFrame`, `await delay`, `Pawn.health`) — live.
4. Editing + rebuilding + re-dropping **hot-reloads** without a server restart (old instance torn down, new instance active) — live.
5. Deleting the `.s2sp` **tears down cleanly** via the ledger (subscriptions/timers/async gone, context disposed, no crash) — live.
6. The **async-liveness guard** holds: a plugin unloaded with an in-flight `delay`/`threadSleep` drops that continuation (no use-after-free) — cargo + live.
7. The naming convention is applied (`OnGameFrame.subscribe`, `delay`, …); no old-name surface remains.
8. Reproduces from the README (build → drop → reload → delete runbook + acceptance).

## 11. Out of scope (Slice 4)

The `tsc` typecheck gate (deferred follow-up); inter-plugin deps/proxies and `publishes` resolution (Slice 4.5 — the manifest **carries** `pluginDependencies`/`publishes` but the runtime does not resolve inter-plugin proxies yet); the handle/`EntityRef` wrapper + cross-context wrapper cache (Slice 5); config materialization + `permissions` enforcement (declared in the manifest, not enforced); reload **state-handoff** (`onUnload(): State → onLoad(prev)`); topological load-order for multiple interdependent plugins (one plugin this slice); the full phased-unload **stack-drain** (the frame-boundary watch already avoids mid-dispatch teardown); `translations`/`configs`/`assets` in the archive; run-from-archive disk optimizations. Note later needs as TODOs and stop. **Also in scope as housekeeping:** update `CLAUDE.md`'s stale "Current state" section (still says Slice 0) to reflect Slices 0–4.

## 12. File structure / deliverables

- `core/src/v8host.rs` (major refactor) — single-context → per-plugin contexts + registry; per-context API install; current-context→plugin-id via embedder slot; per-handler context entry in `dispatch_onframe`; per-context resolution + liveness in `frame_async_drain`; the naming rename in the injected prelude.
- `core/src/plugin.rs` (new) — `PluginInstance`, `PluginLedger`, registry, generation, teardown reverse-walk, liveness predicate (V8-free logic unit-tested).
- `core/src/loader.rs` (new) — `.s2sp` zip read (in-memory), manifest parse/validate, the `/plugins` poll-watch, load/unload/reload orchestration.
- `core/src/ffi.rs` + `shim/include/s2script_core.h` + `shim/src/s2script_mm.cpp` — pass the plugins-dir path; **remove** the baked demo `eval`/`load_cs2`. `core/src/lib.rs` — `mod plugin; mod loader;`.
- `tools/s2script-build/` (new) — the esbuild-based `s2script build` CLI.
- `packages/std/`, `packages/cs2/` (new) — minimal author-time type-stub packages for the injected API.
- `examples/demo-plugin/` (new) — the demo TS plugin.
- README (modify) — the build/drop/reload/delete runbook + Slice-4 acceptance. `CLAUDE.md` — update "Current state".
- Sniper build + Docker live gate + `scripts/rcon.py` reused.

## 13. Open items to validate during implementation

- **The CJS `require`/`module` eval wrapper in bare V8:** confirm the `(function(require, module, exports){…})` wrap + `s2require` mapping + `module.exports` capture works with an esbuild `--format=cjs --external:@s2script/*` bundle (a spike-able unit test in a fresh context).
- **Context embedder-data slot** API in rusty_v8 (`Context::set_slot`/`get_slot` or `set_embedder_data`/`get_embedder_data`) to stash the plugin id, and reading `get_current_context()` from a native scope.
- **Per-handler context entry** cost + correctness in `dispatch_onframe` (entering/exiting a `ContextScope` per handler within the existing `HandleScope`).
- **`Global<Context>` disposal** actually releasing the context's JS objects (drop the `Global`, and any `Global`s into that context — resolvers, handler funcs — must be dropped first, which the ledger teardown does).
- **The frame-drain microtask checkpoint across multiple live contexts** (a continuation runs in its own context; the liveness check gates whether we resolve into it at all).
- **In-memory zip reading** in Rust (a small zip crate, or shell out) and the `.s2sp` size/perf on the drain (only re-read on mtime change).
- Whether `s2script_core_load_cs2` / the Slice-3 `pawn.js` file-load path is subsumed by `Pawn` becoming the injected `@s2script/cs2` API (likely yes — `pawn.js`'s logic moves into the per-context cs2 injection).
