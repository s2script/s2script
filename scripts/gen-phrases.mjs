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
// Run:  node --experimental-strip-types scripts/gen-phrases.mjs           # write
//       node --experimental-strip-types scripts/gen-phrases.mjs --check   # exit 1 on drift, write nothing
import { readdirSync, existsSync, readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(ROOT, "translations");
const check = process.argv.includes("--check");

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

const targets = pluginDirs();

let drift = 0;
// --check must be read-only: only the write path is allowed to create translations/. Creating it
// unconditionally would leave a directory behind on a --check run in a worktree that has none.
if (!check) mkdirSync(OUT, { recursive: true });

const producedNames = new Set();
for (const t of targets) {
  if (!existsSync(t.seed)) continue;
  producedNames.add(`${t.name}.phrases.json`);
  const text = render(await loadSeed(t.seed));
  const dest = join(OUT, `${t.name}.phrases.json`);
  const current = existsSync(dest) ? readFileSync(dest, "utf8") : null;
  if (current === text) continue;
  if (check) {
    console.error(`DRIFT: ${dest} does not match ${t.seed}`);
    drift++;
  } else {
    writeFileSync(dest, text);
    console.log(`wrote ${dest}`);
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
    console.log(`NOTE: ${dest} was not generated by this run (no matching seed) — left untouched`);
  }
}

if (check && drift > 0) {
  console.error(
    `\n${drift} phrases file(s) out of date — run: node --experimental-strip-types scripts/gen-phrases.mjs`,
  );
  process.exit(1);
}
if (check) console.log("PASS: phrases files are up to date");
