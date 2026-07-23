// build.mjs — esbuild driver for @s2script/sdk (the s2s CLI)
// Bundles src/cli.ts → dist/cli.js (ESM, node platform).
// esbuild itself is marked external (it has a native component that cannot be bundled).
// fflate (zip read/write) is small and bundle-friendly, so it is inlined into dist/cli.js
// rather than shipped as a runtime dependency.

import * as esbuild from "esbuild";
import { mkdirSync, chmodSync } from "fs";

mkdirSync("dist", { recursive: true });

await esbuild.build({
  entryPoints: ["src/cli.ts"],
  bundle: true,
  platform: "node",
  format: "esm",
  outfile: "dist/cli.js",
  external: ["esbuild", "typescript", "eslint", "@s2script/eslint-plugin", "@typescript-eslint/*"],
  target: "node24",
  banner: { js: "#!/usr/bin/env node" },
});

chmodSync("dist/cli.js", 0o755);
console.log("built dist/cli.js");
