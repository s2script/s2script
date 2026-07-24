import { test } from "node:test";
import assert from "node:assert";
import { mkdtempSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  parseManifest,
  mergeSpecs,
  sha256Hex,
  installPlan,
} from "../src/registry/install.ts";

function planEntry(name, version, bytes, reviewState = "reviewed") {
  return {
    name, version, reviewState,
    url: `https://x/api/v1/download/s2sp?name=${name}&version=${version}`,
    sha256: sha256Hex(bytes),
    filename: name.replace(/[/]/g, "_") + ".s2sp",
  };
}

function fakeClient(map) {
  // map: { [name]: { entries: PlanEntry[], bytes: {[name]: Uint8Array} } }
  return {
    async plan(name) {
      const p = map[name];
      if (!p) return { root: null, install: [], skipped: [], warnings: [], errors: [`no ${name}`] };
      return { root: { name, version: p.entries[0].version }, install: p.entries, skipped: [], warnings: p.warnings ?? [], errors: [] };
    },
    async downloadS2sp(name) {
      return Buffer.from(map.__bytes[name]);
    },
  };
}

test("parseManifest accepts a plugins map and rejects junk", () => {
  assert.deepEqual(parseManifest({ plugins: { rtv: "^1.0.0" } }), { plugins: { rtv: "^1.0.0" } });
  assert.throws(() => parseManifest({ plugins: "nope" }));
  assert.throws(() => parseManifest({}));
});

test("mergeSpecs lets args override the manifest", () => {
  const merged = mergeSpecs({ plugins: { rtv: "^1.0.0", a: "^1" } }, ["rtv@^2.0.0", "b"]);
  assert.deepEqual(merged, { rtv: "^2.0.0", a: "^1", b: "*" });
});

test("installPlan writes files, verifies sha, is idempotent", async () => {
  const dir = mkdtempSync(join(tmpdir(), "s2s-inst-"));
  const rtvBytes = new Uint8Array([9, 9, 9]);
  const map = {
    rtv: { entries: [planEntry("rtv", "1.0.0", rtvBytes)] },
    __bytes: { rtv: rtvBytes },
  };
  const client = fakeClient(map);

  const first = await installPlan({ client, specs: { rtv: "*" }, dir });
  assert.deepEqual(first.written, ["rtv.s2sp"]);
  assert.ok(existsSync(join(dir, "rtv.s2sp")));

  const second = await installPlan({ client, specs: { rtv: "*" }, dir });
  assert.deepEqual(second.written, []);
  assert.deepEqual(second.skipped, ["rtv.s2sp"]);
});

test("installPlan aborts on a sha256 mismatch without writing", async () => {
  const dir = mkdtempSync(join(tmpdir(), "s2s-inst-"));
  const entry = planEntry("rtv", "1.0.0", new Uint8Array([1, 1, 1]));
  const map = {
    rtv: { entries: [entry] },
    __bytes: { rtv: new Uint8Array([2, 2, 2]) }, // different bytes -> hash mismatch
  };
  await assert.rejects(
    installPlan({ client: fakeClient(map), specs: { rtv: "*" }, dir }),
    /sha256|integrity|mismatch/i
  );
  assert.ok(!existsSync(join(dir, "rtv.s2sp")));
});

test("installPlan with reviewedOnly refuses an unreviewed plugin", async () => {
  const dir = mkdtempSync(join(tmpdir(), "s2s-inst-"));
  const bytes = new Uint8Array([5]);
  const entry = planEntry("rtv", "1.0.0", bytes, "unreviewed");
  const map = { rtv: { entries: [entry], warnings: ["rtv@1.0.0 is not reviewed"] }, __bytes: { rtv: bytes } };
  await assert.rejects(
    installPlan({ client: fakeClient(map), specs: { rtv: "*" }, dir, reviewedOnly: true }),
    /unreviewed|reviewed-only/i
  );
});

test("installPlan --dry-run writes nothing", async () => {
  const dir = mkdtempSync(join(tmpdir(), "s2s-inst-"));
  const bytes = new Uint8Array([7]);
  const map = { rtv: { entries: [planEntry("rtv", "1.0.0", bytes)] }, __bytes: { rtv: bytes } };
  const res = await installPlan({ client: fakeClient(map), specs: { rtv: "*" }, dir, dryRun: true });
  assert.deepEqual(res.written, []);
  assert.ok(!existsSync(join(dir, "rtv.s2sp")));
});

test("resolveMerged surfaces a cross-root version conflict as an error", async () => {
  // Two roots each pin the same dep to an incompatible exact version.
  const client = {
    async plan(name) {
      if (name === "a") return { root: { name, version: "1.0.0" }, install: [
        { name: "a", version: "1.0.0", url: "u", sha256: null, reviewState: "reviewed", filename: "a.s2sp" },
        { name: "c", version: "1.0.0", url: "u", sha256: null, reviewState: "reviewed", filename: "c.s2sp" }], skipped: [], warnings: [], errors: [] };
      if (name === "b") return { root: { name, version: "1.0.0" }, install: [
        { name: "b", version: "1.0.0", url: "u", sha256: null, reviewState: "reviewed", filename: "b.s2sp" },
        { name: "c", version: "2.0.0", url: "u", sha256: null, reviewState: "reviewed", filename: "c.s2sp" }], skipped: [], warnings: [], errors: [] };
      return { root: null, install: [], skipped: [], warnings: [], errors: ["no"] };
    },
  };
  const { errors } = await (await import("../src/registry/install.ts")).resolveMerged(client, { a: "*", b: "*" });
  assert.ok(errors.some((e) => /conflict/i.test(e)));
});
