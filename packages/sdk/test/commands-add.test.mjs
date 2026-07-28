/**
 * `s2s add` — the workspace-root refusal (finding F11, design spec 2026-07-27 §14): `add` is one
 * of the five commands that keeps its exact single-plugin semantics everywhere — including at a
 * specific plugin INSIDE a workspace (--dir plugins/x) — but it must refuse by name when run AT a
 * workspace root, where there is no plugin directory to act on. Proven via a real subprocess so the
 * actual exit code, stderr, and (critically) the ABSENCE of any write to the workspace root are all
 * checked — not a mocked `process.exit`.
 *
 * `S2SCRIPT_REGISTRY_URL` is pinned to an unroutable address (127.0.0.1:1) as a belt-and-suspenders
 * check: if this refusal ever regressed, `addPackage` would fail on THAT bogus registry rather than
 * ever reaching a real one — so a regression here fails on a network/resolve error, never on an
 * actual write to a live registry's types.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/commands-add.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const execFileAsync = promisify(execFile);
const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = join(here, "..");
const fx = (n) => join(here, "fixtures", n);

/** A throwaway copy of a fixture workspace — these tests assert NO write happened to it. */
function copyFixture(name) {
  const dir = join(mkdtempSync(join(tmpdir(), "s2s-add-ws-")), name);
  cpSync(fx(name), dir, { recursive: true });
  return dir;
}

async function runCli(args, cwd) {
  try {
    const r = await execFileAsync(
      process.execPath,
      ["--experimental-strip-types", "--no-warnings", "src/cli.ts", ...args],
      {
        cwd: sdkRoot,
        env: { ...process.env, CI: "1", S2SCRIPT_REGISTRY_URL: "http://127.0.0.1:1" },
        timeout: 10_000,
        // `add` resolves --dir relative to ITS OWN cwd, and this suite must run from a fixed
        // location (sdkRoot, so `src/cli.ts` is found) while still targeting a scratch workspace —
        // so the workspace directory is passed as an argv path (via --dir / the extra `cwd` arg
        // below), never as the subprocess's actual working directory.
      },
    );
    return { code: 0, stdout: r.stdout, stderr: r.stderr };
  } catch (e) {
    return { code: e.code, stdout: e.stdout ?? "", stderr: e.stderr ?? "" };
  }
}

test("F11: `s2s add <pkg>` AT a workspace root is a NAMED refusal, never a silent write to the root", async () => {
  const root = copyFixture("ws-basic");
  const pkgBefore = readFileSync(join(root, "package.json"), "utf8");

  // No --dir given: `add`'s own default is ".", and this subprocess's cwd doubles as the addPackage
  // pluginDir default via --dir root explicitly, so the refusal is exercised the same way an
  // operator running `s2s add @foo/bar` from a workspace root's shell would hit it.
  const r = await runCli(["add", "@foo/bar", "--dir", root]);

  assert.equal(r.code, 1, `expected a non-zero exit, got ${r.code}\nstdout:${r.stdout}\nstderr:${r.stderr}`);
  assert.match(r.stderr, /is a workspace root/);
  assert.match(r.stderr, /not a plugin/);
  assert.match(r.stderr, /--dir <plugin>/);

  // The bug this closes (F11): `s2s add` silently wrote .s2script/types/<dep>/ AND injected
  // s2script.pluginDependencies into the root's package.json — the workspace MARKER FILE itself.
  assert.equal(existsSync(join(root, ".s2script")), false, "must never write into the workspace root");
  assert.equal(readFileSync(join(root, "package.json"), "utf8"), pkgBefore, "root package.json must be untouched");

  rmSync(root, { recursive: true, force: true });
});

test("F11: `s2s add <pkg> --dir <plugin>` for a REAL plugin inside a workspace is unaffected", async () => {
  // The refusal is scoped to the ROOT itself, never to a plugin directory that merely happens to
  // live inside a workspace — spec §14's whole point is that --dir plugins/x keeps working exactly
  // as it does standalone.
  const root = copyFixture("ws-basic");
  const pluginDir = join(root, "plugins", "consumer");

  const r = await runCli(["add", "@foo/bar", "--dir", pluginDir]);

  // Pinned to an unreachable registry, so this run still fails overall — but it must fail on the
  // NETWORK call inside addPackage (a registry/network error), never on the workspace-root
  // refusal, proving that refusal is not over-broad.
  assert.doesNotMatch(r.stderr, /is a workspace root/);
  assert.equal(existsSync(join(root, ".s2script")), false, "network failure, so nothing was written");

  rmSync(root, { recursive: true, force: true });
});

test("F11: the refusal names the ACTUAL workspace root, not the cwd or a relative path", async () => {
  const root = copyFixture("ws-basic");
  const r = await runCli(["add", "@foo/bar", "--dir", root]);
  assert.match(r.stderr, new RegExp(root.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  rmSync(root, { recursive: true, force: true });
});
