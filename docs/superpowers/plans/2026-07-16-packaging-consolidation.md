# Part C — @s2script/* → one `s2script` package Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (- [ ]) syntax for tracking.

**Goal:** Consolidate the 29 always-present `@s2script/*` types-only builtin stubs (plus `globals`) into a single `s2script` package with per-capability subpaths, so a builtin resolves as `s2script/<cap>` (miss = TS2307) while presence-conditional inter-plugin interfaces keep the `@scope/*` shape (miss = `any`) — making the `typecheck.ts:76` filter honest by shape, not by a disk guess.

**Architecture:** A dual-prefix transition. `core/src/v8host.rs s2require` learns to strip `s2script/` alongside the existing `@s2script/` (both → `globalThis.__s2pkg_<cap>`), the four CLI type-resolution sites learn the new `packages/s2script/<cap>.d.ts` layout, consumers migrate in batches while both prefixes resolve, then the legacy `@s2script/<builtin>` prefix and `BUILTIN_MODULES` are removed once nothing uses them. The game package `@s2script/cs2` stays scoped throughout (game → core, never core → game) and moves from `pluginDependencies` to npm `dependencies` so the final typecheck filter is purely shape-based.

**Tech Stack:** Rust (`s2script-core`, cdylib + V8), TypeScript CLI (esbuild/tsc, node `--test` `.mjs`), the games/cs2 prelude JS + its schemagen/navgen emitters, npm workspaces + changesets.

## Global Constraints

- **Ship work as a stack (Graphite), not a branch.** Small atomic PRs, one per reviewable change; always argue for more PRs, never fewer. Branch naming `packaging-consolidation/<terse-change>`.
- **Run the gate suite PER PR, not once at the top:** `make check-boundary`, `./scripts/check-plugins-typecheck.sh`, `cargo test -p s2script-core` (single-threaded — never pass `--test-threads`), `./scripts/check-schema-generated.sh`, `./scripts/check-nav-generated.sh`, `./scripts/check-events-generated.sh`, `./scripts/check-csitem-generated.sh`, `./scripts/test-boundary-nameleak.sh`. State which gate proves each PR.
- **Core is engine-generic; it NEVER imports `games/*`.** `s2require`'s dual-strip stays generic — no module list hardcoded; `s2script/cs2` and `@s2script/cs2` ride the same rule as any capability.
- **Green CI does not prove the Phase-1 PR correct** — green is exactly the silent-hollowing signature. The passing canary (a deliberate builtin type error still FAILS the gate) is what proves resolution did not degrade to `any`.
- **The `games/cs2` `__s2require` literals are compiler-invisible.** A missed rename degrades to `pawn.origin → null` silently at runtime; the live Docker CS2 gate (`pawn.origin != null`), not CI, is that PR's proof.
- **Every count in this plan is illustrative — grep, never hardcode a count.** The literal off-by-one (an earlier pass said 9 vs 10) is the exact silent-failure class this migration worries about.
- **Versioning stays two axes:** a plugin declares `dependencies: { "s2script": "^0.1.0" }` (types) AND keeps `s2script.apiVersion` (host ABI). Not collapsed. `s2script` starts pre-1.0 (not API-frozen); consumers pin `^0.1.0` and re-pin as minors move.
- **Depends on Part A** having claimed the `s2script` npm name and created `packages/s2script/` with a real forwarding bin (`require.resolve("@s2script/cli/dist/cli.js")`). Phase 1 fills that package with the moved `.d.ts` + exports map; it does not touch the bin.

---

## Established facts (verified against the code — do not re-derive)

- `s2require` (`core/src/v8host.rs:4050`, strip at `:4065`): `name.strip_prefix("@s2script/")` → `format!("__s2pkg_{}", rest)` → `globalThis.__s2pkg_<rest>`. Generic, no module list. `@s2script/cs2 → __s2pkg_cs2` rides it.
- `BUILTIN_MODULES` (`core/src/loader.rs:78`) + `is_builtin_module` (`:88`); two call sites `imports_from_manifest` (`:121`, `:125`), both `continue` (skip builtins from the ledger). Manifest carries only `pluginDependencies`/`optionalPluginDependencies`; npm `dependencies` never reach it.
- `build.ts:78-82` esbuild `external` = `["@s2script/*", ...pluginDependencies keys, ...optionalPluginDependencies keys]`.
- `typecheck.ts` four resolution sites: `isBuiltinOnDisk` (`:60-62`, `@s2script/`-prefixed + `existsSync(packagesDir/<name>/index.d.ts)`), the filter (`:76`), `paths: { "@s2script/*": ["*/index.d.ts"] }` (`:87`), globals rootName `join(packagesDir, "globals", "globals.d.ts")` (`:91`). `packagesDir` is passed explicitly by `check-plugins-typecheck.sh` (`{ packagesDir: 'packages' }`) — `resolvePackagesDir`/`isPackagesDir` only run when it is omitted.
- `packages-resolve.ts`: `isPackagesDir` sniffs `globals/globals.d.ts` / `entity/index.d.ts` / …; error text names `@s2script/globals`.
- `tsconfig.base.json:12` — editor twin `"@s2script/*": ["*/index.d.ts"]` (CLI does NOT read it; editor-only).
- The 29 builtin stub dirs = `ls packages/` minus `cli`, `cs2`, `globals`. Cross-`.d.ts` imports to rewrite (non-cs2): `grep -rnE '^\s*(import|export).*from "@s2script/' packages/*/index.d.ts` excluding `packages/cs2` — currently trace ×2, chat ×1, usercmd ×2, damage ×1, sound ×1, entity ×1, cookies ×1 (grep-derived; the cs2 ones move in Phase 3).
- The 10 runtime `__s2require` literals: hand-written in `games/cs2/js/pawn.js` (lines 7, 8, 283, 398, 830 — one embedded in the `__s2pkg_cs2 =` assignment) and `weapon.js:59`; **generator-emitted** in `packages/cli/src/schemagen/emit-js.ts:14` (→ `schema.generated.js:5-6`) and `packages/cli/src/navgen/emit-js.ts:35` (→ `nav.generated.js:4-5`). The generated ones MUST be renamed in the emitter + regenerated, or `check-schema-generated.sh`/`check-nav-generated.sh` fail.
- `@s2script/cs2` (`packages/cs2/package.json`) pins exact stub versions (`@s2script/entity: 0.3.0`, `math`, `trace`, `events`) and its `index.d.ts` imports them; it is declared in `pluginDependencies` by `plugins/zones` + `examples/{demo-plugin,entref-producer,zones-consumer-demo}`.
- Root `package.json` is already `name: "s2script"` (`private: true`) — must be renamed to free the name.

---

## PR C1 (Phase 1): `packaging-consolidation/dual-resolve` — publish the package + dual-resolve

