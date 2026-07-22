// Doc-coverage check for the author-facing .d.ts stubs (dev tool — NOT a CI gate).
// Usage: node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs [file ...]
// No file args → the full in-scope set (31 packages/sdk/*.d.ts + 2 cs2 hand-authored).
import { readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, relative } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..");
const { findUndocumented } = await import(join(repo, "packages/sdk/src/doccov/doccov.ts"));

function defaultFiles() {
  const sdkDir = join(repo, "packages/sdk");
  const sdk = readdirSync(sdkDir)
    .filter((f) => f.endsWith(".d.ts"))
    .map((f) => join(sdkDir, f));
  return [...sdk, join(repo, "packages/cs2/index.d.ts"), join(repo, "packages/cs2/weapon.d.ts")];
}

const args = process.argv.slice(2);
const files = args.length ? args : defaultFiles();
const gaps = findUndocumented(files);

if (gaps.length === 0) {
  console.log(`PASS: ${files.length} file(s) fully documented`);
  process.exit(0);
}

const byFile = new Map();
for (const g of gaps) {
  if (!byFile.has(g.file)) byFile.set(g.file, []);
  byFile.get(g.file).push(g);
}
for (const [file, gs] of byFile) {
  console.error(`\n${relative(repo, file)} — ${gs.length} undocumented:`);
  for (const g of gs) console.error(`  L${g.line}  ${g.kind} ${g.symbol}`);
}
console.error(`\nFAIL: ${gaps.length} undocumented symbol(s) across ${byFile.size} file(s)`);
process.exit(1);
