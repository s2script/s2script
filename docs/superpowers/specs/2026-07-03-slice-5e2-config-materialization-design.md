# Slice 5E.2 — config materialization (design)

**Goal:** Give plugins settings. A plugin DECLARES typed config in `package.json`; the host MATERIALIZES
it at load (declared defaults merged with an admin-editable JSON file, auto-generated on first run); the
plugin READS it via a typed `@s2script/config` API — and, if the author opts in via `config.onChange`,
gets live-reload when the file changes (no plugin reload).

**Status:** design approved (JSON file; "author decides" reload = opt-in via `onChange`).

**Branch base:** `main` (… + 5E.1 merged).
**Cadence:** subagent-driven, merge-to-main-locally, Docker CS2 live gate.

First of the three lifecycle sub-slices ("do em all"): **config (this)** → reload-handoff → permissions.

---

## 1. Declare — the `s2script.config` block

In `package.json`:

```jsonc
"s2script": {
  "config": {
    "greeting":  { "type": "string", "default": "hello",     "description": "Message shown on join" },
    "maxUses":   { "type": "int",    "default": 3 },
    "cooldown":  { "type": "float",  "default": 1.5 },
    "enabled":   { "type": "bool",   "default": true }
  }
}
```

- Four value types: `string`/`int`/`float`/`bool` (SourceMod ConVar-parity; the value types the reads +
  the JSON round-trip cleanly). A declaration = `{ type, default, description? }`. `default` MUST match
  `type`: the **CLI validates this at build** (a mismatch fails the build with a clear message — the build
  is the gate, consistent with 5E.1); the **load-time materialize degrades as a backstop** (a mismatched
  key → WARN + skip that key, using the type zero-value).
- The CLI copies `config` into `manifest.json` (baked into the `.s2sp`), like `pluginDependencies`/
  `publishes`. No new authoring format — it's the existing `s2script` block.

## 2. Materialize — at plugin load

At load the host produces the plugin's live config = **declared defaults ⊕ the admin override file**:

- Override file: `addons/s2script/configs/<plugin-id>.json` (sanitized id, like the `.s2sp` name).
- **Auto-generate on first run:** if the file is absent, the host writes it with every declared key at its
  default + a `//`-comment carrying the `description` + type (JSONC — the shim already parses JSONC).
  Admins edit this file to tune settings; a plugin update adds new keys on the next materialize (missing
  keys are filled from defaults, never clobbering existing admin values).
- **Merge + coerce:** for each declared key, take the file value if present and coercible to the declared
  type, else the default. Coercion is strict-ish: `int`/`float` from a JSON number, `bool` from a JSON
  bool, `string` from a JSON string; a wrong-typed or malformed value → the default + a named WARN
  (degrade-never-crash — a broken config file never crashes the plugin, it falls back per-key).
- Undeclared keys in the file are ignored (forward-compat). The materialized config is a plain object
  `{ greeting: "hello", maxUses: 3, … }` injected into the plugin's context.

## 3. Read — `@s2script/config` (typed, engine-generic)

A new engine-generic module `@s2script/config`, injected per-context like the other `__s2pkg_*`:

```ts
config.getString(key: string): string
config.getInt(key: string): number
config.getFloat(key: string): number
config.getBool(key: string): boolean
config.onChange(handler: (cfg: Config) => void): void   // opt-in live-reload (see §4)
```

The getters return the materialized value coerced to the requested type (the value is already the right
type from §2; the getter is the typed surface). An **undeclared** key returns a type zero-value
(`""`/`0`/`0`/`false`) + a WARN — reads are total (never throw). Read-only: the object is materialized
once at load and re-materialized only on a live-reload (§4).

## 4. Live-reload — opt-in via `config.onChange`

"The author decides" = the *presence of an `onChange` subscription* turns live-reload on (the lazy-hook
pattern used by `OnGameFrame`/`Events`/the pre-hooks):

- A plugin that never calls `onChange` → config materialized once at load; **its config file is not
  watched** (zero overhead). This is the default/read-only mode.
- The FIRST `config.onChange(handler)` for a plugin signals the host to **watch that plugin's config
  file**, reusing the loader's existing frame-drain poll (the `.s2sp` watcher already polls on the
  GameFrame Post drain — extend it to also stat the config files of opted-in plugins). On an mtime
  change: re-read + re-materialize (§2) → replace the plugin's `__s2pkg_config` values → invoke every
  registered `onChange` handler with the new `cfg`. Auto-ledgered; the watch stops on plugin unload.
