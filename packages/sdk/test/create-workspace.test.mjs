/**
 * `s2s create --workspace <dir>` and `s2s create <name>` inside a workspace (design spec
 * 2026-07-27 §8): the workspace-root scaffold, the minimal workspace-member scaffold, and the CLI
 * layer that tells `createPlugin` which mode it is in.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/create-workspace.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, existsSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { createWorkspace } from "../src/create/workspace.ts";
import { createPlugin } from "../src/create/create.ts";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";
import { sharedCompilerOptionsJson } from "../src/tsconfig-shared.ts";
import { run as createRun } from "../src/commands/create.ts";

const here = dirname(fileURLToPath(import.meta.url));
const packagesDir = join(here, "..", "..");

function tmp(prefix) {
  return mkdtempSync(join(tmpdir(), prefix));
}

/** Capture console.log lines, restoring the original in `finally`. */
async function withLogs(fn) {
  const lines = [];
  const original = console.log;
  console.log = (...args) => lines.push(args.join(" "));
  try {
    return { value: await fn(), lines };
  } finally {
    console.log = original;
  }
}

// ---------------------------------------------------------------------------
// createWorkspace — the root scaffold
// ---------------------------------------------------------------------------

test("createWorkspace writes package.json with BOTH workspaces and s2script.workspace, plus a devDependencies set", async () => {
  const dir = tmp("s2ws-root-");
  const result = await createWorkspace({ path: dir, name: "@acme/plugins", yes: true });
  assert.equal(result.dir, dir);
  assert.equal(result.name, "@acme/plugins");

  const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  assert.equal(pkg.name, "@acme/plugins");
  assert.equal(pkg.private, true);
  assert.deepEqual(pkg.workspaces, ["plugins/*", "packages/*"]);
  assert.deepEqual(pkg.s2script, { workspace: { plugins: ["plugins/*"] } });
  // In-tree dev (this checkout's own packages/ present) -> file: links, exactly as
  // create.ts's single-plugin scaffold already proves for a standalone plugin.
  assert.match(pkg.devDependencies["@s2script/sdk"], /^file:/);
  assert.match(pkg.devDependencies["@s2script/cs2"], /^file:/);
  assert.match(pkg.devDependencies["@s2script/eslint-plugin"], /^file:/);
  assert.ok(pkg.devDependencies["@changesets/cli"], "s2s version needs this at the workspace root (§7)");

  rmSync(dir, { recursive: true, force: true });
});

test("createWorkspace writes tsconfig.base.json from the SAME shared literal typecheck.ts enforces", async () => {
  const dir = tmp("s2ws-root-");
  await createWorkspace({ path: dir, yes: true });
  const base = JSON.parse(readFileSync(join(dir, "tsconfig.base.json"), "utf8"));
  assert.deepEqual(base.compilerOptions, sharedCompilerOptionsJson);
  rmSync(dir, { recursive: true, force: true });
});

test("createWorkspace writes an eslint.config.mjs, .changeset/config.json, .gitignore, and empty plugins/+packages/", async () => {
  const dir = tmp("s2ws-root-");
  await createWorkspace({ path: dir, yes: true });

  assert.ok(existsSync(join(dir, "eslint.config.mjs")));
  const eslintSrc = readFileSync(join(dir, "eslint.config.mjs"), "utf8");
  assert.match(eslintSrc, /@s2script\/eslint-plugin/);

  const changeset = JSON.parse(readFileSync(join(dir, ".changeset", "config.json"), "utf8"));
  assert.equal(changeset.baseBranch, "main");
  assert.deepEqual(changeset.ignore, []);

  assert.ok(existsSync(join(dir, ".gitignore")));

  assert.ok(existsSync(join(dir, "plugins")));
  assert.deepEqual(readdirSync(join(dir, "plugins")), [], "plugins/ starts empty");
  assert.ok(existsSync(join(dir, "packages")));
  assert.deepEqual(readdirSync(join(dir, "packages")), [], "packages/ starts empty");

  rmSync(dir, { recursive: true, force: true });
});

