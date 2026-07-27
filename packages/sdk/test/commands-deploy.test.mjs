/**
 * `s2s deploy` — commands/deploy.ts's own CLI-layer argv handling: the --dry-run single-plugin
 * hole (F9) and the --stamp-version positional-swallowing hole (F10). Both are "misparse or
 * silently proceed rather than refuse" bugs (design spec 2026-07-27 §14), so both are proven via a
 * REAL subprocess — the only way to see the actual exit code and the actual stderr an operator
 * would get, with no in-process mocking of `process.exit`.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/commands-deploy.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { dirname, join } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const execFileAsync = promisify(execFile);
const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = join(here, "..");
const fx = (n) => join(here, "fixtures", n);

async function runCli(args) {
  try {
    const r = await execFileAsync(
      process.execPath,
      ["--experimental-strip-types", "--no-warnings", "src/cli.ts", ...args],
      { cwd: sdkRoot, env: { ...process.env, CI: "1" } },
    );
    return { code: 0, stdout: r.stdout, stderr: r.stderr };
  } catch (e) {
    return { code: e.code, stdout: e.stdout, stderr: e.stderr };
  }
}

// ---------------------------------------------------------------------------
// F9 — --dry-run in single-plugin mode must never silently fall through to a REAL upload
// ---------------------------------------------------------------------------

test("F9: `s2s deploy <plugin> --dry-run` is a NAMED refusal, not a silent real deploy attempt", async () => {
  // fixtures/hello is private:true. Before the fix, --dry-run was read but never consulted in
  // single-plugin mode, so this fell straight through into `deployPlugin`, which built the plugin
  // and then hit the §6.3 private gate — a COMPLETELY DIFFERENT error ("is private") than the
  // --dry-run one. A run that had a token and a non-private package would have gone all the way
  // through to a real registry upload. Asserting the NEW message and the ABSENCE of the old one
  // proves the flag is now consulted before anything else happens, not that some other gate
  // happened to catch this particular fixture.
  const r = await runCli(["deploy", fx("hello"), "--dry-run"]);
  assert.equal(r.code, 1, `expected a non-zero exit, got ${r.code}\nstderr: ${r.stderr}`);
  assert.match(r.stderr, /--dry-run only applies at a workspace root/);
  assert.match(r.stderr, /resolves to single-plugin mode/);
  assert.doesNotMatch(
    r.stderr,
    /is private/,
    "must refuse on --dry-run itself, never fall through to deployPlugin's private gate",
  );
});

test("F9: --dry-run's single-plugin refusal reads exactly like --filter's (same named-error shape)", async () => {
  const dryRun = await runCli(["deploy", fx("hello"), "--dry-run"]);
  const filter = await runCli(["deploy", fx("hello"), "--filter", "@demo/hello"]);
  assert.match(dryRun.stderr, /only applies at a workspace root — .* resolves to single-plugin mode/);
  assert.match(filter.stderr, /only applies at a workspace root — .* resolves to single-plugin mode/);
});

// ---------------------------------------------------------------------------
// F10 — --stamp-version must never be swallowed as the positional plugin directory
// ---------------------------------------------------------------------------

test("F10: `s2s deploy --stamp-version 1.5.0` is a NAMED refusal, not an ENOENT on a literal '1.5.0' directory", async () => {
  // Before the fix, --stamp-version was absent from the positionals() value-flag set, so
  // "1.5.0" was left in argv and picked up as the positional `dir` — the command then tried (and
  // failed) to read a package.json out of a directory literally named "1.5.0".
  const r = await runCli(["deploy", "--stamp-version", "1.5.0"]);
  assert.equal(r.code, 1, `expected a non-zero exit, got ${r.code}\nstderr: ${r.stderr}`);
  assert.match(r.stderr, /--stamp-version is a build-only flag/);
  assert.match(r.stderr, /s2s deploy never rewrites versions/);
  assert.doesNotMatch(r.stderr, /1\.5\.0/, "must never treat the version value as a directory");
  assert.doesNotMatch(r.stderr, /ENOENT/, "must refuse before ever touching the filesystem for 'dir'");
});

test("F10: --stamp-version is refused in single-plugin mode too (not just the no-dir case)", async () => {
  const r = await runCli(["deploy", fx("hello"), "--stamp-version", "2.0.0"]);
  assert.equal(r.code, 1);
  assert.match(r.stderr, /--stamp-version is a build-only flag/);
});
