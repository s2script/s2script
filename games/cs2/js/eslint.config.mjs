// Lint config for games/cs2/js/ — the CS2 game-package prelude.
//
// SAME GATE, SAME REASON as core/js/eslint.config.mjs, and it exists because that one did NOT cover
// this directory. A `no-undef` gate scoped to core/js was cited during review as protecting pawn.js;
// it never did. pawn.js runs in the raw context scope (not the plugin CJS wrapper), so it reaches
// core's natives as bare globals exactly like the core prelude does — and a native renamed in Rust
// would break it just as silently.
//
// The globals are DERIVED from core's own source, never hand-listed: a checked-in literal list
// drifts and turns the gate into decoration.
import { readFileSync } from "node:fs";

const root = new URL("../../../", import.meta.url);
const v8host = readFileSync(new URL("core/src/v8host.rs", root), "utf8");

// Every file eslint will lint here, so a global one of them publishes and another reads back is
// visible to scope analysis. The shipped prelude is a CONCATENATION (package-addon.sh) — these are
// separate files on disk but one script at runtime, which is exactly why cross-file globals are
// legitimate here and invisible to a per-file linter.
const sources = ["pawn.js", "weapon.js", "schema.generated.js", "nav.generated.js"]
  .map((f) => {
    try { return readFileSync(new URL(`./${f}`, import.meta.url), "utf8"); }
    catch { return ""; }        // a generated file may be absent before codegen runs
  })
  .join("\n");

const readonlyGlobals = (names) =>
  Object.fromEntries([...new Set(names)].sort().map((n) => [n, "readonly"]));

const natives = [...v8host.matchAll(/set_native\(scope, global_obj, "([^"]+)"/g)].map((m) => m[1]);
const extraFromRust = ["console"];   // v8host.rs — set directly on the global object
const selfPublished = [...sources.matchAll(/globalThis\.(\w+)\s*=/g)].map((m) => m[1]);

if (natives.length === 0) {
  throw new Error("games/cs2/js/eslint.config.mjs: found no set_native globals in v8host.rs — the regex is stale");
}

export default [
  {
    // activity.js is dual-use — it ships concatenated into the prelude AND is unit-tested under
    // node (`scripts/check-activity-test.sh`), so its `typeof module !== "undefined"` guard and its
    // test file legitimately reference CommonJS globals. Scoped narrowly rather than adding them to
    // the prelude's own globals, where they would mask a real `require` creeping into raw-context code.
    files: ["activity.js", "activity.test.js"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "script",
      globals: { module: "readonly", require: "readonly", exports: "readonly", globalThis: "readonly" },
    },
    rules: { "no-undef": "error", "no-unused-vars": ["error", { args: "none", caughtErrors: "none" }] },
  },
  {
    files: ["**/*.js"],
    ignores: ["activity.js", "activity.test.js"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "script",
      globals: readonlyGlobals([...natives, ...extraFromRust, ...selfPublished]),
    },
    linterOptions: { reportUnusedDisableDirectives: true },
    rules: {
      "no-undef": "error",
      "no-unused-vars": ["error", { args: "none", caughtErrors: "none" }],
    },
  },
];
