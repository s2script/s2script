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
import { readFileSync, readdirSync } from "node:fs";

const root = new URL("../../../", import.meta.url);

// Scans ALL of core/src, not just v8host.rs — see the same note in core/js/eslint.config.mjs. Natives
// are moving into per-feature modules under the core-stabilization program, and a config pinned to one
// file would lose a native's global the moment its feature moved out.
const coreSrcDir = new URL("core/src/", root);
const coreSrc = readdirSync(coreSrcDir, { withFileTypes: true, recursive: true })
  .filter((e) => e.isFile() && e.name.endsWith(".rs"))
  .map((e) => readFileSync(new URL(`${e.parentPath ?? e.path}/${e.name}`, coreSrcDir), "utf8"))
  .join("\n");

// The shipped prelude is a CONCATENATION — separate files on disk, one script at runtime — which is
// why a global published by one file and read bare by another is legitimate here and invisible to a
// per-file linter.
//
// The file list is READ OUT OF package-addon.sh, never written here. A hand-kept copy of it shipped
// STALE in this very file's first version: it listed four of the six files the packager concatenates,
// omitting activity.js and csitem.generated.js — while carrying a comment about how hand-kept lists
// drift. Derive it, and fail loudly if the derivation stops working.
const packager = readFileSync(new URL("scripts/package-addon.sh", root), "utf8");
const catLine = packager.match(/cat (games\/cs2\/js\/\S+(?: games\/cs2\/js\/\S+)*)\s*>/);
if (!catLine) {
  throw new Error("games/cs2/js/eslint.config.mjs: could not find the prelude `cat` line in " +
                  "scripts/package-addon.sh — this config can no longer tell which files ship");
}
const shipped = catLine[1].split(/\s+/).map((f) => f.replace(/^games\/cs2\/js\//, ""));
const sources = shipped
  .map((f) => {
    try { return readFileSync(new URL(`./${f}`, import.meta.url), "utf8"); }
    catch { return ""; }        // a generated file may be absent before codegen runs
  })
  .join("\n");

const readonlyGlobals = (names) =>
  Object.fromEntries([...new Set(names)].sort().map((n) => [n, "readonly"]));

const natives = [...coreSrc.matchAll(/set_native\(scope, global_obj, "([^"]+)"/g)].map((m) => m[1]);
const extraFromRust = ["console"];   // v8host.rs — set directly on the global object
// The globals these files publish and then read back bare later in the concatenated script.
//
// CONSTRAINED TO THE PACKAGE'S OWN NAMESPACE, deliberately. An unconstrained `(\w+)` capture runs
// over raw source, so it also matches inside COMMENTS, STRING LITERALS and `==`/`===` comparisons —
// which means any name mentioned in prose gets whitelisted, INCLUDING a native's. That is not
// hypothetical: `__s2_admin_can_target` was whitelisted here purely by a `===` comparison and
// `__s2_activity` purely by a comment, and with the whole mechanism removed the directory still
// linted clean — seven names admitted, none load-bearing, two of them arriving from prose.
// core/js/eslint.config.mjs constrains its capture for exactly this reason.
const selfPublished = [...sources.matchAll(/globalThis\.(__s2pkg_\w+)\s*=/g)].map((m) => m[1]);

if (natives.length === 0) {
  throw new Error("games/cs2/js/eslint.config.mjs: found no set_native globals in core/src — the regex is stale");
}

export default [
  {
    // activity.js is dual-use — it ships concatenated into the prelude AND is unit-tested under
    // node (`scripts/check-activity-test.sh`), so its `typeof module !== "undefined"` guard and its
    // test file legitimately reference CommonJS globals. Scoped narrowly rather than adding them to
    // the prelude's own globals, where they would mask a real `require` creeping into raw-context code.
    //
    // components.test.js is here for the same reason, but note that components.js itself is NOT:
    // it is prelude-only, so a `require` creeping into it must still fail this lint.
    files: ["activity.js", "activity.test.js", "components.test.js", "hudinput.test.js", "menuhud.test.js", "voterail.test.js", "hudkit-prelude.test.js"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "script",
      globals: {
        module: "readonly", require: "readonly", exports: "readonly", globalThis: "readonly",
        __dirname: "readonly",
      },
    },
    rules: { "no-undef": "error", "no-unused-vars": ["error", { args: "none", caughtErrors: "none" }] },
  },
  {
    files: ["**/*.js"],
    ignores: ["activity.js", "activity.test.js", "components.test.js", "hudinput.test.js", "menuhud.test.js", "voterail.test.js", "hudkit-prelude.test.js"],
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
