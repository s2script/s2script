// Pawn-owned CustomPlayerCamera (CS2 September 9, 2026). Native mutations preserve
// engine camera-service and networking side effects; only getters read live schema.
(function () {
  var cs2 = globalThis.__s2pkg_cs2;
  var bindings = globalThis.__s2pkg_cs2_calls;
  if (!cs2 || !cs2.Pawn || !bindings) return;
  var acquire = bindings.call("getCustomCamera");
  var setMode = bindings.call("setCustomCameraMode");
  var setFollow = bindings.call("setCustomCameraFollowConfig");
  var modes = Object.freeze({ DISABLED: 0, CONTROLLED: 1, CONTROLLED_POSITION: 2, FOLLOW_POSITION: 3 });

  function offset(field) { return __s2_schema_offset("CCSCustomPlayerCamera", field); }
  function live(ref) { return !!ref && typeof ref.isValid === "function" && ref.isValid(); }
  function modeValid(mode) { return typeof mode === "number" && mode >= 0 && mode <= 3 && mode % 1 === 0; }
  function floatValid(value) { return typeof value === "number" && Number.isFinite(value) && Math.abs(value) <= 3.4028234663852886e38; }
  function vector(value) {
    if (value === undefined) return { x: 0, y: 0, z: 0 };
    if (!value) return null;
    var x = value.x, y = value.y, z = value.z;
    return floatValid(x) && floatValid(y) && floatValid(z) ? { x: x, y: y, z: z } : null;
  }

  function CustomPlayerCamera(ref) {
    Object.defineProperty(this, "ref", { value: ref, enumerable: true });
  }
  CustomPlayerCamera.prototype.isValid = function () { return live(this.ref) && this.getPlayer() !== null; };
  CustomPlayerCamera.prototype.getPlayer = function () {
    var off = offset("m_hPawn");
    if (!live(this.ref) || off < 0) return null;
    var ref = this.ref.readHandle(off);
    return live(ref) ? new cs2.Pawn(ref) : null;
  };
  CustomPlayerCamera.prototype.getMode = function () {
    var off = offset("m_nCameraMode");
    if (!this.isValid() || off < 0) return null;
    var mode = this.ref.readUInt8(off);
    return modeValid(mode) ? mode : null;
  };
  CustomPlayerCamera.prototype.setMode = function (mode) {
    if (!modeValid(mode) || !setMode || !this.isValid()) return false;
    return setMode(this.ref, mode) !== null;
  };
  CustomPlayerCamera.prototype.setFollowConfig = function (config) {
    if (!config || !setFollow) return false;
    var entity = config.followEntity;
    var eyes = config.followEyes === undefined ? false : config.followEyes;
    var clip = config.clipCameraOffset === undefined ? false : config.clipCameraOffset;
    var followOffset = vector(config.followOffset);
    var cameraOffset = vector(config.cameraOffset);
    var strength = config.cameraOffsetReturnStrength === undefined ? 1 : config.cameraOffsetReturnStrength;
    if (typeof eyes !== "boolean" || typeof clip !== "boolean" || !followOffset || !cameraOffset ||
        !floatValid(strength) || !live(entity) || !this.isValid()) return false;
    return setFollow(this.ref, entity, eyes, followOffset, cameraOffset, clip, strength) !== null;
  };
  cs2.Pawn.prototype.getCustomCamera = function () {
    if (!live(this.ref) || !acquire || !setMode || !setFollow ||
        offset("m_hPawn") < 0 || offset("m_nCameraMode") < 0) return null;
    var ref = acquire(this.ref);
    if (!live(ref)) return null;
    var camera = new CustomPlayerCamera(ref);
    var owner = camera.getPlayer();
    return owner && owner.ref.index === this.ref.index && owner.ref.id === this.ref.id ? camera : null;
  };
  cs2.CustomPlayerCamera = CustomPlayerCamera;
  cs2.CustomCameraMode = modes;
})();
