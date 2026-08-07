#!/usr/bin/env node
// Generate translations/<name>.phrases.json from each plugin's in-code English seed.
//
// The seed is the single source of truth and lives in TypeScript; the shipped JSON is what an
// OPERATOR edits (the root file overrides the seed at load — see core/js/prelude.js's
// Translations.load). Generating rather than hand-writing makes drift between the two impossible.
//
// translations/ is otherwise a drop-in folder, exactly like SourceMod's addons/sourcemod/translations/:
// operators and third-party plugin authors put files there directly (translations/common.phrases.json
// is one such hand-authored file — it has no seed and this script never touches it). A generator has
// no business deleting files it did not create, so this script only ever writes/checks the files it
// can produce from a seed; anything else in the folder is none of its business.
//
// Two validation passes run over every plugin's src/**, in BOTH modes (not just --check), because a
// bad key is a source-code defect, not generated-file drift:
//   - unknown key:  Translations.translate(slot, "Key") / cmd.replyT("Key") where "Key" is a string
//     literal that resolves in neither the plugin's own seed nor translations/common.phrases.json.
//     That call would render the literal key text to a player, so this is a hard error.
//   - shadowed key: a seed key that ALSO exists in translations/common.phrases.json. Legal — own-set-
//     first means the seed wins deterministically — but almost always unintended: an operator editing
//     the shared file would see no effect for that key. Reported, never failed.
// Only string-literal keys are recognised (a ternary between two literals, e.g. the
// `n === 1 ? "X" : "Y"` singular/plural idiom, is resolved to both branches — see
// collectKeyLiterals); a key built some other dynamic way (a variable, a template with `${}`)
// doesn't match the scan and is silently skipped rather than guessed at — a miss here is
// preferable to a false positive.
//
// Run:  node --experimental-strip-types scripts/gen-phrases.mjs           # write
//       node --experimental-strip-types scripts/gen-phrases.mjs --check   # exit 1 on drift, write nothing
import { readdirSync, existsSync, readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join, dirname, relative } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import ts from "typescript";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(ROOT, "translations");
const COMMON_FILE = join(OUT, "common.phrases.json");
const check = process.argv.includes("--check");

function rel(p) {
  return relative(ROOT, p);
}

function pluginDirs() {
  const out = [];
  for (const base of ["plugins", join("plugins", "disabled")]) {
    const abs = join(ROOT, base);
    if (!existsSync(abs)) continue;
    for (const name of readdirSync(abs, { withFileTypes: true })) {
      if (!name.isDirectory() || name.name === "disabled") continue;
      const seed = join(abs, name.name, "src", "phrases.ts");
      if (existsSync(seed)) out.push({ name: name.name, seed });
    }
  }
  return out.sort((a, b) => a.name.localeCompare(b.name));
}

async function loadSeed(file) {
  // Imported, not text-parsed: generating therefore validates the seed as real TypeScript.
  // Requires node >= 22 (--experimental-strip-types), which package.json already pins.
  const mod = await import(pathToFileURL(file).href);
  const seed = mod.phrases ?? mod.default;
  // typeof [] === "object", so an exported array must be rejected explicitly — it would
  // otherwise render as a JSON object with numeric string keys instead of failing loudly.
  if (!seed || typeof seed !== "object" || Array.isArray(seed)) {
    throw new Error(`${file}: expected an exported \`phrases\` object`);
  }
  for (const [key, value] of Object.entries(seed)) {
    if (typeof value !== "string") {
      throw new Error(`${file}: phrase "${key}" must be a string, got ${typeof value}`);
    }
  }
  return seed;
}

function render(seed) {
  // Keys sorted so the generated file has a stable diff regardless of source order.
  const sorted = {};
  for (const k of Object.keys(seed).sort()) sorted[k] = seed[k];
  return JSON.stringify(sorted, null, 2) + "\n";
}

// translations/common.phrases.json is hand-authored (no seed, never generated — see Step 1/2 of
// the base-plugin-phrases slice), but its keys are still half of "is this literal key valid".
// Missing/malformed is degraded to "no shared keys" rather than a crash: a fresh checkout mid-work
// or a stripped fixture tree should not make an unrelated plugin's key-usage scan explode.
function readCommonKeys() {
  if (!existsSync(COMMON_FILE)) return new Set();
  try {
    const obj = JSON.parse(readFileSync(COMMON_FILE, "utf8"));
    return new Set(obj && typeof obj === "object" && !Array.isArray(obj) ? Object.keys(obj) : []);
  } catch {
    return new Set();
  }
}

