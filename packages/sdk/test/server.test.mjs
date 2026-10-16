/**
 * A server's plugins.json + plugins-lock.json: s2s server init / install / update against a fake
 * registry, and the lockfile's byte format (shared with core/src/server_update.rs).
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/server.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { strToU8, zipSync } from "fflate";

import { parseLock, serializeLock, integrityFromSha256Hex, sha256Hex, sha256HexFromIntegrity } from "../src/server/lock.ts";
import { findAddonRoot, findServerRoot, readServerManifest } from "../src/server/manifest.ts";
import { serverInit, serverInstall, serverSetEnabled, serverUpdate } from "../src/server/commands.ts";
import { RegistryError } from "../src/registry/client.ts";

const FIXTURES = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "server-lock");

/** A minimal .s2sp: its manifest plus a plugin.js that varies with the version. */
function s2sp(id, version) {
  return Buffer.from(
    zipSync(
      {
        "manifest.json": strToU8(JSON.stringify({ id, version, apiVersion: "1.x" })),
        "plugin.js": strToU8(`exports.v = ${JSON.stringify(version)};`),
      },
      { level: 0, mtime: new Date("2026-01-01T00:00:00Z") },
    ),
  );
}

const fileOf = (name) => `${name.replace(/[@/]/g, "_")}.s2sp`;

/**
 * A fake registry: `published` is name → versions (newest last), `deps` name → dependency names.
 * plan() resolves a range with semver like the real one, including dependencies.
 */
async function fakeRegistry(published, deps = {}) {
  const { maxSatisfying } = await import("semver");
  const calls = { plan: [], download: [] };
  const entry = (name, version) => ({
    name,
    version,
    url: `https://reg.test/api/v1/download/s2sp?name=${encodeURIComponent(name)}&version=${version}`,
    sha256: sha256Hex(s2sp(name, version)),
    reviewState: "reviewed",
    filename: fileOf(name),
  });
  const client = {
    calls,
    async plan(name, range = "*") {
      calls.plan.push(`${name}@${range}`);
      const versions = published[name];
      if (!versions) throw new RegistryError("plan failed (HTTP 404): package not found", 404);
      const v = maxSatisfying(versions, range);
      if (!v) return { root: null, install: [], skipped: [], warnings: [], errors: [`no version of ${name} satisfies ${range}`] };
      const install = [entry(name, v)];
      for (const d of deps[name] ?? []) install.push(entry(d, published[d].at(-1)));
      return { root: { name, version: v }, install, skipped: [], warnings: [], errors: [] };
    },
    async downloadS2sp(name, version) {
      calls.download.push(`${name}@${version}`);
      return s2sp(name, version);
    },
  };
  return client;
}

function serverDir(deps = {}, extra = {}) {
  const root = mkdtempSync(join(tmpdir(), "s2s-server-"));
  mkdirSync(join(root, "plugins"));
  writeFileSync(join(root, "plugins.json"), JSON.stringify({ name: "test-server", dependencies: deps, ...extra }, null, 2));
  return root;
}

const lockOf = (root) => parseLock(readFileSync(join(root, "plugins-lock.json"), "utf8"));
const versionsOf = (root) =>
  Object.fromEntries(Object.values(lockOf(root).plugins).map((p) => [p.name, p.version]));

test("lock serialization is the shared fixture byte for byte", () => {
  const text = readFileSync(join(FIXTURES, "plugins-lock.json"), "utf8");
  assert.equal(serializeLock(parseLock(text)), text);
});

test("integrity is npm's sha256 SRI form, round-tripping the hex", () => {
  const hex = sha256Hex(Buffer.from("x"));
  const sri = integrityFromSha256Hex(hex);
  assert.match(sri, /^sha256-[A-Za-z0-9+/]+=*$/);
  assert.equal(sha256HexFromIntegrity(sri), hex);
  assert.equal(sha256HexFromIntegrity("sha512-AAAA"), null);
});

test("findServerRoot accepts the addon root or the game dir above it", () => {
  const root = serverDir();
  assert.equal(findServerRoot(root), root);
  const game = mkdtempSync(join(tmpdir(), "s2s-game-"));
  mkdirSync(join(game, "addons"));
  renameSync(root, join(game, "addons", "s2script"));
  assert.equal(findServerRoot(game), join(game, "addons", "s2script"));
  assert.equal(findServerRoot(mkdtempSync(join(tmpdir(), "s2s-none-"))), null);
});

