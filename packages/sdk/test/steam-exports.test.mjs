// packages/sdk/test/steam-exports.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

test("package.json exports ./steam", () => {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  assert.ok(pkg.exports["./steam"], "expected a ./steam export subpath");
});

test("steam.d.ts declares Steam, Workshop and their constants", () => {
  const dts = readFileSync(join(root, "steam.d.ts"), "utf8");
  for (const name of ["Steam", "Workshop", "WorkshopItemState", "SteamResult"])
    assert.match(dts, new RegExp(`export declare const ${name}: \\{`));
  for (const m of ["download(", "request(", "state(", "isInstalled(", "downloadProgress(", "installInfo(",
                   "suspendDownloads(", "onDownloadResult(", "onItemInstalled(", "onConnected(", "isLoggedOn("])
    assert.ok(dts.includes(m), `expected ${m}`);
});

test("the root barrel re-exports steam", () => {
  assert.match(readFileSync(join(root, "index.d.ts"), "utf8"), /export \* from "\.\/steam";/);
});