The critical-trap PR. The `.d.ts` files physically move, so the gate's resolution sites **must move with them in this same commit** — otherwise a plugin that declares builtins falls through `isBuiltinOnDisk` into the ambient `any` stub and typechecks **green**: a silent hollowing of the 5E.1 gate that CI cannot catch. No consumer changes here — fully backward-compatible (both `@s2script/entity` and `s2script/entity` resolve after this).

**Gate that proves this PR:** the no-degrade **canary** fixture (a deliberate builtin type error still FAILS) plus the parity/TS2307 fixtures in `packages/cli/test/typecheck.test.mjs`; the `s2require` dual-strip Rust unit test; then full suite green (`cargo test -p s2script-core`, `./scripts/check-plugins-typecheck.sh`, `make check-boundary`, the four `check-*-generated.sh`). **Green alone is insufficient — the passing canary is the proof.**

### Task C1.1 — Move the 29 builtin `.d.ts` + `globals.d.ts` into `packages/s2script/`

**Files:**
- Create: `packages/s2script/<cap>.d.ts` ×29 (git-moved from `packages/<cap>/index.d.ts`), `packages/s2script/globals.d.ts` (git-moved from `packages/globals/globals.d.ts`)
- Modify: `packages/s2script/package.json` (exists from Part A — add `exports` + keep the bin)

**Interfaces:**
- Consumes: the 29 stub `index.d.ts` bodies, `globals/globals.d.ts`
- Produces: `packages/s2script/entity.d.ts` … one flat `.d.ts` per capability; an `exports` subpath map

- [ ] **Step 1: Derive the exact capability set (do not hardcode).**
  ```bash
  cd /home/gkh/projects/s2script
  comm -23 <(ls packages/ | sort) <(printf 'cli\ncs2\nglobals\n' | sort)
  ```
  Expected: the 29 capability names (admin, bans, chat, clients, commands, config, console, cookies, damage, db, entity, events, frame, http, interfaces, math, menu, net, plugins, server, sound, timers, topmenu, trace, translations, usercmd, usermessages, votes, ws). Save to a shell var `CAPS`.

- [ ] **Step 2: git-move each stub `.d.ts` to the flat layout.**
  ```bash
  for c in $(comm -23 <(ls packages/ | sort) <(printf 'cli\ncs2\nglobals\n' | sort)); do
    git mv "packages/$c/index.d.ts" "packages/s2script/$c.d.ts"
  done
  git mv packages/globals/globals.d.ts packages/s2script/globals.d.ts
  ```
  The old `packages/<cap>/` dirs keep their `package.json`/`CHANGELOG.md` (hollow now — deleted in Phase 3). `packages/globals/` likewise. Do NOT delete them here.

- [ ] **Step 3: Verify the move.**
  ```bash
  ls packages/s2script/*.d.ts | wc -l          # expect 30 (29 caps + globals)
  test ! -e packages/entity/index.d.ts && echo "moved"   # expect: moved
  ```

- [ ] **Step 4: Add the `exports` subpath map to `packages/s2script/package.json`.** Keep the Part-A `name`, `bin`, and `@s2script/cli` dependency untouched; add one `exports` entry per capability (types-condition only — these are types-only). Full map:
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
  (No flat `.` barrel — subpaths only, §3. `globals.d.ts` is injected by the gate as a rootName, not imported, so it is NOT in the map.) Also set `"files": ["*.d.ts", …the bin…]`.

- [ ] **Step 5: Confirm the exports map is well-formed JSON.**
  ```bash
  node -e 'JSON.parse(require("fs").readFileSync("packages/s2script/package.json","utf8")); console.log("ok")'
  ```
  Expected: `ok`.

### Task C1.2 — Rewrite the cross-`.d.ts` imports to relative paths

**Files:** Modify: `packages/s2script/{trace,chat,usercmd,damage,sound,entity,cookies}.d.ts` (the exact set is grep-derived below, not hardcoded)

**Interfaces:**
- Consumes: internal `import type { … } from "@s2script/<cap>"` lines
- Produces: `import type { … } from "./<cap>"` (relative; the `exports` map gates only external subpath access, internal relatives resolve regardless)

- [ ] **Step 1: List the internal imports to rewrite (grep-derived).**
  ```bash
  grep -rnE '^\s*(import|export).*from "@s2script/' packages/s2script/*.d.ts
  ```
  Expected: only capability→capability imports (e.g. `trace.d.ts: from "@s2script/math"`, `from "@s2script/entity"`; `cookies.d.ts: from "@s2script/clients"`; etc.). NO cs2 (cs2 is not moved).

- [ ] **Step 2: Rewrite `@s2script/<cap>` → `./<cap>` inside `packages/s2script/*.d.ts`.**
  ```bash
  sed -i -E 's#(from ")@s2script/([a-z]+)(")#\1./\2\3#g' packages/s2script/*.d.ts
  ```

- [ ] **Step 3: Verify no `@s2script/` specifier survives inside the package.**
  ```bash
  grep -rnE 'from "@s2script/' packages/s2script/*.d.ts || echo "clean"
  ```
  Expected: `clean`.

### Task C1.3 — `s2require` gains `s2script/` stripping (Rust, TDD)

**Files:**
- Modify: `core/src/v8host.rs:4065` (the single strip line in `fn s2require`)
- Test: `core/src/v8host.rs` (add a `#[test]` in the existing `mod tests`)

**Interfaces:**
- Consumes: `name: String` from `args.get(0)`
- Produces: `Option<&str> rest` — matches `@s2script/<rest>` OR `s2script/<rest>` → `__s2pkg_<rest>`; bare `s2script` (no `/`) → null