test("install resolves plugins.json, downloads, and writes the lock", async () => {
  const reg = await fakeRegistry({ "@edge/rtv": ["1.0.0", "1.1.0", "2.0.0"], "@edge/core": ["1.0.0"] }, { "@edge/rtv": ["@edge/core"] });
  const root = serverDir({ "@edge/rtv": "^1.0.0" });
  const summary = await serverInstall({ root, client: reg });
  assert.equal(summary, "2 installed, 0 up to date, 0 removed");
  assert.deepEqual(versionsOf(root), { "@edge/rtv": "1.1.0", "@edge/core": "1.0.0" });
  assert.deepEqual(readdirSync(join(root, "plugins")).sort(), ["_edge_core.s2sp", "_edge_rtv.s2sp"]);
  assert.deepEqual(lockOf(root).root.dependencies, { "@edge/rtv": "^1.0.0" });
});

test("install with a current lock installs exactly the lock, asking the registry nothing", async () => {
  const reg = await fakeRegistry({ "@edge/rtv": ["1.0.0"] });
  const root = serverDir({ "@edge/rtv": "^1.0.0" });
  await serverInstall({ root, client: reg });
  reg.calls.plan.length = 0;
  reg.calls.download.length = 0;
  // A newer version appears; plain `install` must not move to it.
  const newer = await fakeRegistry({ "@edge/rtv": ["1.0.0", "1.5.0"] });
  const summary = await serverInstall({ root, client: newer });
  assert.equal(summary, "0 installed, 1 up to date, 0 removed");
  assert.deepEqual(newer.calls.plan, []);
  assert.deepEqual(versionsOf(root), { "@edge/rtv": "1.0.0" });
});

test("install <name> adds ^latest to plugins.json, keeping the others at their locked versions", async () => {
  const reg = await fakeRegistry({ "@edge/rtv": ["1.0.0"], "@edge/admin": ["3.2.1"] });
  const root = serverDir({ "@edge/rtv": "^1.0.0" });
  await serverInstall({ root, client: reg });
  const later = await fakeRegistry({ "@edge/rtv": ["1.0.0", "1.9.0"], "@edge/admin": ["3.2.1"] });
  await serverInstall({ root, client: later, specs: ["@edge/admin"] });
  assert.deepEqual(readServerManifest(root).dependencies, { "@edge/admin": "^3.2.1", "@edge/rtv": "^1.0.0" });
  assert.deepEqual(versionsOf(root), { "@edge/rtv": "1.0.0", "@edge/admin": "3.2.1" });
  assert.ok(later.calls.plan.includes("@edge/rtv@1.0.0"), "rtv stays pinned to its locked version");
});

test("update moves within the range, never past it, and prunes what the lock owned", async () => {
  const reg = await fakeRegistry({ "@edge/rtv": ["1.0.0"], "@edge/old": ["1.0.0"] });
  const root = serverDir({ "@edge/rtv": "^1.0.0", "@edge/old": "^1.0.0" });
  await serverInstall({ root, client: reg });
  writeFileSync(join(root, "plugins", "handmade.s2sp"), "operator's own");
  const pj = JSON.parse(readFileSync(join(root, "plugins.json"), "utf8"));
  delete pj.dependencies["@edge/old"];
  writeFileSync(join(root, "plugins.json"), JSON.stringify(pj));
  const later = await fakeRegistry({ "@edge/rtv": ["1.0.0", "1.4.0", "2.0.0"], "@edge/old": ["1.0.0"] });
  const summary = await serverUpdate({ root, client: later });
  assert.equal(summary, "1 installed, 0 up to date, 1 removed");
  assert.deepEqual(versionsOf(root), { "@edge/rtv": "1.4.0" });
  assert.deepEqual(readdirSync(join(root, "plugins")).sort(), ["_edge_rtv.s2sp", "handmade.s2sp"]);
});

test("a plugin changed by hand is left in place, not pruned", async () => {
  const reg = await fakeRegistry({ "@edge/old": ["1.0.0"] });
  const root = serverDir({ "@edge/old": "^1.0.0" });
  await serverInstall({ root, client: reg });
  writeFileSync(join(root, "plugins", "_edge_old.s2sp"), "patched locally");
  writeFileSync(join(root, "plugins.json"), JSON.stringify({ name: "t", dependencies: {} }));
  const logs = [];
  await serverUpdate({ root, client: reg, log: (m) => logs.push(m) });
  assert.ok(existsSync(join(root, "plugins", "_edge_old.s2sp")));
  assert.ok(logs.some((l) => /left plugins\/_edge_old\.s2sp in place/.test(l)));
});