// A real parse, not a regex: a text-level scan cannot tell a call from a comment or a string
// literal that merely CONTAINS the text of a call, so it would false-positive on exactly the
// artifact 17-plugin conversions produce most often — a commented-out old call sitting next to
// its replacement. `ts.createSourceFile` (syntax-only; no type checker, no program, no disk I/O
// beyond the read already done) gives every call expression as a real AST node, so a match here is
// a genuine call, by construction, not by how well a pattern avoids matching too much.
//
// Only `<something>.replyT(...)` / `<something>.translate(...)` are recognised — the OBJECT is not
// checked (so `someTestMock.replyT("key")` still counts as a usage; deliberately not filtered out,
// since it is a real call to something named replyT and a text scan couldn't tell it apart from the
// real thing either). The key argument must resolve to one or more genuine string-literal nodes (see
// collectKeyLiterals below); anything else — a variable, a template with `${}` — is a dynamic key and
// is skipped rather than guessed at. Argument position differs by call shape: replyT(key, ...) vs
// translate(slot, key, ...args).
const KEY_ARG_INDEX = { replyT: 0, translate: 1 };

// Resolve a key-argument expression to every string literal it could actually evaluate to.
//   - a plain string literal (`ts.isStringLiteralLike`, which also covers a no-substitution template
//     literal): itself, one literal.
//   - a ternary (`cond ? "A" : "B"`), the `cmd.replyT(n === 1 ? "X" : "Y", n)` idiom used throughout
//     these plugins for singular/plural phrase pairs: recurse into BOTH branches, so a nested/chained
//     ternary (`a ? x : b ? y : z`, which parses as `a ? x : (b ? y : z)`) is walked all the way down.
//   - parenthesized (`(cond ? "A" : "B")`): unwrap and recurse.
//   - anything else (a variable, a function call, a template with `${}`): a genuinely dynamic key —
//     returns no literals, same as before. A miss here is preferable to a false positive.
function collectKeyLiterals(node) {
  if (!node) return [];
  if (ts.isStringLiteralLike(node)) return [node.text];
  if (ts.isConditionalExpression(node)) {
    return [...collectKeyLiterals(node.whenTrue), ...collectKeyLiterals(node.whenFalse)];
  }
  if (ts.isParenthesizedExpression(node)) return collectKeyLiterals(node.expression);
  return [];
}

function findPhraseKeyUsages(srcDir) {
  const usages = [];
  if (!existsSync(srcDir)) return usages;
  for (const e of readdirSync(srcDir, { recursive: true, withFileTypes: true })) {
    if (!e.isFile() || !/\.(ts|tsx|mts|cts)$/.test(e.name)) continue;
    const file = join(e.parentPath ?? e.path ?? srcDir, e.name);
    const text = readFileSync(file, "utf8");
    const scriptKind = /\.tsx$/.test(e.name) ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
    const sourceFile = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, false, scriptKind);
    (function visit(node) {
      if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression)) {
        const argIndex = KEY_ARG_INDEX[node.expression.name.text];
        if (argIndex !== undefined) {
          const arg = node.arguments[argIndex];
          for (const key of collectKeyLiterals(arg)) usages.push({ file, key });
        }
      }
      ts.forEachChild(node, visit);
    })(sourceFile);
  }
  return usages;
}

const targets = pluginDirs();

// "common" is reserved: a plugin directory of that name would generate common.phrases.json and
// silently overwrite the hand-authored shared file — the exact harm Step 1 removed the orphan
// sweep to prevent, arriving through the other door (an overwrite, not a delete). Checked before
// any write/check work starts, in both modes.
const commonCollision = targets.find((t) => t.name === "common");
if (commonCollision) {
  console.error(
    `ERROR: ${rel(dirname(dirname(commonCollision.seed)))} is a plugin directory named "common" — ` +
      `generating it would overwrite the hand-authored ${rel(COMMON_FILE)}. "common" is reserved; ` +
      `rename the plugin.`,
  );
  process.exit(1);
}