test("createWorkspace refuses a non-empty target directory (the existing check, applied to the root)", async () => {
  const dir = tmp("s2ws-root-");
  writeFileSync(join(dir, "README.md"), "hi\n");
  await assert.rejects(() => createWorkspace({ path: dir, yes: true }), /target directory is not empty/);
  rmSync(dir, { recursive: true, force: true });
});

test("createWorkspace refuses to nest inside an existing workspace (spec §14: one level only)", async () => {
  const outer = tmp("s2ws-outer-");
  await createWorkspace({ path: outer, yes: true });
  const nested = join(outer, "plugins", "inner-workspace");
  mkdirSync(nested, { recursive: true });
  await assert.rejects(
    () => createWorkspace({ path: nested, yes: true }),
    (e) => {
      assert.match(e.message, /already inside a workspace rooted at/);
      assert.match(e.message, new RegExp(outer.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
      return true;
    },
  );
  rmSync(outer, { recursive: true, force: true });
});

// ---------------------------------------------------------------------------
// createPlugin(workspaceRoot: …) — the minimal member scaffold
// ---------------------------------------------------------------------------

/** A bare workspace root (package.json + marker only) — enough for createPlugin's file-writing
 *  path; createWorkspace's own richer scaffold is exercised separately above. */
function bareWorkspaceRoot() {
  const root = tmp("s2ws-member-");
  writeFileSync(
    join(root, "package.json"),
    JSON.stringify(
      { name: "@acme/plugins", private: true, workspaces: ["plugins/*"], s2script: { workspace: { plugins: ["plugins/*"] } } },
      null,
      2,
    ),
  );
  return root;
}

test("createPlugin(workspaceRoot) writes plugins/<name>/ with a MINIMAL package.json — no devDependencies, no eslint config", async () => {
  const root = bareWorkspaceRoot();
  const result = await createPlugin({ path: "shop", workspaceRoot: root, yes: true, noInstall: true });

  assert.equal(result.dir, join(root, "plugins", "shop"));
  assert.equal(result.workspaceRoot, root);
  assert.equal(result.installed, false);
  assert.equal(result.skippedInstall, true);

  const pkg = JSON.parse(readFileSync(join(result.dir, "package.json"), "utf8"));
  assert.deepEqual(pkg, {
    name: "@plugin/shop",
    version: "0.1.0",
    private: true,
    main: "src/plugin.ts",
    scripts: { build: "s2s build ." },
  });
  assert.equal(pkg.devDependencies, undefined, "devDependencies live at the root, not per-plugin");

  assert.ok(!existsSync(join(result.dir, "eslint.config.mjs")), "the root's config covers every member");

  rmSync(root, { recursive: true, force: true });
});

test("createPlugin(workspaceRoot) writes a tsconfig.json extending the root's tsconfig.base.json", async () => {
  const root = bareWorkspaceRoot();
  const result = await createPlugin({ path: "shop", workspaceRoot: root, yes: true, noInstall: true });
  const tsconfig = JSON.parse(readFileSync(join(result.dir, "tsconfig.json"), "utf8"));
  assert.equal(tsconfig.extends, "../../tsconfig.base.json");
  assert.deepEqual(tsconfig.include, ["src", ".s2script/gamedata.d.ts", "node_modules/@s2script/sdk/globals.d.ts"]);
  rmSync(root, { recursive: true, force: true });
});

test("createPlugin(workspaceRoot) still typechecks against the monorepo packages", async () => {
  const root = bareWorkspaceRoot();
  const result = await createPlugin({
    path: "shop",
    name: "@acme/shop",
    game: "none",
    workspaceRoot: root,
    yes: true,
    noInstall: true,
  });
  const tc = typecheckPlugin(result.dir, { packagesDir });
  assert.equal(tc.ok, true, JSON.stringify(tc.diagnostics));
  rmSync(root, { recursive: true, force: true });
});

test("createPlugin(workspaceRoot) rejects a slug that is a path, not a bare name", async () => {
  const root = bareWorkspaceRoot();
  await assert.rejects(
    () => createPlugin({ path: "nested/shop", workspaceRoot: root, yes: true, noInstall: true }),
    /takes a plugin NAME, not a path/,
  );
  await assert.rejects(
    () => createPlugin({ path: "..", workspaceRoot: root, yes: true, noInstall: true }),
    /takes a plugin NAME, not a path/,
  );
  rmSync(root, { recursive: true, force: true });
});

test("createPlugin(workspaceRoot) still refuses a non-empty target — applied to plugins/<name> (spec §8)", async () => {
  const root = bareWorkspaceRoot();
  mkdirSync(join(root, "plugins", "shop"), { recursive: true });
  writeFileSync(join(root, "plugins", "shop", "leftover.txt"), "x");
  await assert.rejects(
    () => createPlugin({ path: "shop", workspaceRoot: root, yes: true, noInstall: true }),
    /target directory is not empty/,
  );
  rmSync(root, { recursive: true, force: true });
});

test("createPlugin(workspaceRoot) requires a name non-interactively (no cwd fallback to '.')", async () => {
  const root = bareWorkspaceRoot();
  await assert.rejects(
    () => createPlugin({ workspaceRoot: root, yes: true, noInstall: true }),
    /a plugin name is required inside a workspace/,
  );
  rmSync(root, { recursive: true, force: true });
});

test("createPlugin(workspaceRoot) ignores --install: a workspace member has nothing per-plugin to install", async () => {
  const root = bareWorkspaceRoot();
  const result = await createPlugin({ path: "shop", workspaceRoot: root, yes: true, install: "npm" });
  assert.equal(result.installed, false);
  assert.equal(result.skippedInstall, true);
  assert.equal(result.packageManager, "none");
  rmSync(root, { recursive: true, force: true });
});

test("createPlugin WITHOUT workspaceRoot is completely unaffected — the compatibility hinge", async () => {
  const dir = tmp("s2-create-standalone-");
  const result = await createPlugin({ path: dir, name: "@test/standalone", noInstall: true, yes: true });
  assert.equal(result.workspaceRoot, undefined);
  assert.ok(existsSync(join(dir, "eslint.config.mjs")));
  const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  assert.ok(pkg.devDependencies, "standalone still carries its own devDependencies");
  rmSync(dir, { recursive: true, force: true });
});

// ---------------------------------------------------------------------------
// commands/create.ts — the CLI layer picks the mode
// ---------------------------------------------------------------------------

test("`s2s create --workspace <dir>` (CLI) creates a workspace root", async () => {
  const dir = tmp("s2ws-cli-root-");
  const { lines } = await withLogs(() => createRun(["--workspace", dir, "--yes"]));
  assert.ok(existsSync(join(dir, "package.json")));
  assert.match(JSON.parse(readFileSync(join(dir, "package.json"), "utf8")).s2script.workspace.plugins[0], /plugins/);
  assert.ok(lines.some((l) => l.includes(`created workspace ${dir}`)));
  rmSync(dir, { recursive: true, force: true });
});

test("`s2s create <name>` (CLI), run with cwd inside a workspace, writes plugins/<name>/", async () => {
  const root = tmp("s2ws-cli-member-");
  await createWorkspace({ path: root, yes: true });

  const prevCwd = process.cwd();
  process.chdir(root);
  try {
    const { lines } = await withLogs(() => createRun(["shop", "--game", "none", "--yes"]));
    const dir = join(root, "plugins", "shop");
    assert.ok(existsSync(join(dir, "package.json")));
    const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
    assert.equal(pkg.devDependencies, undefined);
    assert.ok(lines.some((l) => l.includes(`created ${dir}`)));
    assert.ok(lines.some((l) => l.includes("npm install at the workspace root")));
  } finally {
    process.chdir(prevCwd);
  }
  rmSync(root, { recursive: true, force: true });
});

test("`s2s create <name>` (CLI) OUTSIDE any workspace is byte-identical to today (compatibility hinge)", async () => {
  const dir = tmp("s2-cli-standalone-");
  const target = join(dir, "plugin");
  const { lines } = await withLogs(() =>
    createRun([target, "--name", "@test/cli-standalone", "--no-install", "--yes"]),
  );
  assert.deepEqual(lines, [`created ${target}`, `next: cd ${target} && npm run build`]);
  rmSync(dir, { recursive: true, force: true });
});