test("a bad download changes nothing on disk", async () => {
  const reg = await fakeRegistry({ "@edge/a": ["1.0.0"], "@edge/b": ["1.0.0"] });
  const root = serverDir({ "@edge/a": "^1.0.0", "@edge/b": "^1.0.0" });
  await serverInstall({ root, client: reg });
  const before = readFileSync(join(root, "plugins-lock.json"), "utf8");
  const later = await fakeRegistry({ "@edge/a": ["1.0.0", "1.1.0"], "@edge/b": ["1.0.0", "1.1.0"] });
  later.downloadS2sp = async (name, version) => (name === "@edge/b" ? Buffer.from("tampered") : s2sp(name, version));
  await assert.rejects(() => serverUpdate({ root, client: later }), /sha256 mismatch for @edge\/b@1\.1\.0/);
  assert.equal(readFileSync(join(root, "plugins-lock.json"), "utf8"), before);
  assert.deepEqual(readdirSync(join(root, "plugins")).sort(), ["_edge_a.s2sp", "_edge_b.s2sp"], "no staged leftovers");
  assert.deepEqual(readFileSync(join(root, "plugins", "_edge_a.s2sp")), s2sp("@edge/a", "1.0.0"));
});

test("reviewedOnly refuses an unreviewed plugin", async () => {
  const reg = await fakeRegistry({ "@edge/rtv": ["1.0.0"] });
  const plan = reg.plan;
  reg.plan = async (...a) => {
    const p = await plan(...a);
    p.install = p.install.map((e) => ({ ...e, reviewState: "unreviewed" }));
    return p;
  };
  const root = serverDir({ "@edge/rtv": "^1.0.0" }, { reviewedOnly: true });
  await assert.rejects(() => serverInstall({ root, client: reg }), /reviewedOnly: refusing unreviewed plugins: @edge\/rtv@1\.0\.0/);
});

test("server init manages published plugins and leaves the rest alone", async () => {
  const root = mkdtempSync(join(tmpdir(), "s2s-init-"));
  mkdirSync(join(root, "plugins"));
  writeFileSync(join(root, "plugins", "_edge_rtv.s2sp"), s2sp("@edge/rtv", "1.2.0"));
  writeFileSync(join(root, "plugins", "custom.s2sp"), s2sp("custom", "0.1.0"));
  writeFileSync(join(root, "plugins", "_edge_local.s2sp"), s2sp("@edge/local", "9.9.9-dev"));
  writeFileSync(join(root, "plugins", "broken.s2sp"), "not a zip");
  const reg = await fakeRegistry({ "@edge/rtv": ["1.2.0"], "@edge/local": ["1.0.0"] });
  const report = await serverInit({ root, client: reg, name: "my-server" });
  assert.deepEqual(report.managed, [{ filename: "_edge_rtv.s2sp", name: "@edge/rtv", version: "1.2.0" }]);
  assert.deepEqual(report.unmanaged.map((u) => u.filename).sort(), ["_edge_local.s2sp", "broken.s2sp", "custom.s2sp"]);
  const pj = JSON.parse(readFileSync(join(root, "plugins.json"), "utf8"));
  assert.deepEqual(pj, { name: "my-server", dependencies: { "@edge/rtv": "^1.2.0" }, autoUpdate: true });
  assert.equal(existsSync(join(root, "package.json")), false, "never a package.json npm would pick up");
  assert.deepEqual(versionsOf(root), { "@edge/rtv": "1.2.0" });
  await assert.rejects(() => serverInit({ root, client: reg }), /already exists — pass --force/);
  // The generated pair is immediately consistent: install changes nothing.
  assert.equal(await serverInstall({ root, client: reg }), "0 installed, 1 up to date, 0 removed");
});

test("plugins.json rejects ranges npm would reject", async () => {
  const root = serverDir({ "@edge/rtv": "not a range" });
  assert.throws(() => readServerManifest(root), /invalid range "not a range"/);
});

// ---------------------------------------------------------------------------
// Registry-response safety (carried over from the retired s2script-plugins.json installer)
// ---------------------------------------------------------------------------

