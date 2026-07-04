// build.mjs — esbuild driver for @s2script/cli
// Bundles src/cli.ts → dist/cli.js (ESM, node platform).
// esbuild itself is marked external (it has a native component that cannot be bundled).
// adm-zip is marked external too (heavy CJS package; installed as a dependency).

import * as esbuild from "esbuild";
import { mkdirSync, chmodSync } from "fs";

mkdirSync("dist", { recursive: true });

await esbuild.build({
  entryPoints: ["src/cli.ts"],
  bundle: true,
  platform: "node",
  format: "esm",
  outfile: "dist/cli.js",
  external: ["esbuild", "adm-zip", "typescript"],
  target: "node24",
  banner: { js: "#!/usr/bin/env node" },
});

chmodSync("dist/cli.js", 0o755);
console.log("built dist/cli.js");
