// build.mjs — esbuild driver for @s2script/eslint-plugin.
// Bundles src/index.ts → dist/index.js (ESM). The parser/utils/eslint/typescript stay external:
// they must be the SINGLE shared instances the host (editor extension or s2s build) resolves.
import * as esbuild from "esbuild";
import { mkdirSync } from "fs";

mkdirSync("dist", { recursive: true });

await esbuild.build({
  entryPoints: ["src/index.ts"],
  bundle: true,
  platform: "node",
  format: "esm",
  outfile: "dist/index.js",
  external: ["eslint", "typescript", "@typescript-eslint/*"],
  target: "node22",
});

console.log("built dist/index.js");