- [ ] **Step 1: Write the failing test** in `core/src/v8host.rs` `mod tests` (uses the existing `eval_in_context_bool` helper; the prelude sets `__s2pkg_math`/`__s2pkg_entity` in any plugin context):
  ```rust
  #[test]
  fn s2require_dual_resolves_scoped_and_unscoped_prefixes() {
      let _ = init(dummy_logger());
      create_plugin_context("dualpfx");
      // Both prefixes resolve the SAME capability global.
      assert!(eval_in_context_bool("dualpfx",
          r#"__s2require("s2script/math") === __s2require("@s2script/math")"#),
          "s2script/math must resolve to the same object as @s2script/math");
      assert!(eval_in_context_bool("dualpfx",
          r#"typeof __s2require("s2script/entity").EntityRef === "function""#),
          "s2script/entity must expose EntityRef");
      // A bare `s2script` (no trailing slash — the rejected flat barrel) → null at runtime.
      assert!(eval_in_context_bool("dualpfx",
          r#"__s2require("s2script") === null"#),
          "bare s2script must resolve to null");
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
  Expected: FAIL — `s2script/math` currently strips to `None` (only `@s2script/` handled) so `__s2require("s2script/math")` returns `null`, `null === __s2require(...)` is false → panic on the first assert.

- [ ] **Step 3: Implement the dual strip** at `core/src/v8host.rs:4065`. Replace:
  ```rust
        let Some(rest) = name.strip_prefix("@s2script/") else { return };
  ```
  with:
  ```rust
        // Dual-prefix (packaging consolidation): a builtin resolves as BOTH the legacy scoped
        // `@s2script/<cap>` and the consolidated `s2script/<cap>` — both map to `__s2pkg_<cap>`.
        // Bare `s2script` (no trailing slash) matches neither → null (the flat barrel is rejected;
        // the typecheck gate, not s2require, enforces the namespace split). Still generic — no
        // module list hardcoded; `@s2script/cs2` / `s2script/cs2` ride the same rule.
        let Some(rest) = name
            .strip_prefix("@s2script/")
            .or_else(|| name.strip_prefix("s2script/"))
        else {
            return;
        };
  ```
  Also update the doc-comment above `fn s2require` (`:4042-4047`) to name both prefixes.

- [ ] **Step 4: Run it, expect PASS.**
  ```bash
  cargo test -p s2script-core s2require_dual_resolves
  ```
  Expected: `test ... ok`.

### Task C1.4 — Move all four CLI type-resolution sites (same commit, TDD via the canary)

**Files:**
- Modify: `packages/cli/src/typecheck/typecheck.ts:60-62` (`isBuiltinOnDisk`), `:87` (`paths`), `:91` (globals rootName)
- Modify: `packages/cli/src/packages-resolve.ts` (`isPackagesDir`, error text)
- Modify: `packages/cli/src/build.ts:78-82` (esbuild `external`)
- Modify: `tsconfig.base.json:12` (editor twin)
- Test: `packages/cli/test/typecheck.test.mjs` (+ new fixtures under `packages/cli/test/fixtures/typecheck/`)

**Interfaces:**
- Consumes: `packagesDir` (= `packages` in the gate), a plugin's imports + declared deps
- Produces: builtins resolve at `packages/s2script/<cap>.d.ts` (new) OR `packages/<cap>/index.d.ts` (fallback, now serving only cs2); `s2script/<cap>` resolves; `s2script/<cap>` + `s2script/*` are esbuild-external

- [ ] **Step 1: Write the failing canary + acceptance fixtures.** Create four fixtures. First, a shared fake `s2script` package the fixtures resolve against (mirrors `fake-packages/` but adds the new layout):
  - `packages/cli/test/fixtures/typecheck/fake-packages/s2script/entity.d.ts`:
    ```ts
    export interface EntityRef { readonly index: number; readonly serial: number; }
    export declare const Entity: { forRef(r: EntityRef): { health: number | null } | null };
    ```
  - `packages/cli/test/fixtures/typecheck/fake-packages/entity/index.d.ts` (legacy twin, so `@s2script/entity` still resolves during transition):
    ```ts
    export interface EntityRef { readonly index: number; readonly serial: number; }
    export declare const Entity: { forRef(r: EntityRef): { health: number | null } | null };
    ```
  - `packages/cli/test/fixtures/typecheck/fake-packages/s2script/globals.d.ts` (REQUIRED: the gate always injects a globals rootName via `existsSync(packagesDir/s2script/globals.d.ts)` → this path; without the file the rootName points at nothing and the program build is unreliable). Minimal ambient content is fine:
    ```ts
    declare global { const HookResult: { Continue: 0; Changed: 1; Handled: 2; Stop: 3 }; }
    export {};
    ```
  - Canary fixture `.../typecheck/canary-scoped/` (deliberate error against `@s2script/entity`):
    - `package.json`: `{ "name":"@fix/canary-scoped","version":"1.0.0","main":"src/plugin.ts","s2script":{"apiVersion":"1.x"},"private":true }`
    - `src/plugin.ts`:
      ```ts
      import { Entity, EntityRef } from "@s2script/entity";
      export function onLoad(r: EntityRef): void {
        const hp: number = Entity.forRef(r)!.health;   // TS2322: number | null → number
        console.log(hp);
      }
      ```
  - Canary fixture `.../typecheck/canary-unscoped/` — identical but `import … from "s2script/entity"`, name `@fix/canary-unscoped`.
  - Acceptance fixture `.../typecheck/typo-builtin/` (a builtin TYPO must be TS2307, not `any`):
    - `package.json`: `{ "name":"@fix/typo-builtin","version":"1.0.0","main":"src/plugin.ts","s2script":{"apiVersion":"1.x"},"private":true }`
    - `src/plugin.ts`: `import { Entity } from "s2script/frmae"; export const x = Entity;`
  - Acceptance fixture `.../typecheck/typo-interface/` (an unfetched interface typo must stay `any`):
    - `package.json`: `{ "name":"@fix/typo-interface","version":"1.0.0","main":"src/plugin.ts","s2script":{"apiVersion":"1.x","pluginDependencies":{"@community/mapchoser":"^1.0.0"}},"private":true }`
    - `src/plugin.ts`: `import x from "@community/mapchoser"; export const y = String(x);`

  Then add the tests to `packages/cli/test/typecheck.test.mjs`:
  ```js
  test("canary: a deliberate builtin type error still FAILS (scoped @s2script/entity)", () => {
    const r = typecheckPlugin(join(fixtures, "canary-scoped"), { packagesDir: fakePkgs });
    assert.equal(r.ok, false, "scoped canary must fail — green means resolution degraded to any");
    assert.ok(r.diagnostics.some((d) => d.code === 2322),
      "expected TS2322: " + JSON.stringify(r.diagnostics));
  });

  test("canary: a deliberate builtin type error still FAILS (unscoped s2script/entity)", () => {
    const r = typecheckPlugin(join(fixtures, "canary-unscoped"), { packagesDir: fakePkgs });
    assert.equal(r.ok, false, "unscoped canary must fail — green means resolution degraded to any");
    assert.ok(r.diagnostics.some((d) => d.code === 2322),
      "expected TS2322: " + JSON.stringify(r.diagnostics));
  });

  test("acceptance: a builtin TYPO yields TS2307, not any", () => {
    const r = typecheckPlugin(join(fixtures, "typo-builtin"), { packagesDir: fakePkgs });
    assert.equal(r.ok, false);
    assert.ok(r.diagnostics.some((d) => d.code === 2307),
      "expected TS2307 for s2script/frmae: " + JSON.stringify(r.diagnostics));
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
  Expected: `canary-unscoped` FAILS (its `s2script/entity` import currently maps to nothing → TS2307, so `some(2322)` is false); `typo-builtin` FAILS (no `s2script/*` path yet → resolves via nothing → wrong code); the scoped canary passes today (it already resolves via `entity/index.d.ts`). Failing tests confirm the fixtures exercise the not-yet-built resolution.

- [ ] **Step 3: Implement — `isBuiltinOnDisk` checks both locations** (`typecheck.ts:60-62`). Replace:
  ```ts
    const isBuiltinOnDisk = (d: string): boolean =>
      d.startsWith("@s2script/") &&
      existsSync(join(packagesDir, d.slice("@s2script/".length), "index.d.ts"));
  ```
  with:
  ```ts
    // A builtin resolves either at the consolidated layout (packages/s2script/<cap>.d.ts) or the
    // legacy per-package layout (packages/<cap>/index.d.ts, still serving @s2script/cs2). During the
    // dual-prefix transition BOTH the scoped `@s2script/<cap>` and unscoped `s2script/<cap>` spellings
    // count as builtin-on-disk so a plugin that still DECLARES one in pluginDependencies is filtered
    // out of the ambient-stub list and resolves via `paths` below (never degrades to `any`).
    const capOf = (d: string): string | null =>
      d.startsWith("@s2script/") ? d.slice("@s2script/".length)
      : d.startsWith("s2script/") ? d.slice("s2script/".length)
      : null;
    const isBuiltinOnDisk = (d: string): boolean => {
      const cap = capOf(d);
      if (cap === null) return false;
      return existsSync(join(packagesDir, "s2script", cap + ".d.ts")) ||
             existsSync(join(packagesDir, cap, "index.d.ts"));
    };
  ```

- [ ] **Step 4: Implement — `paths` ordered fallback + new `s2script/*` entry** (`typecheck.ts:87`). Replace:
  ```ts
      paths: { "@s2script/*": ["*/index.d.ts"] },
  ```
  with:
  ```ts
      paths: {
        // Consolidated layout first, legacy per-package second (the latter now serves only
        // @s2script/cs2, which is NOT moved). Collapsed to just s2script/* + @s2script/* (cs2)
        // in Phase 3 once the legacy builtin dirs are deleted.
        "s2script/*": ["s2script/*.d.ts"],
        "@s2script/*": ["s2script/*.d.ts", "*/index.d.ts"],
      },
  ```

- [ ] **Step 5: Implement — globals rootName both locations** (`typecheck.ts:91`). Replace:
  ```ts
    const rootNames = [entry, join(packagesDir, "globals", "globals.d.ts"), ...localDts];
  ```
  with:
  ```ts
    const globalsDts = existsSync(join(packagesDir, "s2script", "globals.d.ts"))
      ? join(packagesDir, "s2script", "globals.d.ts")
      : join(packagesDir, "globals", "globals.d.ts");
    const rootNames = [entry, globalsDts, ...localDts];
  ```

- [ ] **Step 6: Implement — `packages-resolve.ts` learns the consolidated shape.** In `isPackagesDir`, add the new layout as a recognized shape; in the throw text, drop the `@s2script/globals` wording:
  ```ts
  export function isPackagesDir(dir: string): boolean {
    const abs = resolve(dir);
    return (
      existsSync(join(abs, "s2script", "globals.d.ts")) ||   // consolidated layout
      existsSync(join(abs, "s2script", "entity.d.ts")) ||    // consolidated layout
      existsSync(join(abs, "globals", "globals.d.ts")) ||    // legacy per-package layout
      existsSync(join(abs, "entity", "index.d.ts")) ||
      existsSync(join(abs, "frame", "index.d.ts")) ||
      existsSync(join(abs, "commands", "index.d.ts"))
    );
  }
  ```
  And the final `throw` message body:
  ```ts
    throw new Error(
      "cannot resolve s2script/* types: no packages dir found.\n" +
        "  Install `s2script` in the plugin (npm i -D s2script),\n" +
        "  or set S2SCRIPT_PACKAGES_DIR / pass --packages-dir."
    );
  ```

- [ ] **Step 7: Implement — esbuild `external` accepts `s2script/*`** (`build.ts:78-82`):
  ```ts
    const external = Array.from(new Set([
      "@s2script/*",
      "s2script/*",
      ...Object.keys(pluginDependencies),
      ...Object.keys(optionalPluginDependencies),
    ]));
  ```
  (`s2script/*` does not match a bare `s2script` import — fine, the flat barrel is rejected, §3.)

- [ ] **Step 8: Implement — editor twin** (`tsconfig.base.json:12`):
  ```json
      "paths": { "s2script/*": ["s2script/*.d.ts"], "@s2script/*": ["s2script/*.d.ts", "*/index.d.ts"] },
  ```

- [ ] **Step 9: Rebuild the CLI (the generated-checks and gate import the built dist) and run the tests, expect PASS.**
  ```bash
  ( cd packages/cli && node build.mjs >/dev/null )
  cd packages/cli && node --experimental-strip-types --no-warnings --test test/typecheck.test.mjs
  ```
  Expected: all four new tests plus the two existing ones pass. **The passing `canary-unscoped` + `canary-scoped` is the load-bearing proof this PR did not hollow the gate.**

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
  npm run changeset   # minor bump `s2script` 0.0.x → 0.1.0; note "first real release: consolidated builtin .d.ts + dual-resolve"
  ```
  **Version note (load-bearing):** Part A planted `s2script@0.0.x` as a placeholder; this is the first REAL release of the consolidated types package. It starts **pre-1.0 at 0.1.0** — the API is not frozen yet, so minors are allowed to break and consumers re-pin. Phase-2 consumers pin `"s2script": "^0.1.0"` (npm 0.x caret = `>=0.1.0 <0.2.0`), which only resolves against a published `>=0.1.0`. In-repo the typecheck gate resolves `s2script/*` via `paths` (not `node_modules`), so the version pin is cosmetic for the gate; it becomes load-bearing at publish and for any out-of-monorepo consumer. Keep `apiVersion` a separate axis (§Global Constraints).

