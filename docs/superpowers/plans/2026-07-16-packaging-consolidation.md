# Part C — @s2script/* → one `@s2script/sdk` package Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (- [ ]) syntax for tracking.

> **Naming pivot (2026-07-16):** this plan targets the scoped **`@s2script/sdk`** package (dir
> `packages/sdk/`, subpath imports `@s2script/sdk/<cap>`, CLI bin **`s2s`**), per the "⚠ Naming
> pivot" banner in the design spec. The unscoped `s2script` npm name is permanently unobtainable
> (npm name-similarity filter, 403 vs `rescript`); Part A (PR #50) is closed/void.

**Goal:** Consolidate the 29 always-present `@s2script/*` types-only builtin stubs (plus `globals`) into a single `@s2script/sdk` package with per-capability subpaths, so a builtin resolves as `@s2script/sdk/<cap>` (miss = TS2307) while presence-conditional inter-plugin interfaces keep the bare `@scope/<name>` package shape (miss = `any`) — making the `typecheck.ts:76` filter honest by shape, not by a disk guess.

**Architecture:** A dual-prefix transition. `core/src/v8host.rs s2require` learns to strip `@s2script/sdk/` alongside the existing `@s2script/` — **trying `@s2script/sdk/` FIRST (order is load-bearing: the shorter prefix also matches `@s2script/sdk/entity` and would strip to `sdk/entity` → `__s2pkg_sdk/entity` garbage)** — both map to `globalThis.__s2pkg_<cap>`. The four CLI type-resolution sites learn the new `packages/sdk/<cap>.d.ts` layout, consumers migrate in batches while both prefixes resolve, then the legacy `@s2script/<builtin>` prefix and `BUILTIN_MODULES` are removed once nothing uses them. The game package `@s2script/cs2` stays a separate scoped package throughout (game → core, never core → game), keeps riding the plain `@s2script/` strip, and moves from `pluginDependencies` to npm `dependencies` so the final typecheck filter is purely shape-based. The CLI + `s2s` bin arrive only in the final absorption PR; during phases 1–2 the build CLI is still `@s2script/cli` (bin `s2script`), unchanged.

**Tech Stack:** Rust (`s2script-core`, cdylib + V8), TypeScript CLI (esbuild/tsc, node `--test` `.mjs`), the games/cs2 prelude JS + its schemagen/navgen emitters, npm workspaces + changesets.

## Global Constraints

- **Ship work as a stack (Graphite), not a branch.** Small atomic PRs, one per reviewable change; always argue for more PRs, never fewer. Branch naming `packaging-consolidation/<terse-change>`.
- **Run the gate suite PER PR, not once at the top:** `make check-boundary`, `./scripts/check-plugins-typecheck.sh`, `cargo test -p s2script-core` (single-threaded — never pass `--test-threads`), `./scripts/check-schema-generated.sh`, `./scripts/check-nav-generated.sh`, `./scripts/check-events-generated.sh`, `./scripts/check-csitem-generated.sh`, `./scripts/test-boundary-nameleak.sh`. State which gate proves each PR.
- **Core is engine-generic; it NEVER imports `games/*`.** `s2require`'s dual-strip stays generic — no module list hardcoded; the strip ORDER (`@s2script/sdk/` before `@s2script/`) is the only new rule, and `@s2script/cs2` keeps riding the plain `@s2script/` strip like any capability.
- **Green CI does not prove the Phase-1 PR correct** — green is exactly the silent-hollowing signature. The passing canary (a deliberate builtin type error still FAILS the gate) is what proves resolution did not degrade to `any`.
- **The `games/cs2` `__s2require` literals are compiler-invisible.** A missed rename degrades to `pawn.origin → null` silently at runtime; the live Docker CS2 gate (`pawn.origin != null`), not CI, is that PR's proof.
- **Every count in this plan is illustrative — grep, never hardcode a count.** The literal off-by-one (an earlier pass said 9 vs 10) is the exact silent-failure class this migration worries about.
- **Versioning stays two axes:** a plugin declares `dependencies: { "@s2script/sdk": "^0.1.0" }` (types) AND keeps `s2script.apiVersion` (host ABI). Not collapsed. `@s2script/sdk` starts pre-1.0 (not API-frozen); consumers pin `^0.1.0` and re-pin as minors move. (The `s2script` manifest block name and `s2script.apiVersion`/`s2script.pluginDependencies` keys are the *manifest grammar* — they do NOT rename.)
- **Part A is DEAD (PR #50 closed; the unscoped `s2script` npm name is permanently blocked).** Nothing pre-exists at `packages/sdk/` — Phase 1 (PR C1) CREATES the package from scratch, **types-only (no bin)**. The CLI + `s2s` bin arrive in the final absorption PR. Cold-start usage after absorption: `npx @s2script/sdk build` — **not** `npx s2s`, which resolves the unrelated existing `s2s@0.20.1` package; installed, the command is `s2s build`.

---

## Established facts (verified against the code — do not re-derive)

- `s2require` (`core/src/v8host.rs:4050`, strip at `:4065`): `name.strip_prefix("@s2script/")` → `format!("__s2pkg_{}", rest)` → `globalThis.__s2pkg_<rest>`. Generic, no module list. `@s2script/cs2 → __s2pkg_cs2` rides it. **The dual-strip must try `@s2script/sdk/` FIRST** — the shorter `@s2script/` also matches `@s2script/sdk/entity` and would strip to `sdk/entity` → `__s2pkg_sdk/entity` garbage.
- `BUILTIN_MODULES` (`core/src/loader.rs:78`) + `is_builtin_module` (`:88`); two call sites `imports_from_manifest` (`:121`, `:125`), both `continue` (skip builtins from the ledger). Manifest carries only `pluginDependencies`/`optionalPluginDependencies`; npm `dependencies` never reach it.
- `build.ts:78-82` esbuild `external` = `["@s2script/*", ...pluginDependencies keys, ...optionalPluginDependencies keys]`. **The `@s2script/*` wildcard already covers `@s2script/sdk/entity` — esbuild external wildcards match across `/` — so NO new external pattern is needed.** (The earlier draft's "add `s2script/*`" step died with the unscoped name.)
- `typecheck.ts` four resolution sites: `isBuiltinOnDisk` (`:60-62`, `@s2script/`-prefixed + `existsSync(packagesDir/<name>/index.d.ts)`), the filter (`:76`), `paths: { "@s2script/*": ["*/index.d.ts"] }` (`:87`), globals rootName `join(packagesDir, "globals", "globals.d.ts")` (`:91`). `packagesDir` is passed explicitly by `check-plugins-typecheck.sh` (`{ packagesDir: 'packages' }`) — `resolvePackagesDir`/`isPackagesDir` only run when it is omitted.
- `packages-resolve.ts`: `isPackagesDir` sniffs `globals/globals.d.ts` / `entity/index.d.ts` / …; error text names `@s2script/globals`.
- `tsconfig.base.json:12` — editor twin `"@s2script/*": ["*/index.d.ts"]` (CLI does NOT read it; editor-only).
- The 29 builtin stub dirs = `ls packages/` minus `cli`, `cs2`, `globals`. Cross-`.d.ts` imports to rewrite (non-cs2): `grep -rnE '^\s*(import|export).*from "@s2script/' packages/*/index.d.ts` excluding `packages/cs2` — currently trace ×2, chat ×1, usercmd ×2, damage ×1, sound ×1, entity ×1, cookies ×1 (grep-derived; the cs2 ones move in Phase 3).
- The 10 runtime `__s2require` literals: hand-written in `games/cs2/js/pawn.js` (lines 7, 8, 283, 398, 830 — one embedded in the `__s2pkg_cs2 =` assignment) and `weapon.js:59`; **generator-emitted** in `packages/cli/src/schemagen/emit-js.ts:14` (→ `schema.generated.js:5-6`) and `packages/cli/src/navgen/emit-js.ts:35` (→ `nav.generated.js:4-5`). The generated ones MUST be renamed in the emitter + regenerated, or `check-schema-generated.sh`/`check-nav-generated.sh` fail.
- `packages/cli/test/schema-runtime.test.mjs` stubs `__s2require` at **8 sites** (grep-verified: lines 26, 58, 80, 106, 133, 162, 201, 231), each keyed on the EXACT strings `"@s2script/entity"` / `"@s2script/math"` (one site uses `name ===` instead of `n ===`). After the cs2-literals rename, pawn.js/schema.generated.js call `@s2script/sdk/entity`/`@s2script/sdk/math` — the stubs must accept both spellings or the harness returns `null` and the test behavior shifts.
- `@s2script/cs2` (`packages/cs2/package.json`) pins exact stub versions (`@s2script/entity: 0.3.0`, `math`, `trace`, `events`) and its `index.d.ts` imports them; it is declared in `pluginDependencies` by `plugins/zones` + `examples/{demo-plugin,entref-producer,zones-consumer-demo}`.
- Root `package.json` is `name: "s2script"` (`private: true`) and **STAYS that way** — it is never published, and the unscoped npm name is unobtainable anyway. The former root-rename PR is dropped.

---

## PR C1 (Phase 1): `packaging-consolidation/dual-resolve` — create the package + dual-resolve

The critical-trap PR. The `.d.ts` files physically move, so the gate's resolution sites **must move with them in this same commit** — otherwise a plugin that declares builtins falls through `isBuiltinOnDisk` into the ambient `any` stub and typechecks **green**: a silent hollowing of the 5E.1 gate that CI cannot catch. No consumer changes here — fully backward-compatible (both `@s2script/entity` and `@s2script/sdk/entity` resolve after this). The package created here is **types-only — no bin**; the CLI + `s2s` bin arrive in PR C5.

**Gate that proves this PR:** the no-degrade **canary** fixture (a deliberate builtin type error still FAILS) plus the parity/TS2307 fixtures in `packages/cli/test/typecheck.test.mjs`; the `s2require` dual-strip Rust unit test; then full suite green (`cargo test -p s2script-core`, `./scripts/check-plugins-typecheck.sh`, `make check-boundary`, the four `check-*-generated.sh`). **Green alone is insufficient — the passing canary is the proof.**

### Task C1.1 — Create `packages/sdk/` and move the 29 builtin `.d.ts` + `globals.d.ts` into it

**Files:**
- Create: `packages/sdk/package.json` (NEW — nothing pre-exists; Part A is dead), `packages/sdk/<cap>.d.ts` ×29 (git-moved from `packages/<cap>/index.d.ts`), `packages/sdk/globals.d.ts` (git-moved from `packages/globals/globals.d.ts`)

**Interfaces:**
- Consumes: the 29 stub `index.d.ts` bodies, `globals/globals.d.ts`
- Produces: `packages/sdk/entity.d.ts` … one flat `.d.ts` per capability; a NEW `@s2script/sdk` package.json with an `exports` subpath map

- [ ] **Step 1: Derive the exact capability set (do not hardcode).**
  ```bash
  cd /home/gkh/projects/s2script
  comm -23 <(ls packages/ | sort) <(printf 'cli\ncs2\nglobals\n' | sort)
  ```
  Expected: the 29 capability names (admin, bans, chat, clients, commands, config, console, cookies, damage, db, entity, events, frame, http, interfaces, math, menu, net, plugins, server, sound, timers, topmenu, trace, translations, usercmd, usermessages, votes, ws). Save to a shell var `CAPS`.

- [ ] **Step 2: Create the package skeleton — types-only, NO bin.** `mkdir -p packages/sdk`, then write `packages/sdk/package.json`:
  ```json
  {
    "name": "@s2script/sdk",
    "version": "0.0.0",
    "description": "s2script SDK — the builtin capability types, imported as @s2script/sdk/<cap>. The CLI (bin: s2s) joins in the absorb-cli PR.",
    "license": "MIT",
    "files": ["*.d.ts"]
  }
  ```
  (`version` becomes `0.1.0` via this PR's changeset — the first real release. No `bin`, no `dependencies` — those arrive in PR C5. The `exports` map is added in Step 5.)

- [ ] **Step 3: git-move each stub `.d.ts` to the flat layout.**
  ```bash
  for c in $(comm -23 <(ls packages/ | sort) <(printf 'cli\ncs2\nglobals\nsdk\n' | sort)); do
    git mv "packages/$c/index.d.ts" "packages/sdk/$c.d.ts"
  done
  git mv packages/globals/globals.d.ts packages/sdk/globals.d.ts
  ```
  The old `packages/<cap>/` dirs keep their `package.json`/`CHANGELOG.md` (hollow now — deleted in Phase 3). `packages/globals/` likewise. Do NOT delete them here.

- [ ] **Step 4: Verify the move.**
  ```bash
  ls packages/sdk/*.d.ts | wc -l          # expect 30 (29 caps + globals)
  test ! -e packages/entity/index.d.ts && echo "moved"   # expect: moved
  ```

- [ ] **Step 5: Add the `exports` subpath map to `packages/sdk/package.json`.** One entry per capability (types-condition only — these are types-only). The KEYS are package-relative subpaths (unchanged by the naming pivot); the import a consumer writes is `@s2script/sdk/<cap>`. Full map:
  ```json
  "exports": {
    "./admin": { "types": "./admin.d.ts" },
    "./bans": { "types": "./bans.d.ts" },
    "./chat": { "types": "./chat.d.ts" },
    "./clients": { "types": "./clients.d.ts" },
    "./commands": { "types": "./commands.d.ts" },
    "./config": { "types": "./config.d.ts" },
    "./console": { "types": "./console.d.ts" },
    "./cookies": { "types": "./cookies.d.ts" },
    "./damage": { "types": "./damage.d.ts" },
    "./db": { "types": "./db.d.ts" },
    "./entity": { "types": "./entity.d.ts" },
    "./events": { "types": "./events.d.ts" },
    "./frame": { "types": "./frame.d.ts" },
    "./http": { "types": "./http.d.ts" },
    "./interfaces": { "types": "./interfaces.d.ts" },
    "./math": { "types": "./math.d.ts" },
    "./menu": { "types": "./menu.d.ts" },
    "./net": { "types": "./net.d.ts" },
    "./plugins": { "types": "./plugins.d.ts" },
    "./server": { "types": "./server.d.ts" },
    "./sound": { "types": "./sound.d.ts" },
    "./timers": { "types": "./timers.d.ts" },
    "./topmenu": { "types": "./topmenu.d.ts" },
    "./trace": { "types": "./trace.d.ts" },
    "./translations": { "types": "./translations.d.ts" },
    "./usercmd": { "types": "./usercmd.d.ts" },
    "./usermessages": { "types": "./usermessages.d.ts" },
    "./votes": { "types": "./votes.d.ts" },
    "./ws": { "types": "./ws.d.ts" },
    "./package.json": "./package.json"
  }
  ```
  (No flat `.` barrel — subpaths only, §3. `globals.d.ts` is injected by the gate as a rootName, not imported, so it is NOT in the map.)

- [ ] **Step 6: Confirm the exports map is well-formed JSON.**
  ```bash
  node -e 'JSON.parse(require("fs").readFileSync("packages/sdk/package.json","utf8")); console.log("ok")'
  ```
  Expected: `ok`.

### Task C1.2 — Rewrite the cross-`.d.ts` imports to relative paths

**Files:** Modify: `packages/sdk/{trace,chat,usercmd,damage,sound,entity,cookies}.d.ts` (the exact set is grep-derived below, not hardcoded)

**Interfaces:**
- Consumes: internal `import type { … } from "@s2script/<cap>"` lines
- Produces: `import type { … } from "./<cap>"` (relative; the `exports` map gates only external subpath access, internal relatives resolve regardless)

- [ ] **Step 1: List the internal imports to rewrite (grep-derived).**
  ```bash
  grep -rnE '^\s*(import|export).*from "@s2script/' packages/sdk/*.d.ts
  ```
  Expected: only capability→capability imports (e.g. `trace.d.ts: from "@s2script/math"`, `from "@s2script/entity"`; `cookies.d.ts: from "@s2script/clients"`; etc.). NO cs2 (cs2 is not moved).

- [ ] **Step 2: Rewrite `@s2script/<cap>` → `./<cap>` inside `packages/sdk/*.d.ts`.**
  ```bash
  sed -i -E 's#(from ")@s2script/([a-z]+)(")#\1./\2\3#g' packages/sdk/*.d.ts
  ```

- [ ] **Step 3: Verify no `@s2script/` specifier survives inside the package.**
  ```bash
  grep -rnE 'from "@s2script/' packages/sdk/*.d.ts || echo "clean"
  ```
  Expected: `clean`.

### Task C1.3 — `s2require` gains `@s2script/sdk/` stripping (Rust, TDD)

**Files:**
- Modify: `core/src/v8host.rs:4065` (the single strip line in `fn s2require`)
- Test: `core/src/v8host.rs` (add a `#[test]` in the existing `mod tests`)

**Interfaces:**
- Consumes: `name: String` from `args.get(0)`
- Produces: `Option<&str> rest` — matches `@s2script/sdk/<rest>` (tried FIRST) OR `@s2script/<rest>` → `__s2pkg_<rest>`; bare `@s2script/sdk` (no cap) → null (falls to the plain strip → `__s2pkg_sdk`, which never exists)

- [ ] **Step 1: Write the failing test** in `core/src/v8host.rs` `mod tests` (uses the existing `eval_in_context_bool` helper; the prelude sets `__s2pkg_math`/`__s2pkg_entity` in any plugin context):
  ```rust
  #[test]
  fn s2require_dual_resolves_sdk_and_legacy_prefixes() {
      let _ = init(dummy_logger());
      create_plugin_context("dualpfx");
      // Both prefixes resolve the SAME capability global.
      assert!(eval_in_context_bool("dualpfx",
          r#"__s2require("@s2script/sdk/math") === __s2require("@s2script/math")"#),
          "@s2script/sdk/math must resolve to the same object as @s2script/math");
      assert!(eval_in_context_bool("dualpfx",
          r#"typeof __s2require("@s2script/sdk/entity").EntityRef === "function""#),
          "@s2script/sdk/entity must expose EntityRef");
      // Bare `@s2script/sdk` (no capability — the rejected flat barrel) → null at runtime:
      // it falls through to the plain `@s2script/` strip → `__s2pkg_sdk`, which never exists.
      assert!(eval_in_context_bool("dualpfx",
          r#"__s2require("@s2script/sdk") === null"#),
          "bare @s2script/sdk must resolve to null");
      // A non-s2script specifier is still null (handled by the JS interop shim).
      assert!(eval_in_context_bool("dualpfx",
          r#"__s2require("@other/x") === null"#));
      shutdown();
  }
  ```

- [ ] **Step 2: Run it, expect FAIL.**
  ```bash
  cargo test -p s2script-core s2require_dual_resolves
  ```
  Expected: FAIL — `@s2script/sdk/math` currently strips via the plain `@s2script/` prefix to `sdk/math` → looks up `__s2pkg_sdk/math` (never exists) → `null`; `null === __s2require("@s2script/math")` is false → panic on the first assert.

- [ ] **Step 3: Implement the dual strip** at `core/src/v8host.rs:4065`. Replace:
  ```rust
        let Some(rest) = name.strip_prefix("@s2script/") else { return };
  ```
  with:
  ```rust
        // Dual-prefix (packaging consolidation): a builtin resolves as BOTH the consolidated
        // `@s2script/sdk/<cap>` and the legacy `@s2script/<cap>` — both map to `__s2pkg_<cap>`.
        // ORDER IS LOAD-BEARING: the shorter `@s2script/` also matches `@s2script/sdk/entity`
        // and would strip to `sdk/entity` → `__s2pkg_sdk/entity` garbage — try `@s2script/sdk/`
        // FIRST. Bare `@s2script/sdk` (no capability) falls to the plain strip → `__s2pkg_sdk`,
        // which never exists → null (the flat barrel is rejected; the typecheck gate, not
        // s2require, enforces the namespace split). Still generic — no module list hardcoded;
        // `@s2script/cs2` keeps riding the plain `@s2script/` strip.
        let Some(rest) = name
            .strip_prefix("@s2script/sdk/")
            .or_else(|| name.strip_prefix("@s2script/"))
        else {
            return;
        };
  ```
  Also update the doc-comment above `fn s2require` (`:4042-4047`) to name both prefixes and the strip order.

- [ ] **Step 4: Run it, expect PASS.**
  ```bash
  cargo test -p s2script-core s2require_dual_resolves
  ```
  Expected: `test ... ok`.

### Task C1.4 — Move the CLI type-resolution sites (same commit, TDD via the canary)

**Files:**
- Modify: `packages/cli/src/typecheck/typecheck.ts:60-62` (`isBuiltinOnDisk`), `:87` (`paths`), `:91` (globals rootName)
- Modify: `packages/cli/src/packages-resolve.ts` (`isPackagesDir`, error text)
- Modify: `tsconfig.base.json:12` (editor twin)
- NOT modified: `packages/cli/src/build.ts:78-82` (esbuild `external`) — see Step 7, no change needed
- Test: `packages/cli/test/typecheck.test.mjs` (+ new fixtures under `packages/cli/test/fixtures/typecheck/`)

**Interfaces:**
- Consumes: `packagesDir` (= `packages` in the gate), a plugin's imports + declared deps
- Produces: builtins resolve at `packages/sdk/<cap>.d.ts` (new) OR `packages/<cap>/index.d.ts` (fallback, now serving only cs2); `@s2script/sdk/<cap>` resolves; `@s2script/sdk/<cap>` is already esbuild-external via the existing `@s2script/*` wildcard

- [ ] **Step 1: Write the failing canary + acceptance fixtures.** Create four fixtures. First, a shared fake `sdk` package the fixtures resolve against (mirrors `fake-packages/` but adds the new layout):
  - `packages/cli/test/fixtures/typecheck/fake-packages/sdk/entity.d.ts`:
    ```ts
    export interface EntityRef { readonly index: number; readonly serial: number; }
    export declare const Entity: { forRef(r: EntityRef): { health: number | null } | null };
    ```
  - `packages/cli/test/fixtures/typecheck/fake-packages/entity/index.d.ts` (legacy twin, so `@s2script/entity` still resolves during transition):
    ```ts
    export interface EntityRef { readonly index: number; readonly serial: number; }
    export declare const Entity: { forRef(r: EntityRef): { health: number | null } | null };
    ```
  - `packages/cli/test/fixtures/typecheck/fake-packages/sdk/globals.d.ts` (REQUIRED: the gate always injects a globals rootName via `existsSync(packagesDir/sdk/globals.d.ts)` → this path; without the file the rootName points at nothing and the program build is unreliable). Minimal ambient content is fine:
    ```ts
    declare global { const HookResult: { Continue: 0; Changed: 1; Handled: 2; Stop: 3 }; }
    export {};
    ```
  - Canary fixture `.../typecheck/canary-legacy/` (deliberate error against the legacy `@s2script/entity`):
    - `package.json`: `{ "name":"@fix/canary-legacy","version":"1.0.0","main":"src/plugin.ts","s2script":{"apiVersion":"1.x"},"private":true }`
    - `src/plugin.ts`:
      ```ts
      import { Entity, EntityRef } from "@s2script/entity";
      export function onLoad(r: EntityRef): void {
        const hp: number = Entity.forRef(r)!.health;   // TS2322: number | null → number
        console.log(hp);
      }
      ```
  - Canary fixture `.../typecheck/canary-sdk/` — identical but `import … from "@s2script/sdk/entity"`, name `@fix/canary-sdk`.
  - Acceptance fixture `.../typecheck/typo-builtin/` (a builtin TYPO must be TS2307, not `any`):
    - `package.json`: `{ "name":"@fix/typo-builtin","version":"1.0.0","main":"src/plugin.ts","s2script":{"apiVersion":"1.x"},"private":true }`
    - `src/plugin.ts`: `import { Entity } from "@s2script/sdk/frmae"; export const x = Entity;`
  - Acceptance fixture `.../typecheck/typo-interface/` (an unfetched interface typo must stay `any`):
    - `package.json`: `{ "name":"@fix/typo-interface","version":"1.0.0","main":"src/plugin.ts","s2script":{"apiVersion":"1.x","pluginDependencies":{"@community/mapchoser":"^1.0.0"}},"private":true }`
    - `src/plugin.ts`: `import x from "@community/mapchoser"; export const y = String(x);`

  Then add the tests to `packages/cli/test/typecheck.test.mjs`:
  ```js
  test("canary: a deliberate builtin type error still FAILS (legacy @s2script/entity)", () => {
    const r = typecheckPlugin(join(fixtures, "canary-legacy"), { packagesDir: fakePkgs });
    assert.equal(r.ok, false, "legacy canary must fail — green means resolution degraded to any");
    assert.ok(r.diagnostics.some((d) => d.code === 2322),
      "expected TS2322: " + JSON.stringify(r.diagnostics));
  });

  test("canary: a deliberate builtin type error still FAILS (consolidated @s2script/sdk/entity)", () => {
    const r = typecheckPlugin(join(fixtures, "canary-sdk"), { packagesDir: fakePkgs });
    assert.equal(r.ok, false, "sdk canary must fail — green means resolution degraded to any");
    assert.ok(r.diagnostics.some((d) => d.code === 2322),
      "expected TS2322: " + JSON.stringify(r.diagnostics));
  });

  test("acceptance: a builtin TYPO yields TS2307, not any", () => {
    const r = typecheckPlugin(join(fixtures, "typo-builtin"), { packagesDir: fakePkgs });
    assert.equal(r.ok, false);
    assert.ok(r.diagnostics.some((d) => d.code === 2307),
      "expected TS2307 for @s2script/sdk/frmae: " + JSON.stringify(r.diagnostics));
  });

  test("acceptance: an unfetched interface typo stays any (correctly indistinguishable)", () => {
    const r = typecheckPlugin(join(fixtures, "typo-interface"), { packagesDir: fakePkgs });
    assert.deepEqual(r.diagnostics, [], "interface typo must stub to any, not error: "
      + JSON.stringify(r.diagnostics));
    assert.equal(r.ok, true);
  });
  ```

- [ ] **Step 2: Run them, expect FAIL.**
  ```bash
  cd packages/cli && node --experimental-strip-types --no-warnings --test test/typecheck.test.mjs
  ```
  Expected: `canary-sdk` FAILS (its `@s2script/sdk/entity` import has no working mapping yet — the legacy `@s2script/*` path pattern sends it to `sdk/entity/index.d.ts`, which does not exist — so `some(2322)` is false); the legacy canary passes today (it already resolves via `entity/index.d.ts`). Run `typo-builtin`/`typo-interface` too and RECORD their pre-change outcome — they pin the acceptance behavior that must hold after Steps 3–8. The failing `canary-sdk` confirms the fixtures exercise the not-yet-built resolution.

- [ ] **Step 3: Implement — `isBuiltinOnDisk` checks both locations** (`typecheck.ts:60-62`). Replace:
  ```ts
    const isBuiltinOnDisk = (d: string): boolean =>
      d.startsWith("@s2script/") &&
      existsSync(join(packagesDir, d.slice("@s2script/".length), "index.d.ts"));
  ```
  with:
  ```ts
    // A builtin resolves either at the consolidated layout (packages/sdk/<cap>.d.ts) or the
    // legacy per-package layout (packages/<cap>/index.d.ts, still serving @s2script/cs2). During
    // the dual-prefix transition BOTH the consolidated `@s2script/sdk/<cap>` and the legacy
    // `@s2script/<cap>` spellings count as builtin-on-disk so a plugin that still DECLARES one in
    // pluginDependencies is filtered out of the ambient-stub list and resolves via `paths` below
    // (never degrades to `any`). ORDER IS LOAD-BEARING: check `@s2script/sdk/` before the shorter
    // `@s2script/`, which also matches and would yield the garbage cap `sdk/<cap>`.
    const capOf = (d: string): string | null =>
      d.startsWith("@s2script/sdk/") ? d.slice("@s2script/sdk/".length)
      : d.startsWith("@s2script/") ? d.slice("@s2script/".length)
      : null;
    const isBuiltinOnDisk = (d: string): boolean => {
      const cap = capOf(d);
      if (cap === null) return false;
      return existsSync(join(packagesDir, "sdk", cap + ".d.ts")) ||
             existsSync(join(packagesDir, cap, "index.d.ts"));
    };
  ```

- [ ] **Step 4: Implement — `paths` ordered fallback + new `@s2script/sdk/*` entry** (`typecheck.ts:87`). Replace:
  ```ts
      paths: { "@s2script/*": ["*/index.d.ts"] },
  ```
  with:
  ```ts
      paths: {
        // Consolidated layout first, legacy per-package second (the latter now serves only
        // @s2script/cs2, which is NOT moved). tsc picks the longest matching prefix, so the
        // @s2script/sdk/* pattern wins for sdk imports. Collapsed to @s2script/sdk/* +
        // @s2script/* (cs2 only) in Phase 3 once the legacy builtin dirs are deleted.
        "@s2script/sdk/*": ["sdk/*.d.ts"],
        "@s2script/*": ["sdk/*.d.ts", "*/index.d.ts"],
      },
  ```

- [ ] **Step 5: Implement — globals rootName both locations** (`typecheck.ts:91`). Replace:
  ```ts
    const rootNames = [entry, join(packagesDir, "globals", "globals.d.ts"), ...localDts];
  ```
  with:
  ```ts
    const globalsDts = existsSync(join(packagesDir, "sdk", "globals.d.ts"))
      ? join(packagesDir, "sdk", "globals.d.ts")
      : join(packagesDir, "globals", "globals.d.ts");
    const rootNames = [entry, globalsDts, ...localDts];
  ```

- [ ] **Step 6: Implement — `packages-resolve.ts` learns the consolidated shape.** In `isPackagesDir`, add the new layout as a recognized shape; in the throw text, drop the `@s2script/globals` wording:
  ```ts
  export function isPackagesDir(dir: string): boolean {
    const abs = resolve(dir);
    return (
      existsSync(join(abs, "sdk", "globals.d.ts")) ||     // consolidated layout
      existsSync(join(abs, "sdk", "entity.d.ts")) ||      // consolidated layout
      existsSync(join(abs, "globals", "globals.d.ts")) || // legacy per-package layout
      existsSync(join(abs, "entity", "index.d.ts")) ||
      existsSync(join(abs, "frame", "index.d.ts")) ||
      existsSync(join(abs, "commands", "index.d.ts"))
    );
  }
  ```
  And the final `throw` message body:
  ```ts
    throw new Error(
      "cannot resolve @s2script/sdk/* types: no packages dir found.\n" +
        "  Install `@s2script/sdk` in the plugin (npm i -D @s2script/sdk),\n" +
        "  or set S2SCRIPT_PACKAGES_DIR / pass --packages-dir."
    );
  ```

- [ ] **Step 7: esbuild `external` — NO change needed (verify, don't edit).** `build.ts:78-82` already lists `"@s2script/*"`, and esbuild external wildcards match across `/` — so `@s2script/sdk/entity` is already external. (The earlier draft added `"s2script/*"` here for the unscoped subpath; that name is dead and the step is dropped.) Confirm with a one-liner rather than trusting this note:
  ```bash
  node -e 'const { buildSync } = require("packages/cli/node_modules/esbuild");
  const r = buildSync({ stdin: { contents: `import "@s2script/sdk/entity";` },
    bundle: true, write: false, external: ["@s2script/*"], format: "esm" });
  console.log(r.outputFiles[0].text.includes("@s2script/sdk/entity") ? "external-ok" : "BUNDLED");'
  ```
  Expected: `external-ok`. If it prints `BUNDLED`, stop — the wildcard assumption is wrong and `"@s2script/sdk/*"` must be added to the `external` array after all.

- [ ] **Step 8: Implement — editor twin** (`tsconfig.base.json:12`):
  ```json
      "paths": { "@s2script/sdk/*": ["sdk/*.d.ts"], "@s2script/*": ["sdk/*.d.ts", "*/index.d.ts"] },
  ```

- [ ] **Step 9: Rebuild the CLI (the generated-checks and gate import the built dist) and run the tests, expect PASS.**
  ```bash
  ( cd packages/cli && node build.mjs >/dev/null )
  cd packages/cli && node --experimental-strip-types --no-warnings --test test/typecheck.test.mjs
  ```
  Expected: all four new tests plus the two existing ones pass. **The passing `canary-sdk` + `canary-legacy` is the load-bearing proof this PR did not hollow the gate.**

- [ ] **Step 10: Run the full gate suite for this PR.**
  ```bash
  cd /home/gkh/projects/s2script
  cargo test -p s2script-core
  ./scripts/check-plugins-typecheck.sh
  make check-boundary
  ./scripts/check-schema-generated.sh && ./scripts/check-nav-generated.sh
  ./scripts/check-events-generated.sh && ./scripts/check-csitem-generated.sh
  ```
  Expected: all green. `check-plugins-typecheck.sh` proves every existing plugin still resolves its `@s2script/*` builtins via the moved layout (backward-compatible).

### Task C1.5 — Changeset + commit

- [ ] **Step 1: Add a changeset** (packages/* changed):
  ```bash
  npm run changeset   # minor bump `@s2script/sdk` 0.0.0 → 0.1.0; note "first release: consolidated builtin .d.ts + dual-resolve"
  ```
  **Version note (load-bearing):** there is no Part-A placeholder — this changeset CREATES the first published version of `@s2script/sdk` (a MAJOR-less minor: 0.0.0 → **0.1.0**). It starts **pre-1.0** — the API is not frozen yet, so minors are allowed to break and consumers re-pin. Phase-2 consumers pin `"@s2script/sdk": "^0.1.0"` (npm 0.x caret = `>=0.1.0 <0.2.0`), which only resolves against a published `>=0.1.0`. In-repo the typecheck gate resolves `@s2script/sdk/*` via `paths` (not `node_modules`), so the version pin is cosmetic for the gate; it becomes load-bearing at publish and for any out-of-monorepo consumer. Keep `apiVersion` a separate axis (§Global Constraints). At migration's end (publish time): `npm deprecate` the 29 `@s2script/*` capability stubs → `@s2script/sdk` (see PR C3) and `@s2script/cli` → `@s2script/sdk` (see PR C5); keep the `@s2script` scope owned as brand protection.

- [ ] **Step 2: Commit the whole PR as one atomic change** (branch `packaging-consolidation/dual-resolve`, tracked on main in a worktree per the slice cadence):
  ```bash
  gt track -p main   # only if the worktree branch starts untracked
  git add -A
  gt create packaging-consolidation/dual-resolve -m "consolidation: create @s2script/sdk + dual-resolve builtins

Create packages/sdk/ (types-only, no bin yet), move the 29 builtin .d.ts + globals
into it, teach s2require to strip @s2script/sdk/ BEFORE @s2script/ (order is
load-bearing), and move the CLI type-resolution sites in lockstep so builtins
resolve at the new location while @s2script/cs2 resolves at the old one. Ships the
no-degrade canary: a deliberate builtin type error still FAILS the gate (green CI
is the silent-failure signature).

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
  ```

---

## PR C2..Cn (Phase 2): `packaging-consolidation/migrate-<batch>` — migrate consumers in batches

Rewrite `@s2script/<builtin>` → `@s2script/sdk/<builtin>` in consumer sources and move builtins **and `@s2script/cs2`** from `s2script.pluginDependencies` to npm `dependencies` (Fork 1). Each PR is atomic because both prefixes still resolve (PR C1). **The `@s2script/cs2` import specifier is unchanged — it stays its own scoped package; only its declaration map entry moves.** One PR per plugin (or a small batch); **always argue for more PRs, never fewer**. Throughout Phase 2 the build CLI is still `@s2script/cli` (bin `s2script`) — unchanged until PR C5.

**Gate that proves each batch PR:** `./scripts/check-plugins-typecheck.sh` green (every migrated plugin resolves under `@s2script/sdk/*`), plus `make check-boundary` and `cargo test -p s2script-core` (unchanged, must stay green).

### Task C2.0 — Generate the batch list (grep-derived, not hardcoded)

- [ ] **Step 1: Enumerate the consumer files and dirs to migrate.**
  ```bash
  cd /home/gkh/projects/s2script
  # Files importing a builtin @s2script/<cap> (cs2 excluded — its specifier stays as-is):
  grep -rlE 'from "@s2script/(admin|bans|chat|clients|commands|config|console|cookies|damage|db|entity|events|frame|http|interfaces|math|menu|net|plugins|server|sound|timers|topmenu|trace|translations|usercmd|usermessages|votes|ws)"' plugins/ examples/ disabled/
  # Package dirs (unique parents) → each becomes one PR (or group 2-3 trivial ones):
  ```
  The batch COUNT is whatever this grep yields — do not assume a number. Group into PRs: one per plugin dir; a few tiny sibling example dirs may share a PR.

### Task C2.template — Per-plugin migration (verified against `plugins/basecommands` and `plugins/zones`)

Two consumer shapes exist; the template handles both:
- **Shape A (no builtin `pluginDependencies`)** — e.g. `plugins/basecommands` (imports builtins, declares none). Only the import specifiers change; add an honest `dependencies` block for publish-readiness.
- **Shape B (builtins + cs2 in `pluginDependencies`)** — e.g. `plugins/zones`. Rewrite import specifiers AND move the builtin/cs2 entries out of `s2script.pluginDependencies` into npm `dependencies`; keep any genuine inter-plugin `@scope/*` interface deps in `pluginDependencies`.

**Files (one plugin, e.g. `plugins/zones`):**
- Modify: `plugins/zones/src/*.ts` (import specifiers), `plugins/zones/package.json` (dep maps)

**Interfaces:**
- Consumes: `@s2script/<builtin>` imports, `s2script.pluginDependencies`
- Produces: `@s2script/sdk/<builtin>` imports; builtins collapse to one `dependencies: { "@s2script/sdk": "^0.1.0" }`; `@s2script/cs2` → `dependencies` (its own scoped key kept); interface deps stay in `pluginDependencies`

- [ ] **Step 1: Rewrite builtin import specifiers in this plugin's sources** (leave `@s2script/cs2` alone):
  ```bash
  P=plugins/zones
  grep -rlE 'from "@s2script/(admin|bans|chat|clients|commands|config|console|cookies|damage|db|entity|events|frame|http|interfaces|math|menu|net|plugins|server|sound|timers|topmenu|trace|translations|usercmd|usermessages|votes|ws)"' "$P/src" \
    | xargs sed -i -E 's#(from ")@s2script/(admin|bans|chat|clients|commands|config|console|cookies|damage|db|entity|events|frame|http|interfaces|math|menu|net|plugins|server|sound|timers|topmenu|trace|translations|usercmd|usermessages|votes|ws)(")#\1@s2script/sdk/\2\3#g'
  ```

- [ ] **Step 2: Verify no legacy builtin specifier survives in this plugin** (only `@s2script/sdk/<cap>` and, if any, `@s2script/cs2` remain):
  ```bash
  grep -rnE 'from "@s2script/(admin|bans|chat|clients|commands|config|console|cookies|damage|db|entity|events|frame|http|interfaces|math|menu|net|plugins|server|sound|timers|topmenu|trace|translations|usercmd|usermessages|votes|ws)"' "$P/src" || echo "clean"
  ```
  Expected: `clean`.

- [ ] **Step 3 (Shape B only): move builtins + cs2 in `package.json`.** Edit `plugins/zones/package.json`: delete the builtin `@s2script/<cap>` and `@s2script/cs2` keys from `s2script.pluginDependencies`; add a top-level `dependencies` block. For `zones`, `pluginDependencies` currently holds 10 builtins + `@s2script/cs2` and NO non-builtin interface, so the whole map moves and `s2script.pluginDependencies` is removed:
  ```json
  {
    "name": "@s2script/zones",
    "version": "0.3.0",
    "private": true,
    "main": "src/plugin.ts",
    "types": "api.d.ts",
    "dependencies": {
      "@s2script/sdk": "^0.1.0",
      "@s2script/cs2": "^0.5.0"
    },
    "s2script": {
      "apiVersion": "1.x",
      "publishes": "self"
    }
  }
  ```
  (If a plugin also declares a genuine `@community/x` or `@s2script/<publishedInterface>` interface dep, KEEP it in `s2script.pluginDependencies` — only builtins + cs2 move.)

- [ ] **Step 3 (Shape A only): add the honest `dependencies` block.** For a plugin like `basecommands` that declares no `pluginDependencies`, add (cosmetic in-repo since it is `private`, but correct for publish + it is what lets Phase-3's shape-based filter stay honest):
  ```json
    "dependencies": { "@s2script/sdk": "^0.1.0", "@s2script/cs2": "^0.5.0" }
  ```
  (Include `@s2script/cs2` only if the plugin imports it.)

- [ ] **Step 4: Typecheck just this plugin, expect PASS.**
  ```bash
  cd /home/gkh/projects/s2script && node -e '
    import("./packages/cli/src/typecheck/typecheck.ts").then(({typecheckPlugin, formatDiagnostics}) => {
      const r = typecheckPlugin("plugins/zones", { packagesDir: "packages" });
      if (!r.ok) { console.error(formatDiagnostics(r.diagnostics)); process.exit(1); }
      console.log("PASS");
    });' --input-type=module 2>/dev/null || \
  node --experimental-strip-types --no-warnings -e '
    import("./packages/cli/src/typecheck/typecheck.ts").then(({typecheckPlugin, formatDiagnostics}) => {
      const r = typecheckPlugin("plugins/zones", { packagesDir: "packages" });
      if (!r.ok) { console.error(formatDiagnostics(r.diagnostics)); process.exit(1); }
      console.log("PASS");
    });'
  ```
  Expected: `PASS`.

- [ ] **Step 5: Run the plugin-typecheck gate + boundary for the batch.**
  ```bash
  ./scripts/check-plugins-typecheck.sh && make check-boundary
  ```
  Expected: green.

- [ ] **Step 6: Commit this batch.**
  ```bash
  git add -A
  gt create packaging-consolidation/migrate-zones -m "consolidation: migrate zones to @s2script/sdk/* + npm deps

Rewrite @s2script/<builtin> → @s2script/sdk/<builtin>; move builtins and @s2script/cs2
from s2script.pluginDependencies to npm dependencies. Both prefixes still resolve,
so this is atomic.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
  ```

Repeat Task C2.template per plugin dir from the C2.0 batch list (grep-derived count).

---

## PR C-cs2lit (Phase 2): `packaging-consolidation/cs2-require-literals` — the games/cs2 literals

The 10 compiler-invisible `__s2require` literals. A miss degrades to `pawn.origin → null` **silently** — CI cannot catch it, so the live Docker CS2 gate is the proof. The generator-emitted literals must be renamed **in the emitter + regenerated**, or the codegen-freshness gates fail. This PR also MUST update the `schema-runtime.test.mjs` `__s2require` stubs (Step 5) — they key on the exact legacy specifier strings and would silently shift the test's behavior otherwise.

**Files:**
- Modify: `games/cs2/js/pawn.js` (5 literals: lines 7, 8, 283, 398, 830), `games/cs2/js/weapon.js` (line 59)
- Modify: `packages/cli/src/schemagen/emit-js.ts:14`, `packages/cli/src/navgen/emit-js.ts:35`
- Modify: `packages/cli/test/schema-runtime.test.mjs` (the 8 `__s2require` stub sites)
- Regenerate: `games/cs2/js/schema.generated.js`, `games/cs2/js/nav.generated.js`

**Interfaces:**
- Consumes: `__s2require("@s2script/<cap>")` runtime calls
- Produces: `__s2require("@s2script/sdk/<cap>")` — resolves via PR C1's dual-strip (both spellings work; this proves the consolidated one live)

**Gate that proves this PR:** `./scripts/check-schema-generated.sh` + `./scripts/check-nav-generated.sh` green (regeneration matched the committed output), the CLI test suite's failure count NOT increasing beyond the pre-existing 13, THEN the **live Docker CS2 gate**: load a plugin and assert `pawn.origin != null`. CI alone is insufficient.

- [ ] **Step 1: Confirm the exact literal set (grep, do not trust the count).**
  ```bash
  grep -rn '__s2require("@s2script/' games/cs2/js/ packages/cli/src/schemagen/emit-js.ts packages/cli/src/navgen/emit-js.ts
  ```
  Expected: pawn.js ×5 (incl. the one embedded in the `__s2pkg_cs2 =` assignment at line 830), weapon.js ×1 (hand-written), schemagen/emit-js.ts ×1, navgen/emit-js.ts ×1 (emitters). 10 runtime literals total (2 emitters produce 4 generated literals). None reference `@s2script/cs2` (verified — the caps are entity/math/server/admin/events), so the blanket `[a-z]+` sed below is safe.

- [ ] **Step 2: Rewrite the hand-written literals in pawn.js + weapon.js.**
  ```bash
  sed -i -E 's#(__s2require\(")@s2script/([a-z]+)("\))#\1@s2script/sdk/\2\3#g' games/cs2/js/pawn.js games/cs2/js/weapon.js
  grep -rn '__s2require("@s2script/' games/cs2/js/pawn.js games/cs2/js/weapon.js | grep -v '@s2script/sdk/' || echo "clean"
  ```
  Expected: `clean`.

- [ ] **Step 3: Rewrite the emitters.** `packages/cli/src/schemagen/emit-js.ts:14` and `packages/cli/src/navgen/emit-js.ts:35` both emit ``` `  var ${cls} = __s2require("@s2script/math").${cls};` ```. Change `@s2script/math` → `@s2script/sdk/math` in both.

- [ ] **Step 4: Rebuild the CLI and regenerate the generated JS.**
  ```bash
  ( cd packages/cli && node build.mjs >/dev/null )
  node packages/cli/dist/cli.js gen-schema     # rewrites games/cs2/js/schema.generated.js
  node packages/cli/dist/cli.js gen-nav        # rewrites games/cs2/js/nav.generated.js
  grep -rn '__s2require("@s2script/' games/cs2/js/*.generated.js | grep -v '@s2script/sdk/' || echo "clean"
  ```
  Expected: `clean`; the two generated files now carry `@s2script/sdk/math`.
  (Determine the exact regen subcommand from `check-schema-generated.sh`/`check-nav-generated.sh` — they invoke `gen-schema --check` / `gen-nav --check`; drop `--check` to write.)

- [ ] **Step 5 (REQUIRED): Update the `schema-runtime.test.mjs` `__s2require` stubs.** The test harness stubs `__s2require` for ONLY the exact strings `"@s2script/entity"` / `"@s2script/math"` — verify with grep, then teach every stub site the new specifiers (after this PR, pawn.js/schema.generated.js call `@s2script/sdk/entity` / `@s2script/sdk/math`; an unupdated stub returns `null` and the test's behavior shifts silently):
  ```bash
  grep -n '__s2require' packages/cli/test/schema-runtime.test.mjs
  # Expected: 8 stub arrows keyed on n === "@s2script/entity" / "@s2script/math"
  # (one site uses `name ===`). Rewrite each to accept BOTH spellings:
  sed -i -E 's#(n(ame)?) === "@s2script/(entity|math)"#(\1 === "@s2script/\3" || \1 === "@s2script/sdk/\3")#g' \
    packages/cli/test/schema-runtime.test.mjs
  grep -n '@s2script/sdk/' packages/cli/test/schema-runtime.test.mjs | wc -l   # expect ≥ 8 rewritten sites
  ```
  Then re-run the FULL CLI suite and confirm the failure count does not INCREASE beyond the pre-existing **13** (7 schema-runtime + 6 player-identity):
  ```bash
  cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs 2>&1 | tail -20
  ```
  Expected: `# fail` ≤ 13, and the failing set is the same pre-existing set — no NEW failures introduced by the literal rename.

- [ ] **Step 6: Run the codegen-freshness gates, expect PASS.**
  ```bash
  ./scripts/check-schema-generated.sh && ./scripts/check-nav-generated.sh
  ```
  Expected: `PASS: schema codegen is up to date` / `PASS: nav codegen is up to date` (regeneration matched the committed output — no stray diff).

- [ ] **Step 7: Package + deploy to the Docker CS2 dev server and run the live gate.** This is the mechanism-proof for the invisible literals.
  ```bash
  make docker-test    # if not already up
  # (re)package the addon so games/cs2/js/*.js reach dist; the prelude is a CONCAT — do NOT raw-cp a single file
  ./scripts/package-addon.sh
  docker compose -f docker/docker-compose.yml restart cs2   # NOT --force-recreate (resets gameinfo.gi)
  # arm after the boot window, then:
  python3 scripts/rcon.py "sm_pawntest_or_a_plugin_that_reads_pawn_origin"
  ```
  Expected: a plugin reading `pawn.origin` returns a real Vector (not `null`). A `null` means a missed literal — the silent-failure this PR guards. Follow the live-gate cadence (arm after boot; deterministic `round_start` if a player is needed).

- [ ] **Step 8: Commit.**
  ```bash
  git add -A
  gt create packaging-consolidation/cs2-require-literals -m "consolidation: rename games/cs2 __s2require literals to @s2script/sdk/*

10 compiler-invisible runtime literals (pawn.js ×5, weapon.js ×1, + schemagen/navgen
emitters, regenerated) + the 8 schema-runtime.test.mjs stub sites. Dual-resolve keeps
both spellings working; the live Docker CS2 gate (pawn.origin != null) is the proof —
CI cannot see these.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
  ```

---

## PR C3 (Phase 3): `packaging-consolidation/remove-legacy-prefix` — delete the old surface

Nothing imports `@s2script/<builtin>` anymore (Phase 2). Remove the legacy stub packages, `BUILTIN_MODULES`, and narrow the typecheck filter to the honest shape-based rule.

**Files:**
- Delete: `packages/<cap>/` ×29 (the hollow dirs) + `packages/globals/`
- Modify: `core/src/loader.rs:78-129` (delete `BUILTIN_MODULES`, `is_builtin_module`, its two `continue` call sites)
- Modify: `packages/cli/src/typecheck/typecheck.ts` (narrow filter; collapse fallback paths)
- Modify: `packages/cli/src/packages-resolve.ts` (drop legacy shape); `tsconfig.base.json:12`
- Test: `core/src/loader.rs` `mod tests` (legacy-manifest load-side test); `packages/cli/test/typecheck.test.mjs` (narrowed-filter test)

**Interfaces:**
- Consumes: manifests with `@s2script/*` builtins possibly still in `pluginDependencies` (legacy `.s2sp`)
- Produces: `imports_from_manifest` no longer special-cases builtins; an `@s2script/sdk/<cap>` name never stubs (resolve-or-TS2307); `pluginDependencies` entries not locally resolvable stub to `any`

**Gate that proves this PR:** a grep proves zero `@s2script/<builtin>` imports survive; `cargo test -p s2script-core` (incl. the new legacy-load test); `./scripts/check-plugins-typecheck.sh`; the narrowed-filter typecheck test.

### Task C3.1 — Grep-gate: zero legacy builtin imports survive

- [ ] **Step 1: Prove nothing imports a builtin under the legacy prefix.**
  ```bash
  cd /home/gkh/projects/s2script
  grep -rnE 'from "@s2script/(admin|bans|chat|clients|commands|config|console|cookies|damage|db|entity|events|frame|http|interfaces|math|menu|net|plugins|server|sound|timers|topmenu|trace|translations|usercmd|usermessages|votes|ws)"' \
    plugins/ examples/ disabled/ games/ && echo "FAIL: legacy imports remain" || echo "PASS: none"
  grep -rn '__s2require("@s2script/' games/cs2/js/ | grep -v '@s2script/sdk/' && echo "FAIL" || echo "PASS: none"
  ```
  Expected: `PASS: none` for both. (Do NOT proceed until clean — a survivor here becomes a dangling import after deletion.)

### Task C3.2 — Delete the hollow stub packages + globals

- [ ] **Step 1: Delete the 29 legacy dirs + globals (grep-derived set).**
  ```bash
  for c in $(comm -23 <(ls packages/ | sort) <(printf 'cli\ncs2\nglobals\nsdk\n' | sort)); do
    git rm -r "packages/$c"
  done
  git rm -r packages/globals
  ls packages/   # expect exactly: cli, cs2, sdk (globals GONE)
  ```
  **Manual step at publish time:** `npm deprecate` each of the 29 published `@s2script/<cap>` capability stubs with `"Consolidated into @s2script/sdk — import @s2script/sdk/<cap>"`. The `@s2script` scope stays owned (brand protection).

### Task C3.3 — Delete `BUILTIN_MODULES` + its call sites (Rust)

- [ ] **Step 1: Delete `BUILTIN_MODULES` (`loader.rs:78-86`), `is_builtin_module` (`:88-90`), and the two `if is_builtin_module(name) { continue; }` guards in `imports_from_manifest` (`:121`, `:125`).** After: `imports_from_manifest` pushes every `pluginDependencies`/`optionalPluginDependencies` entry as a Hard/Optional import unconditionally:
  ```rust
  fn imports_from_manifest(m: &Manifest) -> Vec<(String, String, crate::interfaces::Kind)> {
      let mut out = Vec::new();
      for (name, range) in &m.plugin_dependencies {
          out.push((name.clone(), range.clone(), crate::interfaces::Kind::Hard));
      }
      for (name, range) in &m.optional_plugin_dependencies {
          out.push((name.clone(), range.clone(), crate::interfaces::Kind::Optional));
      }
      out
  }
  ```
  Update the fn doc-comment to drop the builtin-skip rationale.

- [ ] **Step 2: Write the load-side legacy-posture test** in `core/src/loader.rs` `mod tests` (a pre-migration manifest with builtins still in `pluginDependencies` — under their LEGACY `@s2script/<cap>` names, which is exactly what an old `.s2sp` carries — loads and runs post-deletion — phantom-lazy-hard-dep, §6.4). Uses the existing `make_test_s2sp` helper:
  ```rust
  #[test]
  fn legacy_manifest_with_builtins_in_plugin_deps_still_loads() {
      // A pre-consolidation .s2sp declares builtins as pluginDependencies. Post-BUILTIN_MODULES-deletion
      // these flow through as Hard imports with no producer — behaviorally benign: call_target_inner is
      // lazy (Unavailable at CALL time, never at load) and __s2require is prelude-first, so the phantom
      // is never called. The manifest must still parse and its imports flatten without panic.
      let bytes = make_test_s2sp(
          r#"{"id":"@legacy/plugin","version":"0.1.0","apiVersion":"1.x",
              "pluginDependencies":{"@s2script/entity":"^0.2.0","@s2script/math":"^0.1.0"}}"#,
          "module.exports.onLoad=()=>{};",
      );
      let (m, _js) = read_s2sp(&bytes).expect("legacy manifest parses");
      let imports = imports_from_manifest(&m);
      // Builtins are no longer skipped — they become phantom Hard deps (lazy, never called).
      assert_eq!(imports.len(), 2, "both builtin deps flow through post-deletion");
      assert!(imports.iter().all(|(_, _, k)| matches!(k, crate::interfaces::Kind::Hard)));
      assert!(imports.iter().any(|(n, _, _)| n == "@s2script/entity"));
  }
  ```

- [ ] **Step 3: Run the Rust tests, expect PASS.**
  ```bash
  cargo test -p s2script-core
  ```
  Expected: green, incl. `legacy_manifest_with_builtins_in_plugin_deps_still_loads`.

### Task C3.4 — Narrow the typecheck filter to the shape-based rule; collapse fallback paths

**Interfaces:**
- Consumes: a plugin's `pluginDependencies` keys
- Produces: `@s2script/sdk/*` and `@s2script/cs2` never stub (resolve or TS2307); only `pluginDependencies` entries that are not locally resolvable stub to `any`; the disk-existence guess is gone

- [ ] **Step 1: Write the narrowed-filter failing test** in `packages/cli/test/typecheck.test.mjs`. A plugin that (incorrectly, legacy-style) DECLARES `@s2script/sdk/frmae` in `pluginDependencies` must still get TS2307 — proving `@s2script/sdk/*` never stubs:
  - Fixture `.../typecheck/decl-builtin-typo/`:
    - `package.json`: `{ "name":"@fix/decl-builtin-typo","version":"1.0.0","main":"src/plugin.ts","s2script":{"apiVersion":"1.x","pluginDependencies":{"@s2script/sdk/frmae":"^1.0.0"}},"private":true }`
    - `src/plugin.ts`: `import { Entity } from "@s2script/sdk/frmae"; export const x = Entity;`
  - Test:
    ```js
    test("narrowed filter: a declared @s2script/sdk/* typo still yields TS2307 (never stubs)", () => {
      const r = typecheckPlugin(join(fixtures, "decl-builtin-typo"), { packagesDir: fakePkgs });
      assert.equal(r.ok, false);
      assert.ok(r.diagnostics.some((d) => d.code === 2307),
        "@s2script/sdk/* must resolve-or-error, never stub: " + JSON.stringify(r.diagnostics));
    });
    ```

- [ ] **Step 2: Run it, expect FAIL.** Before narrowing, the phase-1 filter's `!isBuiltinOnDisk(d)` leaves `@s2script/sdk/frmae` (a `pluginDependency`, not on disk) in the ambient-stub list → typed `any` → `r.ok === true`.
  ```bash
  cd packages/cli && node --experimental-strip-types --no-warnings --test test/typecheck.test.mjs
  ```
  Expected: the new test FAILS (`ok` is true).

- [ ] **Step 3: Implement the shape-based filter** in `typecheck.ts`. Replace `isBuiltinOnDisk`/`capOf` with a rule that keys on shape — `@s2script/sdk/*` and `@s2script/cs2` are always resolve-or-error (never stubbed); everything else falls through to the stub:
  ```ts
  // Shape-based (post-consolidation): builtins are `@s2script/sdk/<cap>` subpaths and the game
  // package is the separate scoped `@s2script/cs2` — both live in npm `dependencies` and resolve
  // via `paths` below (miss = TS2307, a real error). Only presence-conditional inter-plugin
  // interfaces (declared in pluginDependencies) stub to `any` until fetched. No disk guess —
  // the disk-existence check is gone (the finding fix).
  const isAlwaysResolved = (d: string): boolean =>
    d.startsWith("@s2script/sdk/") || d === "@s2script/cs2" || d.startsWith("@s2script/cs2/");
  ```
  and the filter (`:71-76`):
  ```ts
    const deps = [
      ...Object.keys(s2.pluginDependencies ?? {}),
      ...Object.keys(s2.optionalPluginDependencies ?? {}),
    ].filter((d) => !isAlwaysResolved(d) && !locallyDeclared.has(d));
  ```

- [ ] **Step 4: Collapse the phase-1 fallback paths** (`typecheck.ts:87`) — the legacy `*/index.d.ts` builtin dirs are deleted; only `@s2script/cs2` needs the per-package form:
  ```ts
      paths: {
        "@s2script/sdk/*": ["sdk/*.d.ts"],
        "@s2script/*": ["*/index.d.ts"],   // now serves only @s2script/cs2 (packages/cs2/index.d.ts)
      },
  ```
  and simplify the globals rootName + `isBuiltinOnDisk`-both branches removed. The globals rootName is now unconditionally the consolidated path:
  ```ts
    const rootNames = [entry, join(packagesDir, "sdk", "globals.d.ts"), ...localDts];
  ```

- [ ] **Step 5: Collapse `packages-resolve.ts` to the consolidated shape only** (drop the legacy `entity/index.d.ts` etc. sniff — those dirs are gone):
  ```ts
  export function isPackagesDir(dir: string): boolean {
    const abs = resolve(dir);
    return (
      existsSync(join(abs, "sdk", "globals.d.ts")) ||
      existsSync(join(abs, "sdk", "entity.d.ts"))
    );
  }
  ```
  and `tsconfig.base.json:12`:
  ```json
      "paths": { "@s2script/sdk/*": ["sdk/*.d.ts"], "@s2script/*": ["*/index.d.ts"] },
  ```

- [ ] **Step 6: Rebuild CLI, run the typecheck tests + the plugin gate, expect PASS.**
  ```bash
  ( cd packages/cli && node build.mjs >/dev/null )
  cd packages/cli && node --experimental-strip-types --no-warnings --test test/typecheck.test.mjs
  cd /home/gkh/projects/s2script && ./scripts/check-plugins-typecheck.sh
  ```
  Expected: all typecheck tests green (incl. `decl-builtin-typo` now TS2307, and the phase-1 `typo-interface` still `any`); every migrated plugin still resolves.

- [ ] **Step 7: Full gate suite + commit.**
  ```bash
  cargo test -p s2script-core && make check-boundary
  ./scripts/check-schema-generated.sh && ./scripts/check-nav-generated.sh
  ./scripts/check-events-generated.sh && ./scripts/check-csitem-generated.sh
  ./scripts/test-boundary-nameleak.sh
  git add -A
  gt create packaging-consolidation/remove-legacy-prefix -m "consolidation: delete legacy builtin stubs + BUILTIN_MODULES; honest filter

Delete the 29 hollow stub dirs + packages/globals; delete BUILTIN_MODULES and its
two imports_from_manifest call sites; narrow the typecheck filter to the shape-based
rule (@s2script/sdk/* + @s2script/cs2 resolve-or-error, pluginDependencies stub-until-
fetched) — the disk guess that made typecheck.ts:76 unfixable is gone. Legacy .s2sp
load-side test pins the phantom-lazy-hard-dep posture.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
  ```

---

## PR C4 (Phase 3): `packaging-consolidation/republish-cs2` — re-point the game package

`@s2script/cs2`'s own `package.json` still pins deleted stub versions (`@s2script/entity: 0.3.0`, …) and its `index.d.ts` still imports `@s2script/entity`/`math`/`trace`/`events`. Re-point both at `@s2script/sdk`.

**Files:**
- Modify: `packages/cs2/package.json` (`dependencies`), `packages/cs2/index.d.ts` (the cross-imports, grep-derived)

**Interfaces:**
- Consumes: `@s2script/cs2`'s `.d.ts` importing builtins
- Produces: `import type { EntityRef } from "@s2script/sdk/entity"` etc.; `dependencies: { "@s2script/sdk": "^0.1.0" }`

**Gate that proves this PR:** `./scripts/check-plugins-typecheck.sh` green (the `@s2script/cs2` consumers still resolve `Player`/`Pawn` through the re-pointed package); `cargo test -p s2script-core` unchanged.

- [ ] **Step 1: Re-point `packages/cs2/package.json` deps.** Replace the four exact stub pins with a single `@s2script/sdk`:
  ```json
    "dependencies": { "@s2script/sdk": "^0.1.0" },
  ```

- [ ] **Step 2: Rewrite the cross-imports in `packages/cs2/index.d.ts`** (grep-derived — currently lines 8, 9, 10, 15, 136, 137, 138):
  ```bash
  grep -nE 'from "@s2script/' packages/cs2/index.d.ts   # confirm the set
  sed -i -E 's#(from ")@s2script/(entity|math|trace|events)(")#\1@s2script/sdk/\2\3#g' packages/cs2/index.d.ts
  grep -nE 'from "@s2script/' packages/cs2/index.d.ts | grep -v '@s2script/sdk/' || echo "clean"
  ```
  Expected: `clean`. (`@s2script/cs2`'s own name is unchanged — only the builtins it imports move to `@s2script/sdk/*`.)

  (`@s2script/cli` is left alone here — its physical absorption into `@s2script/sdk` and deprecation happen in **PR C5**, the final PR. Until C5 lands, the build CLI remains `@s2script/cli` with its `s2script` bin.)

- [ ] **Step 4: Typecheck a cs2 consumer + full plugin gate, expect PASS.**
  ```bash
  ./scripts/check-plugins-typecheck.sh
  ```
  Expected: green — `plugins/basecommands` etc. resolve `Player` through the re-pointed `@s2script/cs2`.

- [ ] **Step 5: Changeset + commit.**
  ```bash
  npm run changeset   # patch/minor: @s2script/cs2 re-pointed at @s2script/sdk
  git add -A
  gt create packaging-consolidation/republish-cs2 -m "consolidation: re-point @s2script/cs2 at @s2script/sdk

@s2script/cs2 dropped its exact stub pins for a single @s2script/sdk dep and rewrote
its .d.ts imports to @s2script/sdk/* — else the published game package dangles on
deleted stub versions.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
  ```

---

## PR C5 (Phase 3): `packaging-consolidation/absorb-cli` — the CLI ships in `@s2script/sdk`, bin `s2s`

Completes Fork 2 (`@s2script/sdk` = types + CLI, the `typescript`/`tsc` model). Physically move the CLI into `packages/sdk/`, give `@s2script/sdk` its own bin — **named `s2s`** (bin names are exempt from npm's package-name filter; installed usage `s2s build`, cold-start `npx @s2script/sdk build` — NOT `npx s2s`, which resolves the unrelated `s2s@0.20.1` package) — and make `@s2script/cli` a true deprecated redirect. There is no Part-A forwarding shim to delete (Part A never happened). **This is the LAST PR** — it runs after every PR that edits `packages/cli/src/*` (C1, C-cs2lit, C3), so those earlier PRs keep their `packages/cli/...` paths and this one does the move + reference rewrite in a single atomic step.

**Files:**
- Move (git mv): `packages/cli/{src,test,build.mjs,tsconfig.json}` → `packages/sdk/`
- Modify: `packages/sdk/package.json` (add `bin: { "s2s": "dist/cli.js" }`, CLI deps + scripts)
- Modify (reference rewrites `packages/cli/` → `packages/sdk/`): `scripts/check-schema-generated.sh`, `scripts/check-nav-generated.sh`, `scripts/check-events-generated.sh`, `scripts/check-csitem-generated.sh`, `scripts/check-plugins-typecheck.sh`, `scripts/publish-packages.sh`, `scripts/build-base-plugins.sh`, `scripts/bootstrap-npm-trusted-publishing.sh`, the `tsconfig.base.json` comment
- Delete: `packages/cli/` (now a deprecated redirect — see Step 6)

**Interfaces:**
- Consumes: the CLI at `packages/cli/dist/cli.js` + `packages/cli/src/typecheck/typecheck.ts` (the paths every script uses today)
- Produces: the CLI at `packages/sdk/dist/cli.js` + `packages/sdk/src/typecheck/typecheck.ts`; `@s2script/sdk` bins `s2s` at its own dist; `@s2script/cli` no longer a dependency of anything

**Gate that proves this PR:** every `check-*-generated.sh` still green (they now invoke `packages/sdk/dist/cli.js`); `./scripts/check-plugins-typecheck.sh` green (it now imports `packages/sdk/src/typecheck/typecheck.ts`); `cd packages/sdk && npm test` (the moved CLI test suite); `./scripts/build-base-plugins.sh` builds; `node packages/sdk/dist/cli.js --help` runs the real CLI.

- [ ] **Step 1: git-move the CLI into the package.**
  ```bash
  cd /home/gkh/projects/s2script
  git mv packages/cli/src packages/sdk/src
  git mv packages/cli/test packages/sdk/test-cli   # avoids colliding with any sdk test/ dir; rename in-package next
  git mv packages/cli/build.mjs packages/sdk/build.mjs
  git mv packages/cli/tsconfig.json packages/sdk/tsconfig.json
  # If packages/sdk has no test/ yet, prefer the plain name:
  # git mv packages/sdk/test-cli packages/sdk/test
  ```
  Verify: `ls packages/sdk/src/typecheck/typecheck.ts` exists.

- [ ] **Step 2: Fold the CLI's `package.json` into `@s2script/sdk`'s.** From `@s2script/cli`'s `package.json`, copy into `packages/sdk/package.json`: the `dependencies` (`esbuild`, `adm-zip`, `typescript`), the `devDependencies` (`@types/adm-zip`), the `build`/`test` scripts, and set `"type": "module"` (the CLI is ESM). Add the bin — **name `s2s`, not `s2script`** (the pivot):
  ```json
  "type": "module",
  "bin": { "s2s": "dist/cli.js" },
  "scripts": {
    "build": "node build.mjs",
    "test": "node --experimental-strip-types --no-warnings --test test/*.test.mjs"
  }
  ```
  Update `files` to include `dist` alongside the `*.d.ts` types entries (keep the `exports` map from C1). There is NO `forward.cjs`/`bin/s2script.cjs` to delete — Part A never shipped.

- [ ] **Step 3: Rewrite every script/config reference `packages/cli` → `packages/sdk`.** Derive the set (do not trust the list), then rewrite:
  ```bash
  grep -rn 'packages/cli' scripts/ tsconfig.base.json | grep -v node_modules
  sed -i 's#packages/cli/#packages/sdk/#g' \
    scripts/check-schema-generated.sh scripts/check-nav-generated.sh \
    scripts/check-events-generated.sh scripts/check-csitem-generated.sh \
    scripts/publish-packages.sh scripts/build-base-plugins.sh \
    scripts/bootstrap-npm-trusted-publishing.sh
  # check-plugins-typecheck.sh imports ./packages/cli/src/typecheck/typecheck.ts:
  sed -i 's#packages/cli/src/typecheck#packages/sdk/src/typecheck#g' scripts/check-plugins-typecheck.sh
  # tsconfig.base.json has a prose comment naming packages/cli/src/typecheck/typecheck.ts:
  sed -i 's#packages/cli/src/typecheck#packages/sdk/src/typecheck#g' tsconfig.base.json
  grep -rn 'packages/cli' scripts/ tsconfig.base.json | grep -v node_modules || echo "clean"
  ```
  Expected: `clean` (also confirm `bootstrap-npm-trusted-publishing.sh`'s `@s2script/cli` package-name check is updated/removed — that script lists publishable packages; `@s2script/sdk` replaces `@s2script/cli` there).

- [ ] **Step 4: Build the moved CLI and run its tests, expect PASS.**
  ```bash
  ( cd packages/sdk && node build.mjs >/dev/null )
  test -f packages/sdk/dist/cli.js && echo "built"
  ( cd packages/sdk && npm test )
  ```
  Expected: `built`; the moved CLI test suite passes with the same pre-existing failure set as before the move (no NEW failures).

- [ ] **Step 5: Run the generated-checks + plugin gate against the new location.**
  ```bash
  ./scripts/check-schema-generated.sh && ./scripts/check-nav-generated.sh
  ./scripts/check-events-generated.sh && ./scripts/check-csitem-generated.sh
  ./scripts/check-plugins-typecheck.sh
  ./scripts/build-base-plugins.sh
  ```
  Expected: all green — they now drive `packages/sdk/dist/cli.js` and import the moved `typecheck.ts`.

- [ ] **Step 6: Turn `@s2script/cli` into a deprecated redirect.** Remove the workspace package (nothing in-repo depends on it now) and record the published-package deprecation as a manual step:
  ```bash
  git rm -r packages/cli
  ```
  Add to the "Manual steps" note: at publish time run `npm deprecate @s2script/cli "The CLI now ships in @s2script/sdk — npm i -D @s2script/sdk (bin: s2s)"`. The published `@s2script/cli@0.2.x` stays on npm (deprecated); it is simply no longer produced from this repo.

- [ ] **Step 7: Smoke the real CLI.** After publish (manual), `npx @s2script/sdk build` in an empty dir runs the package's own `dist/cli.js`, and an installed consumer gets the `s2s` command (`s2s build`). In-repo, prove the bin target resolves to the built CLI:
  ```bash
  node packages/sdk/dist/cli.js --help 2>&1 | head -c 120   # the CLI's own help/usage
  ```

- [ ] **Step 8: Changeset + commit.**
  ```bash
  npm run changeset   # minor `@s2script/sdk`: absorb the CLI (types + CLI in one package, bin s2s); @s2script/cli deprecated
  git add -A
  gt create packaging-consolidation/absorb-cli -m "consolidation: absorb the CLI into @s2script/sdk (types + CLI, bin s2s)

Move packages/cli/{src,test,build.mjs,tsconfig} into packages/sdk/, add the s2s bin
pointing at the package's own built dist/cli.js, and rewrite the ~8 script/config
refs from packages/cli to packages/sdk. @s2script/cli becomes a deprecated redirect
(git-removed here; npm deprecate at publish). Completes Fork 2: one install gives
types AND the CLI (s2s build; cold-start npx @s2script/sdk build).

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
  ```

- [ ] **Step 9: Submit the whole stack.**
  ```bash
  gt restack && gt submit --no-interactive
  ```

---

## Stack summary (dependency order)

1. `packaging-consolidation/dual-resolve` (Phase 1 — the trap PR; creates `packages/sdk/` types-only; canary is the proof)
2. `packaging-consolidation/migrate-<plugin>` ×N (Phase 2 — grep-derived batch count, one per plugin)
3. `packaging-consolidation/cs2-require-literals` (Phase 2 — live Docker CS2 gate, `pawn.origin != null`; incl. the schema-runtime stub update)
4. `packaging-consolidation/remove-legacy-prefix` (Phase 3 — delete stubs + BUILTIN_MODULES, honest filter)
5. `packaging-consolidation/republish-cs2` (Phase 3 — re-point the game package at `@s2script/sdk`)
6. `packaging-consolidation/absorb-cli` (Phase 3, LAST — CLI ships in `@s2script/sdk` with bin `s2s`, `@s2script/cli` deprecated)

(The former `rename-root-package` PR is dropped — the root `package.json` stays `name: "s2script"`, private; the unscoped npm name is unobtainable and there is nothing to free.)
