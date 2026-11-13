#!/usr/bin/env node
// docs/PROGRESS.md fragments, changesets-style.
//
// WHY THIS EXISTS. Every finished slice used to prepend its entry to the top of
// docs/PROGRESS.md, so any two PRs open at once edited the same lines and the second to merge
// always conflicted (three PRs at once on Nov 8, 2026). Now each PR adds its own file under
// docs/progress/ and never touches PROGRESS.md; `fold` moves the fragments into PROGRESS.md
// later, in one commit nobody else is editing (the changesets "version packages" PR runs it).
//
//   node scripts/progress.mjs check   validate fragment names and contents (ci-js gate)
//   node scripts/progress.mjs fold    prepend fragments to PROGRESS.md (newest first) and delete them
//
// Fragment: docs/progress/YYYY-MM-DD-<slug>.md, holding exactly the entry text as it should
// appear in PROGRESS.md (normally one bold-led paragraph). README.md is not a fragment.
import { readFileSync, writeFileSync, readdirSync, rmSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = process.env.S2_PROGRESS_ROOT ?? join(dirname(fileURLToPath(import.meta.url)), "..");
const DIR = join(ROOT, "docs/progress");
const LOG = join(ROOT, "docs/PROGRESS.md");
const ANCHOR = "## Current state (full history)\n";
const NAME = /^\d{4}-\d{2}-\d{2}-[a-z0-9][a-z0-9-]*\.md$/;

function fragments() {
  let names;
  try { names = readdirSync(DIR); } catch { return { ok: [], bad: [] }; }
  const ok = [], bad = [];
  for (const n of names) {
    if (n === "README.md" || n.startsWith(".")) continue;
    if (!NAME.test(n)) { bad.push(`${n}: name must be YYYY-MM-DD-<lowercase-slug>.md`); continue; }
    const body = readFileSync(join(DIR, n), "utf8").trim();
    if (!body) { bad.push(`${n}: empty`); continue; }
    if (/^#{1,2} /m.test(body)) { bad.push(`${n}: no '#'/'##' headings; an entry sits under PROGRESS.md's own heading`); continue; }
    ok.push({ name: n, body });
  }
  // Newest first, matching PROGRESS.md's order; same-day entries in reverse name order.
  ok.sort((a, b) => (a.name < b.name ? 1 : a.name > b.name ? -1 : 0));
  return { ok, bad };
}

const cmd = process.argv[2];
const { ok, bad } = fragments();
if (bad.length) {
  console.error("progress: bad fragment(s) in docs/progress/:\n  " + bad.join("\n  "));
  process.exit(1);
}

if (cmd === "check") {
  const log = readFileSync(LOG, "utf8");
  if (!log.includes(ANCHOR)) {
    console.error(`progress: docs/PROGRESS.md lost its '${ANCHOR.trim()}' heading; fold needs it`);
    process.exit(1);
  }
  console.log(`progress: ok (${ok.length} unfolded fragment${ok.length === 1 ? "" : "s"})`);
} else if (cmd === "fold") {
  if (!ok.length) { console.log("progress: nothing to fold"); process.exit(0); }
  const log = readFileSync(LOG, "utf8");
  const at = log.indexOf(ANCHOR);
  if (at < 0) { console.error(`progress: no '${ANCHOR.trim()}' heading in PROGRESS.md`); process.exit(1); }
  const head = log.slice(0, at + ANCHOR.length);
  const tail = log.slice(at + ANCHOR.length).replace(/^\n+/, "");
  writeFileSync(LOG, head + "\n" + ok.map((f) => f.body + "\n\n").join("") + tail);
  for (const f of ok) rmSync(join(DIR, f.name));
  console.log(`progress: folded ${ok.length} fragment(s) into docs/PROGRESS.md`);
} else {
  console.error("usage: node scripts/progress.mjs check|fold");
  process.exit(2);
}
