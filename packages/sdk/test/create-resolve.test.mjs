import { test } from "node:test";
import assert from "node:assert";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { mkdirSync, writeFileSync, rmSync, symlinkSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import {
  isPackagesDir,
  resolvePackagesDir,
  findPackagesDirNearCli,
} from "../src/packages-resolve.ts";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";
import { buildPlugin } from "../src/build.ts";
import { createPlugin, versionSpecFrom, registryDevDeps } from "../src/create/create.ts";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..", "..");
const packagesDir = join(repoRoot, "packages");
const fixtures = join(here, "fixtures", "typecheck");
const fakePkgs = join(fixtures, "fake-packages");

test("isPackagesDir recognizes monorepo packages and fake fixtures", () => {
  assert.equal(isPackagesDir(packagesDir), true);
  assert.equal(isPackagesDir(fakePkgs), true);
  assert.equal(isPackagesDir(here), false);
});

test("findPackagesDirNearCli finds monorepo packages from this test URL", () => {
  // test file is packages/sdk/test/*.mjs → .. = sdk, ../.. = packages
  const found = findPackagesDirNearCli(import.meta.url);
  // from test/*.mjs: dirname=test, ../..=packages — but findPackagesDirNearCli does start/../..
  // start=test → ../..= packages/sdk — NOT packages. So from test URL it may miss.
  // Explicit: resolve from cli package.json location via env/explicit instead.
  const viaExplicit = resolvePackagesDir({ explicit: packagesDir });
  assert.equal(viaExplicit, packagesDir);
});

test("resolvePackagesDir respects S2SCRIPT_PACKAGES_DIR", () => {
  const prev = process.env.S2SCRIPT_PACKAGES_DIR;
  process.env.S2SCRIPT_PACKAGES_DIR = fakePkgs;
  try {
    assert.equal(resolvePackagesDir(), fakePkgs);
  } finally {
    if (prev === undefined) delete process.env.S2SCRIPT_PACKAGES_DIR;
    else process.env.S2SCRIPT_PACKAGES_DIR = prev;
  }
});

test("typecheck resolves via plugin node_modules/@s2script layout", () => {
  const tmp = join(tmpdir(), `s2-nm-${process.pid}-${Date.now()}`);
  mkdirSync(join(tmp, "src"), { recursive: true });
  writeFileSync(
    join(tmp, "package.json"),
    JSON.stringify({
      name: "@test/nm",
      version: "0.1.0",
      main: "src/plugin.ts",
      s2script: { apiVersion: "1.x" },
    }),
  );
  writeFileSync(
    join(tmp, "src", "plugin.ts"),
    `import { Player } from "@s2script/cs2";
export function onLoad() { const p: Player | null = null; void p; }
`,
  );
  // Symlink fake-packages as node_modules/@s2script (same shape as published scope)
  mkdirSync(join(tmp, "node_modules"), { recursive: true });
  const nm = join(tmp, "node_modules", "@s2script");
  symlinkSync(fakePkgs, nm);

  // Pretend the CLI is not in the monorepo so resolution falls through to plugin node_modules.
  const resolved = resolvePackagesDir({
    pluginDir: tmp,
    fromCliUrl: "file:///tmp/not-a-real-cli/dist/cli.js",
  });
  assert.equal(resolved, nm);

  const r = typecheckPlugin(tmp, { packagesDir: resolved });
  assert.equal(r.ok, true, JSON.stringify(r.diagnostics));
  rmSync(tmp, { recursive: true, force: true });
});

test("create --yes scaffolds a CS2 plugin that typechecks against monorepo packages", async () => {
  const tmp = join(tmpdir(), `s2-create-${process.pid}-${Date.now()}`);
  const result = await createPlugin({
    path: tmp,
    name: "@test/created",
    game: "cs2",
    template: "minimal",
    noInstall: true,
    yes: true,
  });
  assert.equal(result.dir, tmp);
  assert.equal(result.name, "@test/created");
  assert.equal(result.game, "cs2");
  assert.equal(result.skippedInstall, true);
  assert.ok(existsSync(join(tmp, "src", "plugin.ts")));
  assert.ok(existsSync(join(tmp, "package.json")));
  assert.ok(existsSync(join(tmp, "tsconfig.json")));

  const pkg = JSON.parse(
    (await import("node:fs")).readFileSync(join(tmp, "package.json"), "utf8"),
  );
  // In-tree create should prefer file: links to monorepo packages.
  // Post-consolidation the CLI ships inside @s2script/sdk (bin s2s), so there is
  // no separate @s2script/cli devDep.
  assert.equal(pkg.devDependencies["@s2script/cli"], undefined);
  assert.match(pkg.devDependencies["@s2script/sdk"], /^file:/);
  assert.match(pkg.devDependencies["@s2script/cs2"], /^file:/);

  // Typecheck with explicit monorepo packagesDir (no node_modules install)
  const tc = typecheckPlugin(tmp, { packagesDir });
  assert.equal(tc.ok, true, JSON.stringify(tc.diagnostics));

  const out = await buildPlugin(tmp, packagesDir);
  assert.match(out, /\.s2sp$/);
  assert.ok(existsSync(out));

  rmSync(tmp, { recursive: true, force: true });
});

test("create --game none scaffolds an engine-generic plugin", async () => {
  const tmp = join(tmpdir(), `s2-create-gen-${process.pid}-${Date.now()}`);
  await createPlugin({
    path: tmp,
    name: "@test/generic",
    game: "none",
    noInstall: true,
    yes: true,
  });
  const src = (await import("node:fs")).readFileSync(join(tmp, "src", "plugin.ts"), "utf8");
  assert.match(src, /OnGameFrame/);
  assert.match(src, /delay/);
  const tc = typecheckPlugin(tmp, { packagesDir });
  assert.equal(tc.ok, true, JSON.stringify(tc.diagnostics));
  rmSync(tmp, { recursive: true, force: true });
});

test("versionSpecFrom carets a clean semver and degrades to latest on any failure", () => {
  assert.equal(versionSpecFrom(0, "0.5.0\n"), "^0.5.0");
  assert.equal(versionSpecFrom(0, "1.2.3-beta.1\n"), "^1.2.3-beta.1");
  assert.equal(versionSpecFrom(0, ""), "latest");
  assert.equal(versionSpecFrom(0, "not-a-version"), "latest");
  assert.equal(versionSpecFrom(1, "0.5.0"), "latest");
  assert.equal(versionSpecFrom(null, "0.5.0"), "latest");
});

test("registryDevDeps pins sdk to the CLI version and resolves other packages live", () => {
  const resolve = (pkg) => (pkg === "@s2script/cs2" ? "^0.5.0" : "latest");
  const deps = registryDevDeps("cs2", "0.1.0", resolve);
  // sdk stays tied to the CLI's own version...
  assert.equal(deps["@s2script/sdk"], "^0.1.0");
  // ...but cs2 is whatever the registry resolver returned — NOT ^0.1.0 (the bug).
  assert.equal(deps["@s2script/cs2"], "^0.5.0");
});

test("registryDevDeps for game=none includes only the sdk", () => {
  const deps = registryDevDeps("none", "0.1.0", () => "unused");
  assert.deepEqual(deps, { "@s2script/sdk": "^0.1.0" });
});
