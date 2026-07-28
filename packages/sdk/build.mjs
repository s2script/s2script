// build.mjs — esbuild driver for @s2script/sdk (the s2s CLI)
// Bundles src/cli.ts → dist/cli.js (ESM, node platform).
// esbuild itself is marked external (it has a native component that cannot be bundled).
// fflate (zip read/write) and semver (the workspace sibling-range gate) are small and
// bundle-friendly, so they are devDependencies inlined into dist/cli.js rather than shipped
// as runtime dependencies — the SDK's zero-runtime-dependency posture.

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
