import { buildPlugin } from "./build.ts";
import { runGenSchema } from "./schemagen/gen.ts";
import { runGenEvents } from "./eventgen/gen.ts";
import { runGenNav } from "./navgen/gen.ts";
import { createPlugin } from "./create/create.ts";
import { resolvePackagesDir } from "./packages-resolve.ts";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";

const argv = process.argv.slice(2);
const command = argv[0];

function repoRootFromCli(): string {
  // dist/cli.js → packages/cli → packages → repo  (or src/cli.ts → same)
  return join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
}

function parseFlag(args: string[], name: string): string | undefined {
  const eq = args.find((a) => a.startsWith(`${name}=`));
  if (eq) return eq.slice(name.length + 1);
  const i = args.indexOf(name);
  if (i >= 0 && args[i + 1] && !args[i + 1]!.startsWith("-")) return args[i + 1];
  return undefined;
}

function hasFlag(args: string[], name: string): boolean {
  return args.includes(name);
}

if (command === "gen-schema") {
  const repoRoot = repoRootFromCli();
  const check = argv[1] === "--check";
  const r = runGenSchema(repoRoot, { check });
  if (check) {
    if (r.drift.length) { console.error(`FAIL: generated files out of date — run \`s2script gen-schema\`:\n  ${r.drift.join("\n  ")}`); process.exit(1); }
    console.log(`schema codegen up to date (${r.classes} classes, ${r.fields} fields, ${r.skipped} skipped)`);
  } else {
    console.log(`gen-schema: wrote ${r.classes} classes, ${r.fields} fields (${r.skipped} skipped)`);
  }
} else if (command === "gen-events") {
  const repoRoot = repoRootFromCli();
  const check = argv[1] === "--check";
  const r = runGenEvents(repoRoot, { check });
  if (check) {
    if (r.drift.length) { console.error(`FAIL: generated files out of date — run \`s2script gen-events\`:\n  ${r.drift.join("\n  ")}`); process.exit(1); }
    console.log(`event codegen up to date (${r.events} events)`);
  } else {
    console.log(`gen-events: wrote ${r.events} events`);
  }
} else if (command === "gen-nav") {
  const repoRoot = repoRootFromCli();
  const check = argv[1] === "--check";
  const r = runGenNav(repoRoot, { check });
  if (check) {
    if (r.drift.length) { console.error(`FAIL: generated files out of date — run \`s2script gen-nav\`:\n  ${r.drift.join("\n  ")}`); process.exit(1); }
    console.log(`nav codegen up to date (${r.wrappers} wrappers, ${r.fields} fields)`);
  } else {
    console.log(`gen-nav: wrote ${r.wrappers} wrappers, ${r.fields} fields`);
  }
} else if (command === "create") {
  const args = argv.slice(1);
  const pathArg = args.find((a) => !a.startsWith("-"));
  try {
    const result = await createPlugin({
      path: pathArg,
      name: parseFlag(args, "--name"),
      game: parseFlag(args, "--game") as "cs2" | "none" | undefined,
      template: parseFlag(args, "--template") as "minimal" | undefined,
      install: parseFlag(args, "--install") as "npm" | "pnpm" | "yarn" | "bun" | "none" | undefined,
      noInstall: hasFlag(args, "--no-install"),
      yes: hasFlag(args, "--yes") || hasFlag(args, "-y"),
    });
    console.log(`created ${result.dir}`);
    if (result.installed) console.log(`dependencies installed (${result.packageManager})`);
    else if (!result.skippedInstall) console.log(`next: cd ${result.dir} && npm install && npm run build`);
    else console.log(`next: cd ${result.dir} && npm run build`);
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
} else if (command === "build" && argv[1]) {
  const args = argv.slice(1);
  const dir = args.find((a) => !a.startsWith("-"))!;
  const packagesDirFlag = parseFlag(args, "--packages-dir");
  try {
    const packagesDir = resolvePackagesDir({
      explicit: packagesDirFlag,
      pluginDir: resolve(dir),
      fromCliUrl: import.meta.url,
    });
    console.log(await buildPlugin(dir, packagesDir));
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
} else {
  console.error(
    "Usage:\n" +
      "  s2script create [path] [--game cs2|none] [--name <pkg>] [--template minimal]\n" +
      "                  [--install npm|pnpm|yarn|bun|none] [--no-install] [-y]\n" +
      "  s2script build <dir> [--packages-dir <path>]\n" +
      "  s2script gen-schema [--check]\n" +
      "  s2script gen-events [--check]\n" +
      "  s2script gen-nav [--check]"
  );
  process.exit(1);
}
