import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { cs2AddonBundle } from "./cs2-addon.mjs";

function host({ unavailable, missingField } = {}) {
  const writes = [];
  const owner = { index: 1, id: 10, live: true, isValid() { return this.live; } };
  const camera = { index: 2, id: 20, live: true, mode: 0,
    isValid() { return this.live; }, readHandle() { return owner.live ? owner : null; },
    readUInt8() { return this.mode; } };
  const calls = {
    getCustomCamera: () => camera,
    setCustomCameraMode: (ref, mode) => { writes.push(["mode", ref, mode]); camera.mode = mode; },
    setCustomCameraFollowConfig: (...args) => { writes.push(["follow", ...args]); },
  };
  function Pawn(ref) { this.ref = ref; }
  const ctx = vm.createContext({
    __s2pkg_cs2: { Pawn },
    __s2pkg_cs2_calls: { call: name => name === unavailable ? null : (...args) => calls[name](...args) },
    __s2_schema_offset: (_cls, field) => field === missingField ? -1 : field === "m_hPawn" ? 100 : 104,
  });
  vm.runInContext(readFileSync(new URL("../../../games/cs2/js/camera.js", import.meta.url), "utf8"), ctx);
  return { api: ctx.__s2pkg_cs2, pawn: new Pawn(owner), owner, camera, calls, writes };
}

test("camera implementation is included in the shipped bundle", () => {
  assert.match(cs2AddonBundle, /Pawn\.prototype\.getCustomCamera/);
});

test("pawn camera acquisition, owner lookup and all modes", () => {
  const h = host();
  const camera = h.pawn.getCustomCamera();
  assert.equal(camera.ref, h.camera);
  assert.equal(camera.getPlayer().ref, h.owner);
  for (const mode of [0, 1, 2, 3]) {
    assert.equal(camera.setMode(mode), true);
    assert.equal(camera.getMode(), mode);
  }
  assert.equal(h.api.CustomCameraMode.FOLLOW_POSITION, 3);
  for (const mode of [-1, 4, 1.5, NaN, "1", null]) assert.equal(camera.setMode(mode), false);
  assert.equal(h.writes.length, 4);
});

test("follow config expands defaults and preserves native argument order", () => {
  const h = host();
  const camera = h.pawn.getCustomCamera();
  assert.equal(camera.setFollowConfig({ followEntity: h.owner }), true);
  assert.equal(h.writes[0][1], h.camera);
  assert.equal(h.writes[0][2], h.owner);
  assert.deepEqual(JSON.parse(JSON.stringify(h.writes[0].slice(3))),
    [false, { x: 0, y: 0, z: 0 }, { x: 0, y: 0, z: 0 }, false, 1]);
  const config = { followEntity: h.owner, followEyes: true, followOffset: { x: 1, y: 2, z: 3 },
    cameraOffset: { x: -100, y: 4, z: 5 }, clipCameraOffset: true, cameraOffsetReturnStrength: 0.25 };
  assert.equal(camera.setFollowConfig(config), true);
  assert.deepEqual(JSON.parse(JSON.stringify(h.writes[1].slice(3))),
    [true, config.followOffset, config.cameraOffset, true, 0.25]);
  config.cameraOffset.x = 999;
  assert.equal(h.writes[1][5].x, -100, "native receives a value snapshot");
});

test("invalid follow config never invokes the engine", () => {
  const h = host(); const camera = h.pawn.getCustomCamera();
  for (const config of [null, {}, { followEntity: h.owner, followEyes: "true" },
    { followEntity: h.owner, clipCameraOffset: 1 },
    { followEntity: h.owner, followOffset: { x: NaN, y: 0, z: 0 } },
    { followEntity: h.owner, cameraOffset: { x: 1e100, y: 0, z: 0 } },
    { followEntity: h.owner, cameraOffsetReturnStrength: Infinity }]) {
    assert.equal(camera.setFollowConfig(config), false);
  }
  h.owner.live = false;
  assert.equal(camera.setFollowConfig({ followEntity: h.owner }), false);
  assert.equal(h.writes.length, 0);
});

test("stale camera and pawn, missing descriptors or schema fail closed", () => {
  for (const unavailable of ["getCustomCamera", "setCustomCameraMode", "setCustomCameraFollowConfig"]) {
    assert.equal(host({ unavailable }).pawn.getCustomCamera(), null);
  }
  for (const missingField of ["m_hPawn", "m_nCameraMode"]) {
    assert.equal(host({ missingField }).pawn.getCustomCamera(), null);
  }
  const h = host(); const camera = h.pawn.getCustomCamera();
  h.camera.live = false;
  assert.equal(camera.getMode(), null);
  assert.equal(camera.getPlayer(), null);
  assert.equal(camera.setMode(1), false);
  assert.equal(camera.setFollowConfig({ followEntity: h.owner }), false);
  assert.equal(h.pawn.getCustomCamera(), null);
  h.owner.live = false;
  assert.equal(h.pawn.getCustomCamera(), null);
});

test("rejected native calls and mismatched owners cannot report success", () => {
  const h = host(); const camera = h.pawn.getCustomCamera();
  h.calls.setCustomCameraMode = () => null;
  h.calls.setCustomCameraFollowConfig = () => null;
  assert.equal(camera.setMode(1), false);
  assert.equal(camera.setFollowConfig({ followEntity: h.owner }), false);
  h.camera.readHandle = () => ({ ...h.owner, id: 999 });
  assert.equal(h.pawn.getCustomCamera(), null);
});
