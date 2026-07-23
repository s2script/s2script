import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { runGenSchema } from "../schemagen/gen.ts";
import { runGenEvents } from "../eventgen/gen.ts";
import { runGenNav } from "../navgen/gen.ts";

export type CodegenKind = "schema" | "events" | "nav";

function repoRoot(): string {
  // Bundled, import.meta.url is always dist/cli.js: packages/sdk/dist → packages/sdk → packages → repo.
  return join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
}

/** gen-schema / gen-events / gen-nav. Output + exit codes are byte-identical to the old cli.ts
 *  branches — the `check-*-generated.sh` gates depend on them. Not interactive (codegen has no prompts). */
export async function run(kind: CodegenKind, argv: string[]): Promise<void> {
  const root = repoRoot();
  const check = argv[0] === "--check";
  if (kind === "schema") {
    const r = runGenSchema(root, { check });
    if (check) {
      if (r.drift.length) { console.error(`FAIL: generated files out of date — run \`s2s gen-schema\`:\n  ${r.drift.join("\n  ")}`); process.exit(1); }
      console.log(`schema codegen up to date (${r.classes} classes, ${r.fields} fields, ${r.skipped} skipped)`);
    } else {
      console.log(`gen-schema: wrote ${r.classes} classes, ${r.fields} fields (${r.skipped} skipped)`);
    }
  } else if (kind === "events") {
    const r = runGenEvents(root, { check });
    if (check) {
      if (r.drift.length) { console.error(`FAIL: generated files out of date — run \`s2s gen-events\`:\n  ${r.drift.join("\n  ")}`); process.exit(1); }
      console.log(`event codegen up to date (${r.events} events)`);
    } else {
      console.log(`gen-events: wrote ${r.events} events`);
    }
  } else {
    const r = runGenNav(root, { check });
    if (check) {
      if (r.drift.length) { console.error(`FAIL: generated files out of date — run \`s2s gen-nav\`:\n  ${r.drift.join("\n  ")}`); process.exit(1); }
      console.log(`nav codegen up to date (${r.wrappers} wrappers, ${r.fields} fields)`);
    } else {
      console.log(`gen-nav: wrote ${r.wrappers} wrappers, ${r.fields} fields`);
    }
  }
}