/** Wrap a fake registry so every plan entry for `name` is rewritten by `patch`. */
function tamper(reg, name, patch) {
  const plan = reg.plan;
  reg.plan = async (...a) => {
    const p = await plan(...a);
    p.install = p.install.map((e) => (e.name === name ? { ...e, ...patch } : e));
    return p;
  };
  return reg;
}

test("a path-traversal filename from the registry is refused before anything is written", async () => {
  const reg = tamper(await fakeRegistry({ "@edge/rtv": ["1.0.0"] }), "@edge/rtv", { filename: "../../evil.s2sp" });
  const root = serverDir({ "@edge/rtv": "^1.0.0" });
  await assert.rejects(() => serverInstall({ root, client: reg }), /unsafe filename "\.\.\/\.\.\/evil\.s2sp"/);
  assert.deepEqual(reg.calls.download, []);
  assert.equal(existsSync(join(root, "plugins-lock.json")), false);
});

test("an entry the registry gives no sha256 for is refused", async () => {
  const reg = tamper(await fakeRegistry({ "@edge/rtv": ["1.0.0"] }), "@edge/rtv", { sha256: null });
  const root = serverDir({ "@edge/rtv": "^1.0.0" });
  await assert.rejects(() => serverInstall({ root, client: reg }), /did not provide a sha256/);
});

test("one unresolvable plugin fails the whole install and writes nothing", async () => {
  const reg = await fakeRegistry({ "@edge/rtv": ["1.0.0"] });
  const root = serverDir({ "@edge/rtv": "^1.0.0", "@edge/missing": "^1.0.0" });
  await assert.rejects(() => serverInstall({ root, client: reg }), /package not found/);
  assert.deepEqual(readdirSync(join(root, "plugins")), []);
});

test("two plugins needing different versions of a dependency is a conflict", async () => {
  const reg = await fakeRegistry({ "@edge/a": ["1.0.0"], "@edge/b": ["1.0.0"], "@edge/core": ["1.0.0", "2.0.0"] });
  const plan = reg.plan;
  reg.plan = async (name, range) => {
    const p = await plan(name, range);
    const core = { ...p.install[0], name: "@edge/core", filename: "_edge_core.s2sp" };
    p.install.push({ ...core, version: name === "@edge/a" ? "1.0.0" : "2.0.0" });
    return p;
  };
  const root = serverDir({ "@edge/a": "^1.0.0", "@edge/b": "^1.0.0" });
  await assert.rejects(() => serverInstall({ root, client: reg }), /version conflict for @edge\/core: 1\.0\.0 vs 2\.0\.0/);
});

test("install --with records the bundle's optional members and forwards them to the plan", async () => {
  const reg = await fakeRegistry({ "@edge/jb": ["1.0.0"] });
  const seen = [];
  const plan = reg.plan;
  reg.plan = async (name, range, withMembers) => {
    seen.push([name, withMembers]);
    return plan(name, range);
  };
  const root = serverDir();
  await serverInstall({ root, client: reg, specs: ["@edge/jb"], with: ["@edge/jb-perks"] });
  assert.deepEqual(seen, [["@edge/jb", ["@edge/jb-perks"]]]);
  const m = readServerManifest(root);
  assert.deepEqual(m.with, { "@edge/jb": ["@edge/jb-perks"] });
  assert.deepEqual(m.dependencies, { "@edge/jb": "^1.0.0" });
  assert.deepEqual(lockOf(root).root.with, { "@edge/jb": ["@edge/jb-perks"] });
  await assert.rejects(
    () => serverInstall({ root, client: reg, specs: ["@edge/a", "@edge/b"], with: ["x"] }),
    /--with applies to exactly one bundle/,
  );
});

test("findAddonRoot finds where a first plugins.json belongs", () => {
  const game = mkdtempSync(join(tmpdir(), "s2s-game-"));
  mkdirSync(join(game, "addons", "s2script", "plugins"), { recursive: true });
  assert.equal(findAddonRoot(game), join(game, "addons", "s2script"));
  assert.equal(findAddonRoot(join(game, "addons", "s2script")), join(game, "addons", "s2script"));
  assert.equal(findAddonRoot(mkdtempSync(join(tmpdir(), "s2s-empty-"))), null);
});

