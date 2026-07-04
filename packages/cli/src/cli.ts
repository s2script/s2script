import { buildPlugin } from "./build.ts";
import { runGenSchema } from "./schemagen/gen.ts";
import { runGenEvents } from "./eventgen/gen.ts";
import { runGenNav } from "./navgen/gen.ts";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const [command, arg] = process.argv.slice(2);

if (command === "gen-schema") {
  const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");   // dist/ → packages/cli → packages → repo
  const check = arg === "--check";
  const r = runGenSchema(repoRoot, { check });
  if (check) {
    if (r.drift.length) { console.error(`FAIL: generated files out of date — run \`s2script gen-schema\`:\n  ${r.drift.join("\n  ")}`); process.exit(1); }
    console.log(`schema codegen up to date (${r.classes} classes, ${r.fields} fields, ${r.skipped} skipped)`);
  } else {
    console.log(`gen-schema: wrote ${r.classes} classes, ${r.fields} fields (${r.skipped} skipped)`);
  }
} else if (command === "gen-events") {
  const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");   // dist/ → packages/cli → packages → repo
  const check = arg === "--check";
  const r = runGenEvents(repoRoot, { check });
  if (check) {
    if (r.drift.length) { console.error(`FAIL: generated files out of date — run \`s2script gen-events\`:\n  ${r.drift.join("\n  ")}`); process.exit(1); }
    console.log(`event codegen up to date (${r.events} events)`);
  } else {
    console.log(`gen-events: wrote ${r.events} events`);
  }
} else if (command === "gen-nav") {
  const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");   // dist/ → packages/cli → packages → repo
  const check = arg === "--check";
  const r = runGenNav(repoRoot, { check });
  if (check) {
    if (r.drift.length) { console.error(`FAIL: generated files out of date — run \`s2script gen-nav\`:\n  ${r.drift.join("\n  ")}`); process.exit(1); }
    console.log(`nav codegen up to date (${r.wrappers} wrappers, ${r.fields} fields)`);
  } else {
    console.log(`gen-nav: wrote ${r.wrappers} wrappers, ${r.fields} fields`);
  }
} else if (command === "build" && arg) {
  const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");   // dist/ → packages/cli → packages → repo
  try { console.log(await buildPlugin(arg, join(repoRoot, "packages"))); }
  catch (e) { console.error(String(e instanceof Error ? e.message : e)); process.exit(1); }
} else {
  console.error("Usage: s2script build <dir> | s2script gen-schema [--check] | s2script gen-events [--check] | s2script gen-nav [--check]");
  process.exit(1);
}
