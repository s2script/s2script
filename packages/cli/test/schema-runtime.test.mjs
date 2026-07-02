import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import vm from "node:vm";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const genJs = readFileSync(join(repo, "games/cs2/js/schema.generated.js"), "utf8");
const pawnJs = readFileSync(join(repo, "games/cs2/js/pawn.js"), "utf8");

test("schema.generated.js + pawn.js compose: Pawn.prototype has generated accessors", () => {
  const stdPkg = { EntityRef: function (i, s) { this.index = i; this.serial = s; } };
  stdPkg.EntityRef.prototype.isValid = function () { return true; };
  stdPkg.EntityRef.prototype.readInt32 = function () { return 100; };
  stdPkg.EntityRef.prototype.readFloat32 = function () { return 0.25; };
  stdPkg.EntityRef.prototype.readBool = function () { return false; };
  stdPkg.EntityRef.prototype.readHandle = function () { return new stdPkg.EntityRef(1, 7); };
  const ctx = {
    __s2require: (name) => (name === "@s2script/std" ? stdPkg : null),
    __s2_schema_offset: () => 100,
    __s2_ent_current_serial: () => 7,
    __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);   // concatenation order: schema first, pawn second
  const Pawn = ctx.__s2pkg_cs2.Pawn;
  assert.equal(typeof Object.getOwnPropertyDescriptor(Pawn.prototype, "health").get, "function");
  assert.equal(typeof Object.getOwnPropertyDescriptor(Pawn.prototype, "friction").get, "function");
  const p = new Pawn(new stdPkg.EntityRef(5, 9));
  assert.equal(p.health, 100);
  assert.equal(p.friction, 0.25);
});