- [ ] **Step 2: Commit the whole PR as one atomic change** (branch `packaging-consolidation/dual-resolve`, tracked on main in a worktree per the slice cadence):
  ```bash
  gt track -p main   # only if the worktree branch starts untracked
  git add -A
  gt create packaging-consolidation/dual-resolve -m "consolidation: publish s2script package + dual-resolve builtins

Move the 29 builtin .d.ts + globals into packages/s2script/, teach s2require
to strip s2script/ alongside @s2script/, and move all four CLI type-resolution
sites in lockstep so builtins resolve at the new location while @s2script/cs2
resolves at the old one. Ships the no-degrade canary: a deliberate builtin type
error still FAILS the gate (green CI is the silent-failure signature).

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
  ```

---

## PR C2..Cn (Phase 2): `packaging-consolidation/migrate-<batch>` — migrate consumers in batches

Rewrite `@s2script/<builtin>` → `s2script/<builtin>` in consumer sources and move builtins **and `@s2script/cs2`** from `s2script.pluginDependencies` to npm `dependencies` (Fork 1). Each PR is atomic because both prefixes still resolve (PR C1). **The `@s2script/cs2` import specifier is unchanged — it stays scoped; only its declaration map entry moves.** One PR per plugin (or a small batch); **always argue for more PRs, never fewer**.

