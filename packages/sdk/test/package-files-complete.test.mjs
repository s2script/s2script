import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync, readdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const packagesDir = join(here, "..", "..");

/**
 * Every relative re-export target must actually SHIP.
 *
 * `@s2script/cs2` re-exported `Weapon` from `./weapon`, but `weapon.d.ts` was missing from the
 * package's `files` array. Nothing errored: TypeScript resolves a dangling specifier to `any`, so
 * consumers silently lost every weapon field and the only symptom was code that should not have
 * compiled compiling fine. A downstream plugin ended up hand-writing the interface instead.
 *
 * The shipped set comes from `npm pack --dry-run --json` rather than from reading `files` directly.
 * `files` mixes literal names, globs and directories, and re-deriving npm's packing rules here would
 * mean this test could disagree with the tarball it is supposed to be checking.
 */
function shippedFiles(pkgDir) {
  // --ignore-scripts: `prepack` builds and prints to stdout, which would land in front of the JSON.
  const out = execFileSync("npm", ["pack", "--dry-run", "--json", "--ignore-scripts"], {
    cwd: pkgDir,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  // Slice from the opening bracket anyway: npm is free to prepend notices, and a JSON.parse of the
  // whole buffer would turn any future stdout noise into a confusing syntax error here.
  const start = out.indexOf("[");
  assert.ok(start >= 0, `npm pack --json produced no JSON array for ${pkgDir}:\n${out}`);
  const [meta] = JSON.parse(out.slice(start));
  return { name: meta.name, files: new Set(meta.files.map((f) => f.path)) };
}

function relativeTargets(text) {
  const out = new Set();
  // Covers `export … from`, `export * from` and `import … from` alike.
  for (const m of text.matchAll(/\bfrom\s+["'](\.\/[^"']+)["']/g)) out.add(m[1]);
  return out;
}

const typePackages = readdirSync(packagesDir, { withFileTypes: true })
  .filter((e) => e.isDirectory() && existsSync(join(packagesDir, e.name, "package.json")))
  .map((e) => join(packagesDir, e.name))
  .filter((dir) => {
    const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
    return !pkg.private && Array.isArray(pkg.files);
  });

test("every relative re-export target in a shipped .d.ts also ships", () => {
  assert.ok(typePackages.length > 0, "found no publishable packages — the discovery walk broke");

  for (const dir of typePackages) {
    const { name, files } = shippedFiles(dir);
    for (const file of [...files].filter((f) => f.endsWith(".d.ts"))) {
      for (const spec of relativeTargets(readFileSync(join(dir, file), "utf8"))) {
        // Resolve relative to the re-exporting file, and allow the specifier to omit the extension
        // or spell it `.js` (the TS convention for a d.ts sibling).
        const base = join(dirname(file), spec.replace(/^\.\//, "").replace(/\.js$/, ""));
        const candidates = [`${base}.d.ts`, base, `${base}/index.d.ts`];
        assert.ok(
          candidates.some((c) => files.has(c)),
          `${name}: ${file} re-exports "${spec}" but none of ${candidates.join(" / ")} is in the ` +
            `published tarball — consumers resolve it to \`any\` with no error`,
        );
      }
    }
  }
});

test("a declared exports subpath points at a file that ships", () => {
  for (const dir of typePackages) {
    const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
    if (!pkg.exports) continue;
    const { name, files } = shippedFiles(dir);
    for (const [subpath, entry] of Object.entries(pkg.exports)) {
      const types = typeof entry === "string" ? entry : entry?.types;
      if (!types) continue;
      const rel = types.replace(/^\.\//, "");
      assert.ok(
        files.has(rel),
        `${name}: exports["${subpath}"].types is ${types}, which is not in the published tarball — ` +
          `the subpath resolves in the editor and 404s once published`,
      );
    }
  }
});
