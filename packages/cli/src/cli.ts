/**
 * CLI entry point for @s2script/cli.
 * Usage: s2script build <dir>
 */

import { buildPlugin } from "./build.ts";

const [command, dir] = process.argv.slice(2);

if (command !== "build" || !dir) {
  console.error("Usage: s2script build <dir>");
  process.exit(1);
}

try {
  const outPath = await buildPlugin(dir);
  console.log(outPath);
} catch (err) {
  console.error(String(err));
  process.exit(1);
}