**Gate that proves each batch PR:** `./scripts/check-plugins-typecheck.sh` green (every migrated plugin resolves under `s2script/*`), plus `make check-boundary` and `cargo test -p s2script-core` (unchanged, must stay green).

### Task C2.0 — Generate the batch list (grep-derived, not hardcoded)

- [ ] **Step 1: Enumerate the consumer files and dirs to migrate.**
  ```bash
  cd /home/gkh/projects/s2script
  # Files importing a builtin @s2script/<cap> (cs2 excluded — its specifier stays scoped):
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
- Produces: `s2script/<builtin>` imports; builtins collapse to one `dependencies: { "s2script": "^0.1.0" }`; `@s2script/cs2` → `dependencies` (scoped key kept); interface deps stay in `pluginDependencies`

- [ ] **Step 1: Rewrite builtin import specifiers in this plugin's sources** (leave `@s2script/cs2` alone):
  ```bash
  P=plugins/zones
  grep -rlE 'from "@s2script/(admin|bans|chat|clients|commands|config|console|cookies|damage|db|entity|events|frame|http|interfaces|math|menu|net|plugins|server|sound|timers|topmenu|trace|translations|usercmd|usermessages|votes|ws)"' "$P/src" \
    | xargs sed -i -E 's#(from ")@s2script/(admin|bans|chat|clients|commands|config|console|cookies|damage|db|entity|events|frame|http|interfaces|math|menu|net|plugins|server|sound|timers|topmenu|trace|translations|usercmd|usermessages|votes|ws)(")#\1s2script/\2\3#g'
  ```

- [ ] **Step 2: Verify only `@s2script/cs2` (if any) survives as a scoped import in this plugin.**
  ```bash
  grep -rnE 'from "@s2script/' "$P/src" || echo "no scoped imports"
  ```
  Expected: either nothing, or only `@s2script/cs2` lines.

- [ ] **Step 3 (Shape B only): move builtins + cs2 in `package.json`.** Edit `plugins/zones/package.json`: delete the builtin `@s2script/<cap>` and `@s2script/cs2` keys from `s2script.pluginDependencies`; add a top-level `dependencies` block. For `zones`, `pluginDependencies` currently holds 10 builtins + `@s2script/cs2` and NO non-builtin interface, so the whole map moves and `s2script.pluginDependencies` is removed:
  ```json
  {
    "name": "@s2script/zones",
    "version": "0.3.0",
    "private": true,
    "main": "src/plugin.ts",
    "types": "api.d.ts",
    "dependencies": {
      "s2script": "^0.1.0",
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
    "dependencies": { "s2script": "^0.1.0", "@s2script/cs2": "^0.5.0" }
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
  gt create packaging-consolidation/migrate-zones -m "consolidation: migrate zones to s2script/* + npm deps

Rewrite @s2script/<builtin> → s2script/<builtin>; move builtins and @s2script/cs2
from s2script.pluginDependencies to npm dependencies. Both prefixes still resolve,
so this is atomic.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
  ```

Repeat Task C2.template per plugin dir from the C2.0 batch list (grep-derived count).

---

## PR C-cs2lit (Phase 2): `packaging-consolidation/cs2-require-literals` — the games/cs2 literals

The 10 compiler-invisible `__s2require` literals. A miss degrades to `pawn.origin → null` **silently** — CI cannot catch it, so the live Docker CS2 gate is the proof. The generator-emitted literals must be renamed **in the emitter + regenerated**, or the codegen-freshness gates fail.

**Files:**
- Modify: `games/cs2/js/pawn.js` (5 literals: lines 7, 8, 283, 398, 830), `games/cs2/js/weapon.js` (line 59)
- Modify: `packages/cli/src/schemagen/emit-js.ts:14`, `packages/cli/src/navgen/emit-js.ts:35`
- Regenerate: `games/cs2/js/schema.generated.js`, `games/cs2/js/nav.generated.js`

**Interfaces:**
- Consumes: `__s2require("@s2script/<cap>")` runtime calls
- Produces: `__s2require("s2script/<cap>")` — resolves via PR C1's dual-strip (both spellings work; this proves the unscoped one live)

**Gate that proves this PR:** `./scripts/check-schema-generated.sh` + `./scripts/check-nav-generated.sh` green (regeneration matched the committed output), THEN the **live Docker CS2 gate**: load a plugin and assert `pawn.origin != null`. CI alone is insufficient.

- [ ] **Step 1: Confirm the exact literal set (grep, do not trust the count).**
  ```bash
  grep -rn '__s2require("@s2script/' games/cs2/js/ packages/cli/src/schemagen/emit-js.ts packages/cli/src/navgen/emit-js.ts
  ```
  Expected: pawn.js ×5, weapon.js ×1 (hand-written), schemagen/emit-js.ts ×1, navgen/emit-js.ts ×1 (emitters). 10 runtime literals total (2 emitters produce 4 generated literals).

- [ ] **Step 2: Rewrite the hand-written literals in pawn.js + weapon.js.**
  ```bash
  sed -i -E 's#(__s2require\(")@s2script/([a-z]+)("\))#\1s2script/\2\3#g' games/cs2/js/pawn.js games/cs2/js/weapon.js
  grep -rn '__s2require("@s2script/' games/cs2/js/pawn.js games/cs2/js/weapon.js || echo "clean"
  ```
  Expected: `clean`.

- [ ] **Step 3: Rewrite the emitters.** `packages/cli/src/schemagen/emit-js.ts:14` and `packages/cli/src/navgen/emit-js.ts:35` both emit ``` `  var ${cls} = __s2require("@s2script/math").${cls};` ```. Change `@s2script/math` → `s2script/math` in both.

- [ ] **Step 4: Rebuild the CLI and regenerate the generated JS.**
  ```bash
  ( cd packages/cli && node build.mjs >/dev/null )
  node packages/cli/dist/cli.js gen-schema     # rewrites games/cs2/js/schema.generated.js
  node packages/cli/dist/cli.js gen-nav        # rewrites games/cs2/js/nav.generated.js
  grep -rn '__s2require("@s2script/' games/cs2/js/*.generated.js || echo "clean"
  ```
  Expected: `clean`; the two generated files now carry `s2script/math`.
  (Determine the exact regen subcommand from `check-schema-generated.sh`/`check-nav-generated.sh` — they invoke `gen-schema --check` / `gen-nav --check`; drop `--check` to write.)

- [ ] **Step 5: Run the codegen-freshness gates, expect PASS.**
  ```bash
  ./scripts/check-schema-generated.sh && ./scripts/check-nav-generated.sh
  ```
  Expected: `PASS: schema codegen is up to date` / `PASS: nav codegen is up to date` (regeneration matched the committed output — no stray diff).

- [ ] **Step 6: Package + deploy to the Docker CS2 dev server and run the live gate.** This is the mechanism-proof for the invisible literals.
  ```bash
  make docker-test    # if not already up
  # (re)package the addon so games/cs2/js/*.js reach dist; the prelude is a CONCAT — do NOT raw-cp a single file
  ./scripts/package-addon.sh
  docker compose -f docker/docker-compose.yml restart cs2   # NOT --force-recreate (resets gameinfo.gi)
  # arm after the boot window, then:
  python3 scripts/rcon.py "sm_pawntest_or_a_plugin_that_reads_pawn_origin"
  ```
  Expected: a plugin reading `pawn.origin` returns a real Vector (not `null`). A `null` means a missed literal — the silent-failure this PR guards. Follow the live-gate cadence (arm after boot; deterministic `round_start` if a player is needed).

- [ ] **Step 7: Commit.**
  ```bash
  git add -A
  gt create packaging-consolidation/cs2-require-literals -m "consolidation: rename games/cs2 __s2require literals to s2script/*

10 compiler-invisible runtime literals (pawn.js ×5, weapon.js ×1, + schemagen/navgen
emitters, regenerated). Dual-resolve keeps both spellings working; the live Docker
CS2 gate (pawn.origin != null) is the proof — CI cannot see these.

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
- Produces: `imports_from_manifest` no longer special-cases builtins; a `s2script/<cap>` name never stubs (resolve-or-TS2307); `pluginDependencies` entries not locally resolvable stub to `any`

**Gate that proves this PR:** a grep proves zero `@s2script/<builtin>` imports survive; `cargo test -p s2script-core` (incl. the new legacy-load test); `./scripts/check-plugins-typecheck.sh`; the narrowed-filter typecheck test.

### Task C3.1 — Grep-gate: zero legacy builtin imports survive

- [ ] **Step 1: Prove nothing imports a builtin under the legacy prefix.**
  ```bash
  cd /home/gkh/projects/s2script
  grep -rnE 'from "@s2script/(admin|bans|chat|clients|commands|config|console|cookies|damage|db|entity|events|frame|http|interfaces|math|menu|net|plugins|server|sound|timers|topmenu|trace|translations|usercmd|usermessages|votes|ws)"' \
    plugins/ examples/ disabled/ games/ && echo "FAIL: legacy imports remain" || echo "PASS: none"
  grep -rn '__s2require("@s2script/' games/cs2/js/ && echo "FAIL" || echo "PASS: none"
  ```
  Expected: `PASS: none` for both. (Do NOT proceed until clean — a survivor here becomes a dangling import after deletion.)

### Task C3.2 — Delete the hollow stub packages + globals

- [ ] **Step 1: Delete the 29 legacy dirs + globals (grep-derived set).**
  ```bash
  for c in $(comm -23 <(ls packages/ | sort) <(printf 'cli\ncs2\nglobals\ns2script\n' | sort)); do
    git rm -r "packages/$c"
  done
  git rm -r packages/globals
  ls packages/   # expect: cli, cs2, globals GONE; s2script present; cs2 present
  ```

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

- [ ] **Step 2: Write the load-side legacy-posture test** in `core/src/loader.rs` `mod tests` (a pre-migration manifest with builtins still in `pluginDependencies` loads and runs post-deletion — phantom-lazy-hard-dep, §6.4). Uses the existing `make_test_s2sp` helper:
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
- Produces: `s2script/*` and `@s2script/cs2` never stub (resolve or TS2307); only `pluginDependencies` entries that are not locally resolvable stub to `any`; the disk-existence guess is gone

- [ ] **Step 1: Write the narrowed-filter failing test** in `packages/cli/test/typecheck.test.mjs`. A plugin that (incorrectly, legacy-style) DECLARES `s2script/frmae` in `pluginDependencies` must still get TS2307 — proving `s2script/*` never stubs:
  - Fixture `.../typecheck/decl-builtin-typo/`:
    - `package.json`: `{ "name":"@fix/decl-builtin-typo","version":"1.0.0","main":"src/plugin.ts","s2script":{"apiVersion":"1.x","pluginDependencies":{"s2script/frmae":"^1.0.0"}},"private":true }`
    - `src/plugin.ts`: `import { Entity } from "s2script/frmae"; export const x = Entity;`
  - Test:
    ```js
    test("narrowed filter: a declared s2script/* typo still yields TS2307 (never stubs)", () => {
      const r = typecheckPlugin(join(fixtures, "decl-builtin-typo"), { packagesDir: fakePkgs });
      assert.equal(r.ok, false);
      assert.ok(r.diagnostics.some((d) => d.code === 2307),
        "s2script/* must resolve-or-error, never stub: " + JSON.stringify(r.diagnostics));
    });
    ```

- [ ] **Step 2: Run it, expect FAIL.** Before narrowing, the phase-1 filter's `!isBuiltinOnDisk(d)` leaves `s2script/frmae` (a `pluginDependency`, not on disk) in the ambient-stub list → typed `any` → `r.ok === true`.
  ```bash
  cd packages/cli && node --experimental-strip-types --no-warnings --test test/typecheck.test.mjs
  ```
  Expected: the new test FAILS (`ok` is true).

- [ ] **Step 3: Implement the shape-based filter** in `typecheck.ts`. Replace `isBuiltinOnDisk`/`capOf` with a rule that keys on shape — `s2script/*` and `@s2script/cs2` are always resolve-or-error (never stubbed); everything else falls through to the stub:
  ```ts
  // Shape-based (post-consolidation): builtins are `s2script/<cap>` and the game package is the scoped
  // `@s2script/cs2` — both live in npm `dependencies` and resolve via `paths` below (miss = TS2307,
  // a real error). Only presence-conditional inter-plugin interfaces (declared in pluginDependencies)
  // stub to `any` until fetched. No disk guess — the disk-existence check is gone (the finding fix).
  const isAlwaysResolved = (d: string): boolean =>
    d.startsWith("s2script/") || d === "@s2script/cs2" || d.startsWith("@s2script/cs2/");
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
        "s2script/*": ["s2script/*.d.ts"],
        "@s2script/*": ["*/index.d.ts"],   // now serves only @s2script/cs2 (packages/cs2/index.d.ts)
      },
  ```
  and simplify the globals rootName + `isBuiltinOnDisk`-both branches removed. The globals rootName is now unconditionally the consolidated path:
  ```ts
    const rootNames = [entry, join(packagesDir, "s2script", "globals.d.ts"), ...localDts];
  ```

- [ ] **Step 5: Collapse `packages-resolve.ts` to the consolidated shape only** (drop the legacy `entity/index.d.ts` etc. sniff — those dirs are gone):
  ```ts
  export function isPackagesDir(dir: string): boolean {
    const abs = resolve(dir);
    return (
      existsSync(join(abs, "s2script", "globals.d.ts")) ||
      existsSync(join(abs, "s2script", "entity.d.ts"))
    );
  }
  ```
  and `tsconfig.base.json:12`:
  ```json
      "paths": { "s2script/*": ["s2script/*.d.ts"], "@s2script/*": ["*/index.d.ts"] },
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
rule (s2script/* + @s2script/cs2 resolve-or-error, pluginDependencies stub-until-
fetched) — the disk guess that made typecheck.ts:76 unfixable is gone. Legacy .s2sp
load-side test pins the phantom-lazy-hard-dep posture.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
  ```

---

## PR C4 (Phase 3): `packaging-consolidation/rename-root-package` — free the name

**Files:** Modify: `package.json:2` (root)

**Gate that proves this PR:** `npm install --package-lock-only --ignore-scripts` succeeds (workspaces still resolve); the name `s2script` is now claimable by `packages/s2script`.

- [ ] **Step 1: Rename the private root package off `"s2script"`.** Change `"name": "s2script"` → `"name": "s2script-monorepo"` (private root, never published; frees the name for `packages/s2script`). Keep `private: true`, `workspaces`, scripts.

- [ ] **Step 2: Verify the workspace still installs.**
  ```bash
  npm install --package-lock-only --ignore-scripts
  git diff --stat package-lock.json   # lockfile updates for the rename, no errors
  ```

- [ ] **Step 3: Commit.**
  ```bash
  git add package.json package-lock.json
  gt create packaging-consolidation/rename-root-package -m "consolidation: rename private root package off 's2script'

Frees the unscoped name for packages/s2script (the real consolidated package).

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
  ```

---

## PR C5 (Phase 3): `packaging-consolidation/republish-cs2-deprecate-cli` — re-point the game package

`@s2script/cs2`'s own `package.json` still pins deleted stub versions (`@s2script/entity: 0.3.0`, …) and its `index.d.ts` still imports `@s2script/entity`/`math`/`trace`/`events`. Re-point both at `s2script`, and deprecate `@s2script/cli` (its CLI now ships in `s2script`).

**Files:**
- Modify: `packages/cs2/package.json` (`dependencies`), `packages/cs2/index.d.ts` (the 6 cross-imports)
- Modify: `packages/cli/package.json` (deprecation marker / README note)

**Interfaces:**
- Consumes: `@s2script/cs2`'s `.d.ts` importing builtins
- Produces: `import type { EntityRef } from "s2script/entity"` etc.; `dependencies: { "s2script": "^0.1.0" }`

**Gate that proves this PR:** `./scripts/check-plugins-typecheck.sh` green (the `@s2script/cs2` consumers still resolve `Player`/`Pawn` through the re-pointed package); `cargo test -p s2script-core` unchanged.

- [ ] **Step 1: Re-point `packages/cs2/package.json` deps.** Replace the four exact stub pins with a single `s2script`:
  ```json
    "dependencies": { "s2script": "^0.1.0" },
  ```

- [ ] **Step 2: Rewrite the cross-imports in `packages/cs2/index.d.ts`** (grep-derived — currently lines 8, 9, 10, 15, 136, 137, 138):
  ```bash
  grep -nE 'from "@s2script/' packages/cs2/index.d.ts   # confirm the set
  sed -i -E 's#(from ")@s2script/(entity|math|trace|events)(")#\1s2script/\2\3#g' packages/cs2/index.d.ts
  grep -nE 'from "@s2script/' packages/cs2/index.d.ts || echo "clean"
  ```
  Expected: `clean`. (`@s2script/cs2`'s own name is unchanged — only the builtins it imports move to `s2script/*`.)

  (`@s2script/cli` is left alone here — its physical absorption into `s2script` and deprecation happen in **PR C6**, the final PR. Until C6 lands, `s2script`'s Part-A forwarding bin keeps `npx s2script build` working.)

- [ ] **Step 4: Typecheck a cs2 consumer + full plugin gate, expect PASS.**
  ```bash
  ./scripts/check-plugins-typecheck.sh
  ```
  Expected: green — `plugins/basecommands` etc. resolve `Player` through the re-pointed `@s2script/cs2`.

- [ ] **Step 5: Changeset + commit.**
  ```bash
  npm run changeset   # patch/minor: @s2script/cs2 re-pointed at s2script
  git add -A
  gt create packaging-consolidation/republish-cs2 -m "consolidation: re-point @s2script/cs2 at s2script

@s2script/cs2 dropped its exact stub pins for a single s2script dep and rewrote its
.d.ts imports to s2script/* — else the published game package dangles on deleted stub
versions.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
  ```

---

## PR C6 (Phase 3): `packaging-consolidation/absorb-cli` — the CLI ships in `s2script`

Completes Fork 2 (`s2script` = types + CLI, the `typescript`/`tsc` model). Physically move the CLI into `packages/s2script/`, point `s2script`'s bin at its OWN built CLI, drop the `@s2script/cli` dependency (and the Part-A forwarding shim), and make `@s2script/cli` a true deprecated redirect. **This is the LAST PR** — it runs after every PR that edits `packages/cli/src/*` (C1–C3), so those earlier PRs keep their `packages/cli/...` paths and this one does the move + reference rewrite in a single atomic step.

**Files:**
- Move (git mv): `packages/cli/{src,test,build.mjs,tsconfig.json,CHANGELOG.md}` → `packages/s2script/`
- Modify: `packages/s2script/package.json` (bin → own CLI, add CLI deps + scripts, drop `@s2script/cli` dep); delete `packages/s2script/{forward.cjs,bin/s2script.cjs}` (the Part-A forwarding shim)
- Modify (reference rewrites `packages/cli/` → `packages/s2script/`): `scripts/check-schema-generated.sh`, `scripts/check-nav-generated.sh`, `scripts/check-events-generated.sh`, `scripts/check-csitem-generated.sh`, `scripts/check-plugins-typecheck.sh`, `scripts/publish-packages.sh`, `scripts/build-base-plugins.sh`, `scripts/bootstrap-npm-trusted-publishing.sh`, the `tsconfig.base.json` comment
- Delete: `packages/cli/` (now a deprecated redirect — see Step 6)

**Interfaces:**
- Consumes: the CLI at `packages/cli/dist/cli.js` + `packages/cli/src/typecheck/typecheck.ts` (the paths every script and the Part-A bin use today)
- Produces: the CLI at `packages/s2script/dist/cli.js` + `packages/s2script/src/typecheck/typecheck.ts`; `s2script` bin runs its own dist; `@s2script/cli` no longer a dependency of anything

**Gate that proves this PR:** every `check-*-generated.sh` still green (they now invoke `packages/s2script/dist/cli.js`); `./scripts/check-plugins-typecheck.sh` green (it now imports `packages/s2script/src/typecheck/typecheck.ts`); `cd packages/s2script && npm test` (the moved CLI test suite); `./scripts/build-base-plugins.sh` builds; `npx s2script build` in a clean dir runs the real CLI (no forwarding, no `@s2script/cli`).

- [ ] **Step 1: git-move the CLI into the package.**
  ```bash
  cd /home/gkh/projects/s2script
  git mv packages/cli/src packages/s2script/src
  git mv packages/cli/test packages/s2script/test-cli   # avoids colliding with any s2script test/ dir; rename in-package next
  git mv packages/cli/build.mjs packages/s2script/build.mjs
  git mv packages/cli/tsconfig.json packages/s2script/tsconfig.json
  # If packages/s2script has no test/ yet, prefer the plain name:
  # git mv packages/s2script/test-cli packages/s2script/test
  ```
  Verify: `ls packages/s2script/src/typecheck/typecheck.ts` exists.

- [ ] **Step 2: Fold the CLI's `package.json` into `s2script`'s.** From `@s2script/cli`'s `package.json`, copy into `packages/s2script/package.json`: the `dependencies` (`esbuild`, `adm-zip`, `typescript`), the `devDependencies` (`@types/adm-zip`), the `build`/`test` scripts, and set `"type": "module"` (the CLI is ESM). Change the bin to the CLI's own built entry and REMOVE the `@s2script/cli` dependency:
  ```json
  "type": "module",
  "bin": { "s2script": "dist/cli.js" },
  "scripts": {
    "build": "node build.mjs",
    "test": "node --experimental-strip-types --no-warnings --test test/*.test.mjs"
  }
  ```
  Update `files` to include `dist` and the moved sources as the CLI shipped them (keep the `*.d.ts` types entries + `exports` map from C1). Then delete the Part-A forwarding shim:
  ```bash
  git rm packages/s2script/forward.cjs packages/s2script/bin/s2script.cjs packages/s2script/test/forward.test.mjs 2>/dev/null || true
  rmdir packages/s2script/bin 2>/dev/null || true
  ```

- [ ] **Step 3: Rewrite every script/config reference `packages/cli` → `packages/s2script`.** Derive the set (do not trust the list), then rewrite:
  ```bash
  grep -rn 'packages/cli' scripts/ tsconfig.base.json | grep -v node_modules
  sed -i 's#packages/cli/#packages/s2script/#g' \
    scripts/check-schema-generated.sh scripts/check-nav-generated.sh \
    scripts/check-events-generated.sh scripts/check-csitem-generated.sh \
    scripts/publish-packages.sh scripts/build-base-plugins.sh \
    scripts/bootstrap-npm-trusted-publishing.sh
  # check-plugins-typecheck.sh imports ./packages/cli/src/typecheck/typecheck.ts:
  sed -i 's#packages/cli/src/typecheck#packages/s2script/src/typecheck#g' scripts/check-plugins-typecheck.sh
  # tsconfig.base.json has a prose comment naming packages/cli/src/typecheck/typecheck.ts:
  sed -i 's#packages/cli/src/typecheck#packages/s2script/src/typecheck#g' tsconfig.base.json
  grep -rn 'packages/cli' scripts/ tsconfig.base.json | grep -v node_modules || echo "clean"
  ```
  Expected: `clean` (also confirm `bootstrap-npm-trusted-publishing.sh`'s `@s2script/cli` package-name check is updated/removed — that script lists publishable packages).

- [ ] **Step 4: Build the moved CLI and run its tests, expect PASS.**
  ```bash
  ( cd packages/s2script && node build.mjs >/dev/null )
  test -f packages/s2script/dist/cli.js && echo "built"
  ( cd packages/s2script && npm test )
  ```
  Expected: `built`; the moved CLI test suite passes (`# fail 0`).

- [ ] **Step 5: Run the generated-checks + plugin gate against the new location.**
  ```bash
  ./scripts/check-schema-generated.sh && ./scripts/check-nav-generated.sh
  ./scripts/check-events-generated.sh && ./scripts/check-csitem-generated.sh
  ./scripts/check-plugins-typecheck.sh
  ./scripts/build-base-plugins.sh
  ```
  Expected: all green — they now drive `packages/s2script/dist/cli.js` and import the moved `typecheck.ts`.

- [ ] **Step 6: Turn `@s2script/cli` into a deprecated redirect.** Remove the workspace package (nothing in-repo depends on it now) and record the published-package deprecation as a manual step:
  ```bash
  git rm -r packages/cli
  ```
  Add to the "Manual steps" note: at publish time run `npm deprecate @s2script/cli "The CLI now ships in the s2script package — npm i -D s2script"`. The published `@s2script/cli@0.2.x` stays on npm (deprecated); it is simply no longer produced from this repo.

- [ ] **Step 7: Clean-dir smoke of the real CLI (no forwarding).** After publish (manual), `npx s2script build` in an empty dir runs `s2script`'s OWN `dist/cli.js` — not a forward to `@s2script/cli`, which is no longer a dependency. In-repo, prove the bin resolves to the built CLI:
  ```bash
  node packages/s2script/dist/cli.js --help 2>&1 | head -c 120   # the CLI's own help/usage, not a forward
  ```

- [ ] **Step 8: Changeset + commit.**
  ```bash
  npm run changeset   # minor `s2script`: absorb the CLI (types + CLI in one package); @s2script/cli deprecated
  git add -A
  gt create packaging-consolidation/absorb-cli -m "consolidation: absorb the CLI into s2script (types + CLI, one package)

Move packages/cli/{src,test,build.mjs,tsconfig} into packages/s2script/, point the
s2script bin at its own built dist/cli.js, drop the @s2script/cli dependency and the
Part-A forwarding shim, and rewrite the ~8 script/config refs from packages/cli to
packages/s2script. @s2script/cli becomes a deprecated redirect (git-removed here;
npm deprecate at publish). Completes Fork 2: one install gives types AND the CLI.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
  ```

- [ ] **Step 9: Submit the whole stack.**
  ```bash
  gt restack && gt submit --no-interactive
  ```

---

## Stack summary (dependency order)

1. `packaging-consolidation/dual-resolve` (Phase 1 — the trap PR; canary is the proof)
2. `packaging-consolidation/migrate-<plugin>` ×N (Phase 2 — grep-derived batch count, one per plugin)
3. `packaging-consolidation/cs2-require-literals` (Phase 2 — live Docker CS2 gate, `pawn.origin != null`)
4. `packaging-consolidation/remove-legacy-prefix` (Phase 3 — delete stubs + BUILTIN_MODULES, honest filter)
5. `packaging-consolidation/rename-root-package` (Phase 3)
6. `packaging-consolidation/republish-cs2` (Phase 3 — re-point the game package at `s2script`)
7. `packaging-consolidation/absorb-cli` (Phase 3, LAST — CLI ships in `s2script`, `@s2script/cli` deprecated)
