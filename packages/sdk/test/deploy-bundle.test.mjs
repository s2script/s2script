/**
 * `s2s deploy` at a bundle root: members carry the bundle claim in their registry manifest (never
 * in the .s2sp), and the bundle itself publishes last as a manifest-only upload — or is refused
 * when its version is already published with different member pins.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/deploy-bundle.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { unzipSync } from "fflate";

import { RegistryClient } from "../src/registry/client.ts";
import { buildWorkspace } from "../src/workspace/build-all.ts";
import { loadWorkspace } from "../src/workspace/workspace.ts";
import { readBundle } from "../src/workspace/bundle.ts";
import {
  builtPluginsFromOutcomes,
  computePlan,
  formatBundlePlan,
  planBundle,
  uploadBundle,
  uploadPlan,
} from "../src/workspace/deploy-all.ts";

const here = dirname(fileURLToPath(import.meta.url));
const packagesDir = join(here, "..", "..");
const fx = (n) => join(here, "fixtures", n);

const jsonResponse = (status, body) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** A fake registry: records every deploy archive, answers meta from `published`. */
function fakeRegistry({ published = {}, failDeploy = () => false } = {}) {
  const posts = [];
  const client = new RegistryClient({
    baseUrl: "https://www.example.com",
    token: "s2s_x",
    fetch: async (url, init) => {
      const u = new URL(String(url));
      if (u.pathname === "/api/v1/meta") {
        const name = u.searchParams.get("name");
        if (!published[name]) return jsonResponse(404, { message: "package not found" });
        return jsonResponse(200, { name, versions: published[name] });
      }
      if (u.pathname === "/api/v1/deploy") {
        const files = unzipSync(new Uint8Array(init.body));
        const manifest = JSON.parse(Buffer.from(files["manifest.json"]).toString("utf8"));
        posts.push({ files: Object.keys(files).sort(), manifest, s2sp: files["plugin.s2sp"] });
        if (failDeploy(manifest)) return jsonResponse(500, { error: "boom" });
        return jsonResponse(200, { name: manifest.id, version: manifest.version, reviewState: "unreviewed" });
      }
      return jsonResponse(404, { message: "unexpected" });
    },
  });
  return { client, posts };
}

async function builtBundleWorkspace() {
  const ws = loadWorkspace(fx("ws-bundle"));
  const build = await buildWorkspace({ workspace: ws, packagesDir });
  assert.deepEqual(build.failed, []);
  const { ordered, built } = builtPluginsFromOutcomes(ws, build.outcomes);
  const plan = await computePlan(ordered, async () => false);
  return { ws, bundle: readBundle(ws), plan, built };
}

test("bundle deploy: members carry the claim in the registry manifest, never in the .s2sp", async () => {
  const { bundle, plan, built } = await builtBundleWorkspace();
  const { client, posts } = fakeRegistry();
  const results = await uploadPlan(plan, built, client, { bundle: bundle.name });

  assert.deepEqual(
    results.map((r) => `${r.plugin.name}:${r.status}`).sort(),
    ["@fixture/jb-core:published", "@fixture/jb-devtools:skipped", "@fixture/jb-perks:published"],
  );
  assert.equal(posts.length, 2);
  for (const p of posts) {
    assert.equal(p.manifest.bundle, "@fixture/jb");
    const inner = JSON.parse(Buffer.from(unzipSync(p.s2sp)["manifest.json"]).toString("utf8"));
    assert.equal(inner.bundle, undefined, "the runtime manifest stays minimal");
  }
});

test("bundle deploy: a never-published bundle plans PUBLISH and uploads manifest.json alone", async () => {
  const { bundle, plan, built } = await builtBundleWorkspace();
  const { client, posts } = fakeRegistry();
  const entry = await planBundle(bundle, client);
  assert.equal(entry.reason, "PUBLISH");
  assert.match(formatBundlePlan(entry), /@fixture\/jb 1\.0\.0 {3}PUBLISH \(bundle of 2 plugins\)/);

  const members = await uploadPlan(plan, built, client, { bundle: bundle.name });
  const r = await uploadBundle(entry, members, client);
  assert.equal(r.status, "published");
  const last = posts.at(-1);
  assert.deepEqual(last.files, ["manifest.json"]);
  assert.deepEqual(last.manifest, {
    id: "@fixture/jb",
    version: "1.0.0",
    kind: "bundle",
    members: {
      "@fixture/jb-core": { version: "1.2.0" },
      "@fixture/jb-perks": { version: "0.3.0", optional: true },
    },
  });
});

test("bundle deploy: the bundle is not uploaded when a member failed", async () => {
  const { bundle, plan, built } = await builtBundleWorkspace();
  const { client, posts } = fakeRegistry({ failDeploy: (m) => m.id === "@fixture/jb-perks" });
  const entry = await planBundle(bundle, client);
  const members = await uploadPlan(plan, built, client, { bundle: bundle.name });
  const r = await uploadBundle(entry, members, client);
  assert.deepEqual([r.status, r.detail], ["skipped", "a member failed to publish"]);
  assert.ok(!posts.some((p) => p.manifest.kind === "bundle"));
});

test("bundle deploy: an already-published version with the same pins is a skip", async () => {
  const { bundle } = await builtBundleWorkspace();
  const { client } = fakeRegistry({
    published: {
      "@fixture/jb": [
        {
          version: "1.0.0",
          members: {
            "@fixture/jb-perks": { version: "0.3.0", optional: true },
            "@fixture/jb-core": { version: "1.2.0" },
          },
        },
      ],
    },
  });
  const entry = await planBundle(bundle, client);
  assert.equal(entry.reason, "skip (already published)");
  assert.deepEqual(await uploadBundle(entry, [], client), {
    bundle,
    status: "skipped",
    detail: "skip (already published)",
  });
});

test("bundle deploy: an already-published version with different pins is refused before upload", async () => {
  const { bundle } = await builtBundleWorkspace();
  const { client } = fakeRegistry({
    published: { "@fixture/jb": [{ version: "1.0.0", members: { "@fixture/jb-core": { version: "1.1.0" } } }] },
  });
  await assert.rejects(planBundle(bundle, client), /already published with different members — bump/);
});

test("bundle deploy: an unreachable registry at plan time degrades to PUBLISH, like the plugin check", async () => {
  const { bundle } = await builtBundleWorkspace();
  const client = new RegistryClient({
    baseUrl: "https://www.example.com",
    fetch: async () => {
      throw new TypeError("fetch failed");
    },
  });
  assert.equal((await planBundle(bundle, client)).reason, "PUBLISH");
});