// ---------------------------------------------------------------------------
// Disabled plugins: installed into plugins/disabled/, kept current, never loaded
// ---------------------------------------------------------------------------

test("install --disabled puts the plugin in plugins/disabled/ and records it", async () => {
  const reg = await fakeRegistry({ "@edge/rtv": ["1.0.0"] });
  const root = serverDir();
  await serverInstall({ root, client: reg, specs: ["@edge/rtv"], disabled: true });
  assert.deepEqual(readdirSync(join(root, "plugins")), ["disabled"]);
  assert.deepEqual(readdirSync(join(root, "plugins", "disabled")), ["_edge_rtv.s2sp"]);
  assert.deepEqual(readServerManifest(root).disabled, ["@edge/rtv"]);
  const lock = lockOf(root);
  assert.deepEqual(Object.keys(lock.plugins), ["plugins/disabled/_edge_rtv.s2sp"]);
  assert.deepEqual(lock.root.disabled, ["@edge/rtv"]);
  // The pair is consistent: a plain install changes nothing and asks the registry nothing.
  reg.calls.plan.length = 0;
  assert.equal(await serverInstall({ root, client: reg }), "0 installed, 1 up to date, 0 removed");
  assert.deepEqual(reg.calls.plan, []);
});

test("enable and disable move the installed file without downloading it", async () => {
  const reg = await fakeRegistry({ "@edge/rtv": ["1.0.0"] });
  const root = serverDir();
  await serverInstall({ root, client: reg, specs: ["@edge/rtv"], disabled: true });
  reg.calls.download.length = 0;
  const logs = [];
  await serverSetEnabled({ root, client: reg, names: ["@edge/rtv"], enabled: true, log: (m) => logs.push(m) });
  assert.deepEqual(reg.calls.download, [], "no download");
  assert.ok(logs.includes("enabled @edge/rtv@1.0.0"), logs.join("\n"));
  assert.deepEqual(readdirSync(join(root, "plugins")).sort(), ["_edge_rtv.s2sp", "disabled"]);
  assert.deepEqual(readdirSync(join(root, "plugins", "disabled")), []);
  assert.deepEqual(readServerManifest(root).disabled, []);
  assert.equal("disabled" in JSON.parse(readFileSync(join(root, "plugins.json"), "utf8")), false);

  await serverSetEnabled({ root, client: reg, names: ["@edge/rtv"], enabled: false });
  assert.deepEqual(readdirSync(join(root, "plugins", "disabled")), ["_edge_rtv.s2sp"]);
  await assert.rejects(() => serverSetEnabled({ root, client: reg, names: ["@edge/nope"], enabled: true }), /not in plugins\.json/);
});

test("a dependency shared with an enabled plugin stays enabled", async () => {
  const reg = await fakeRegistry(
    { "@edge/a": ["1.0.0"], "@edge/b": ["1.0.0"], "@edge/core": ["1.0.0"] },
    { "@edge/a": ["@edge/core"], "@edge/b": ["@edge/core"] },
  );
  const root = serverDir({ "@edge/a": "^1.0.0", "@edge/b": "^1.0.0" }, { disabled: ["@edge/a"] });
  await serverInstall({ root, client: reg });
  assert.deepEqual(readdirSync(join(root, "plugins")).sort(), ["_edge_b.s2sp", "_edge_core.s2sp", "disabled"]);
  assert.deepEqual(readdirSync(join(root, "plugins", "disabled")), ["_edge_a.s2sp"]);
});

test("update keeps a disabled plugin current, in place", async () => {
  const reg = await fakeRegistry({ "@edge/rtv": ["1.0.0"] });
  const root = serverDir({ "@edge/rtv": "^1.0.0" }, { disabled: ["@edge/rtv"] });
  await serverInstall({ root, client: reg });
  await serverUpdate({ root, client: await fakeRegistry({ "@edge/rtv": ["1.0.0", "1.1.0"] }) });
  assert.deepEqual(readFileSync(join(root, "plugins", "disabled", "_edge_rtv.s2sp")), s2sp("@edge/rtv", "1.1.0"));
  assert.deepEqual(readdirSync(join(root, "plugins")), ["disabled"]);
});

test("plugins.json rejects a disabled name that is not a dependency", () => {
  const root = serverDir({ "@edge/rtv": "^1.0.0" }, { disabled: ["@edge/other"] });
  assert.throws(() => readServerManifest(root), /"disabled" names @edge\/other/);
});