- Re-materialize is degrade-safe (a mid-edit malformed file → per-key fallback, WARN, no crash; the
  handler still fires with the best-effort config).

## 5. Components & data flow (boundary: config is engine-generic)

| Concern | Lives in | Why |
|---|---|---|
| `s2script.config` → `manifest.json` | CLI (`build.ts`) | authoring/packaging |
| Manifest `config` parse; merge/coerce; per-context inject; `onChange` mux + re-materialize | core (`loader.rs` + `v8host.rs`) | engine-generic; every plugin has config |
| Read/write the override JSON file; poll its mtime | shim (file I/O, like gamedata/pawn.js) | the host owns disk paths |
| `@s2script/config` runtime (`__s2pkg_config`) + typed getters + `onChange` | core prelude | engine-generic |

Flow: CLI bakes `config` into the manifest → at load, core parses it (defaults) + asks the shim to read
the override file (a `config_read(id)` op; `config_write(id, content)` auto-generates it) → core merges +
coerces → injects `__s2pkg_config` into the context → the plugin reads via `@s2script/config`. If a
plugin registers `onChange`, the loader poll stats the file; on change → `config_read` → re-materialize →
fire handlers.

New engine-ops (ABI-appended): `config_read(id) -> const char* | null`, `config_write(id, content) -> int`.
(The watch reuses the existing frame-drain poll; no new hook descriptor needed — the poll checks opted-in
plugins' config mtimes.)

## 6. Degrade-never-crash

Absent file → auto-generate + use defaults. Malformed file / wrong-typed key → per-key default + WARN.
A `default` whose type mismatches its `type` → skip that key with a WARN (use the type zero-value).
`config_read`/`config_write` op absent (no shim) → config is defaults-only (no file), reads still work.
Undeclared key read → zero-value + WARN. No path throws into plugin JS.

## 7. Testing & live gate

- **In-isolate (core):** materialize (defaults-only; override-merge; per-type coercion; malformed-key →
  default + WARN; undeclared-file-key ignored); the typed getters (right value/type; undeclared →
  zero-value); `onChange` collapse (multiple handlers fire on re-materialize); degrade with no ops.
- **In-isolate/vm (CLI/JS):** the CLI copies `config` into the manifest; the `@s2script/config` `.d.ts`
  + prelude shape.
- **One sniper rebuild** (the `config_read`/`config_write` ops + the prelude).
- **Live gate (de_inferno):** a demo declares config (`greeting` string, `maxUses` int, `enabled` bool);
  on first load the host **auto-generates** `addons/s2script/configs/<id>.json` with the defaults; the
  demo logs the read values (= defaults). Then: edit the file (change `greeting`), and — for the demo
  which registers `config.onChange` — observe the handler fire with the new value WITHOUT a reload; a
  second demo (no `onChange`) keeps the old value until reloaded. Degrade: delete/corrupt the file →
  defaults, server ticking, no crash.

## 8. Rough task decomposition (~5)

1. **CLI:** copy `s2script.config` into `manifest.json` + the manifest `config` type; a build/typecheck
   check that `default` matches `type` (or defer to load-time). CLI tests.
2. **Core materialization:** parse manifest `config`; the merge/coerce (defaults ⊕ file JSON) as a pure
   function; per-key degrade; in-isolate tests. (File content passed in as a string — pure + testable.)
3. **Shim + ops:** `config_read`/`config_write` op impls (read/auto-write `addons/s2script/configs/<id>.json`)
   + ABI-append (C header + Rust mirror) + wire into materialization at load.
4. **`@s2script/config` runtime + `onChange` live-reload:** the prelude module (`__s2pkg_config` +
   typed getters + `onChange`), the core `onChange` mux + re-materialize, the loader-poll config-mtime
   watch for opted-in plugins; the `.d.ts` (`packages/config`); in-isolate + vm tests.
5. **Demo + one sniper build + live gate + docs.**

## 9. Explicitly out of scope (do not build ahead)

The `.cfg`/ConVar-command format (JSON only); per-key change callbacks (whole-config `onChange` only);
config schema versioning/migration; secrets/encryption; runtime `config.set(...)` writes from JS
(admin-file is the source of truth); permissions over the config natives (the permissions sub-slice);
the reload STATE-handoff (`onUnload→onLoad(prev)` — the next sub-slice). Note later needs as TODOs and stop.