const commonKeys = readCommonKeys();

let drift = 0;
let keyErrors = 0;
// --check must be read-only: only the write path is allowed to create translations/. Creating it
// unconditionally would leave a directory behind on a --check run in a worktree that has none.
if (!check) mkdirSync(OUT, { recursive: true });

const producedNames = new Set();
for (const t of targets) {
  if (!existsSync(t.seed)) continue;
  producedNames.add(`${t.name}.phrases.json`);
  const seed = await loadSeed(t.seed);
  const ownKeys = new Set(Object.keys(seed));
  const text = render(seed);
  const dest = join(OUT, `${t.name}.phrases.json`);
  const current = existsSync(dest) ? readFileSync(dest, "utf8") : null;
  if (current !== text) {
    if (check) {
      console.error(`DRIFT: ${dest} does not match ${t.seed}`);
      drift++;
    } else {
      writeFileSync(dest, text);
      console.log(`wrote ${dest}`);
    }
  }

  // Unknown-key scan: every literal key this plugin's src/** passes to Translations.translate/
  // .replyT must resolve in its own seed or the shared file. A miss ships raw key text to a
  // player, so this fails the run — in both --check and write mode.
  for (const usage of findPhraseKeyUsages(dirname(t.seed))) {
    if (ownKeys.has(usage.key) || commonKeys.has(usage.key)) continue;
    console.error(
      `ERROR: ${rel(usage.file)}: unknown phrase key "${usage.key}" ` +
        `(not in ${rel(t.seed)} or ${rel(COMMON_FILE)})`,
    );
    keyErrors++;
  }

  // Shadow report: a seed key that duplicates a common.phrases.json key is legal (own-set-first
  // always resolves it locally) but almost always unintended — an operator editing the shared
  // file would see no effect on this key with no diagnostic anywhere. Warn, never fail.
  for (const k of ownKeys) {
    if (!commonKeys.has(k)) continue;
    console.log(
      `NOTE: ${rel(t.seed)}: seed key "${k}" duplicates ${rel(COMMON_FILE)} — the shared phrase ` +
        `is shadowed and can never be reached for this key`,
    );
  }
}

// Stale-file report: a top-level translations/*.phrases.json with no corresponding seed is
// EITHER a leftover from a deleted plugin seed OR a file this script never produced in the first
// place (an operator override, a third-party plugin's own phrases file, the hand-authored
// common.phrases.json). This script cannot tell those apart, so it only ever reports — never
// deletes — anything it did not just write. translations/ is a drop-in folder, exactly like
// SourceMod's addons/sourcemod/translations/: operators and third-party authors are meant to put
// files there directly, and a generator has no business removing files it did not create. This is
// therefore purely informational and never fails --check. Only the top level is inspected:
// translations/<lang>/... subdirectories are hand-written by translators (arriving in a later
// task) and must never be touched here — readdirSync with withFileTypes + isFile() only sees
// direct children, never recurses, so a subdirectory and its contents are structurally
// unreachable by this report.
if (existsSync(OUT)) {
  const existing = readdirSync(OUT, { withFileTypes: true }).filter(
    (e) => e.isFile() && e.name.endsWith(".phrases.json"),
  );
  const stale = existing.filter((e) => !producedNames.has(e.name));

  for (const entry of stale) {
    const dest = join(OUT, entry.name);
    // Worded as steady-state-normal, not as a problem: this fires on every single run for
    // common.phrases.json (permanently un-generated by design) and for any operator/third-party
    // file dropped alongside it, so it must not read like something is wrong.
    console.log(`NOTE: ${dest} not generated by this run (hand-authored or third-party) — left untouched`);
  }
}

if (keyErrors > 0) {
  console.error(`\n${keyErrors} unknown phrase key reference(s) — see ERROR lines above.`);
}
if (check && drift > 0) {
  console.error(
    `\n${drift} phrases file(s) out of date — run: node --experimental-strip-types scripts/gen-phrases.mjs`,
  );
}
if (keyErrors > 0 || (check && drift > 0)) process.exit(1);
if (check) console.log("PASS: phrases files are up to date");
