import { buildPlugin } from "./build.ts";
import { runGenSchema } from "./schemagen/gen.ts";
import { runGenEvents } from "./eventgen/gen.ts";
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
} else if (command === "build" && arg) {
  try { console.log(await buildPlugin(arg)); }
  catch (err) { console.error(String(err)); process.exit(1); }
} else {
  console.error("Usage: s2script build <dir> | s2script gen-schema [--check] | s2script gen-events [--check]");
  process.exit(1);
}
