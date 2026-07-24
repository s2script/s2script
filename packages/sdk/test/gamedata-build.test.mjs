/**
 * TDD test: `s2s build` validates the plugin's gamedata, generates
 * .s2script/gamedata.d.ts BEFORE the typecheck gate, records `permissions` in the
 * derived manifest, and packs gamedata.json into the .s2sp.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/gamedata-build.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, mkdirSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { unzipSync } from "fflate";
import { buildPlugin } from "../src/build.ts";

function scaffold(gamedata, permissions, body) {
  const dir = mkdtempSync(join(tmpdir(), "s2gd-"));
  mkdirSync(join(dir, "src"), { recursive: true });
  mkdirSync(join(dir, "gamedata"), { recursive: true });
  writeFileSync(join(dir, "gamedata", "plugin.gamedata.jsonc"), JSON.stringify(gamedata));
  writeFileSync(join(dir, "package.json"), JSON.stringify({
    name: "@demo/gd", version: "0.1.0", main: "src/plugin.ts",
    s2script: { gamedata: "gamedata/plugin.gamedata.jsonc", ...(permissions ? { permissions } : {}) },
  }));
  writeFileSync(join(dir, "src", "plugin.ts"), body);
  return dir;
}

const GD = {
  signatures: { Ig: { linuxsteamrt64: { module: "libserver.so", pattern: "55 48", resolve: "direct" } } },
  calls: { ignite: { receiver: { kind: "entity" }, target: { kind: "signature", name: "Ig" },
                     args: ["float"], returns: "void" } },
};
const OK_BODY = `import { plugin } from "@s2script/sdk/plugin";
import { Engine } from "@s2script/sdk/unsafe";
export default plugin(() => { const f = Engine.call("ignite"); void f; });`;

test("packs gamedata.json into the .s2sp and records permissions in the manifest", async () => {
  const dir = scaffold(GD, ["engine:calls"], OK_BODY);
  const out = await buildPlugin(dir);
  const zip = unzipSync(readFileSync(out));
  assert.ok(zip["gamedata.json"], "expected a gamedata.json member");
  const manifest = JSON.parse(Buffer.from(zip["manifest.json"]).toString("utf8"));
  assert.deepEqual(manifest.permissions, ["engine:calls"]);
  assert.ok(JSON.parse(Buffer.from(zip["gamedata.json"]).toString("utf8")).calls.ignite);
});

test("writes .s2script/gamedata.d.ts", async () => {
  const dir = scaffold(GD, ["engine:calls"], OK_BODY);
  await buildPlugin(dir);
  assert.ok(existsSync(join(dir, ".s2script", "gamedata.d.ts")));
});

test("a calls section without the permission fails the build", async () => {
  const dir = scaffold(GD, undefined, OK_BODY);
  await assert.rejects(() => buildPlugin(dir), /engine:calls/);
});

test("a wrong arg count fails the typecheck gate", async () => {
  const body = `import { plugin } from "@s2script/sdk/plugin";
import { Engine } from "@s2script/sdk/unsafe";
export default plugin(() => { const f = Engine.call("ignite"); if (f) f(null as never, 1, 2); });`;
  const dir = scaffold(GD, ["engine:calls"], body);
  // The matcher matters: a bare assert.rejects() passes for ANY rejection, including the TS2345
  // "not assignable to 'never'" you get when the augmentation is DEAD. Requiring the arity error
  // (TS2554) is what distinguishes a working gate from a silently empty EngineCalls.
  await assert.rejects(() => buildPlugin(dir), /TS2554|Expected 2 arguments/);
});

test("a plugin with no gamedata key still builds and packs no gamedata.json", async () => {
  const dir = mkdtempSync(join(tmpdir(), "s2gd-"));
  mkdirSync(join(dir, "src"), { recursive: true });
  writeFileSync(join(dir, "package.json"), JSON.stringify({ name: "@demo/plain", version: "0.1.0", main: "src/plugin.ts", s2script: {} }));
  writeFileSync(join(dir, "src", "plugin.ts"), `import { plugin } from "@s2script/sdk/plugin";\nexport default plugin(() => {});`);
  const zip = unzipSync(readFileSync(await buildPlugin(dir)));
  assert.equal(zip["gamedata.json"], undefined);
});

// --- stale-artifact + containment (added after review) ---

test("removing gamedata deletes the stale generated .d.ts", async () => {
  const dir = scaffold(GD, ["engine:calls"], OK_BODY);
  await buildPlugin(dir);
  const gen = join(dir, ".s2script", "gamedata.d.ts");
  assert.ok(existsSync(gen), "precondition: generated after the first build");

  // Drop the gamedata and the code that used it, exactly as an author would.
  const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  delete pkg.s2script.gamedata;
  delete pkg.s2script.permissions;
  writeFileSync(join(dir, "package.json"), JSON.stringify(pkg));
  writeFileSync(join(dir, "src", "plugin.ts"),
    `import { plugin } from "@s2script/sdk/plugin";\nexport default plugin(() => {});`);

  const out = await buildPlugin(dir);
  assert.ok(!existsSync(gen),
    "a stale gamedata.d.ts makes the gate certify Engine.calls the .s2sp cannot make");
  assert.equal(unzipSync(readFileSync(out))["gamedata.json"], undefined);
});

test("a gamedata path escaping the plugin directory fails the build", async () => {
  const dir = scaffold(GD, ["engine:calls"], OK_BODY);
  const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  pkg.s2script.gamedata = "../../../etc/hostname";
  writeFileSync(join(dir, "package.json"), JSON.stringify(pkg));
  await assert.rejects(() => buildPlugin(dir), /escapes the plugin directory/);
});

test("gamedata with trailing and block comments builds (house JSONC style)", async () => {
  const dir = scaffold(GD, ["engine:calls"], OK_BODY);
  const jsonc = `{
  // the signature block
  "signatures": {
    "Ig": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "55 48", /* a real pattern would be longer */
        "resolve": "direct" // match offset == entry
      }
    }
  },
  "calls": {
    "ignite": {
      "receiver": { "kind": "entity" },
      "target": { "kind": "signature", "name": "Ig" },
      "args": ["float"], // lifetime
      "returns": "void"
    }
  }
}`;
  writeFileSync(join(dir, "gamedata", "plugin.gamedata.jsonc"), jsonc);
  const out = await buildPlugin(dir);
  assert.ok(unzipSync(readFileSync(out))["gamedata.json"], "packed despite inline comments");
});
