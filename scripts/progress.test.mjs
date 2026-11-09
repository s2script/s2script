import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), "progress.mjs");
const run = (root, cmd) =>
  spawnSync(process.execPath, [SCRIPT, cmd], { env: { ...process.env, S2_PROGRESS_ROOT: root }, encoding: "utf8" });

function repo(fragments) {
  const root = mkdtempSync(join(tmpdir(), "progress-"));
  mkdirSync(join(root, "docs/progress"), { recursive: true });
  writeFileSync(join(root, "docs/PROGRESS.md"), "# Log\n\n---\n\n## Current state (full history)\n\n**Old (2026-01-01).** old.\n");
  writeFileSync(join(root, "docs/progress/README.md"), "# not a fragment\n");
  for (const [n, b] of Object.entries(fragments)) writeFileSync(join(root, "docs/progress", n), b);
  return root;
}

test("fold prepends newest first and deletes fragments", () => {
  const root = repo({ "2026-11-08-b.md": "**B.** b\n", "2026-11-09-a.md": "**A.** a\n", "2026-11-08-c.md": "**C.** c" });
  const r = run(root, "fold");
  assert.equal(r.status, 0, r.stderr);
  assert.equal(
    readFileSync(join(root, "docs/PROGRESS.md"), "utf8"),
    "# Log\n\n---\n\n## Current state (full history)\n\n**A.** a\n\n**C.** c\n\n**B.** b\n\n**Old (2026-01-01).** old.\n",
  );
  assert.deepEqual(readdirSync(join(root, "docs/progress")), ["README.md"]);
  assert.equal(run(root, "fold").stdout.trim(), "progress: nothing to fold");
});

test("check accepts good fragments and rejects bad names, empty bodies and headings", () => {
  assert.equal(run(repo({ "2026-11-09-ok.md": "**Ok.** fine" }), "check").status, 0);
  for (const bad of [{ "notes.md": "x" }, { "2026-11-09-Caps.md": "x" }, { "2026-11-09-e.md": " \n" }, { "2026-11-09-h.md": "## Heading\nx" }]) {
    const r = run(repo(bad), "check");
    assert.equal(r.status, 1, JSON.stringify(bad));
    assert.match(r.stderr, /bad fragment/);
  }
});
