/* global exports */
// Simulated native boundary for JS paint tests. Real ownership/unload logic is tested in Rust.
exports.sharedSwitchFixture = function (find, invoke) {
  const leases = new Map();
  function native(owner) {
    return (name, index, id, slot, token, on) => {
      const entity = find(index, id);
      if (!entity) return on ? "stale entity" : null;
      const key = JSON.stringify([name, index, id, slot]);
      const before = leases.get(key) || new Set();
      const after = new Set(before);
      const holder = JSON.stringify([owner, token]);
      if (on) after.add(holder);
      else if (token !== null) after.delete(holder);
      else for (const h of after) if (JSON.parse(h)[0] === owner) after.delete(h);
      if (!!before.size !== !!after.size) {
        const error = invoke(name, entity, slot, !!after.size);
        if (error) return error;
      }
      if (after.size) leases.set(key, after); else leases.delete(key);
      return null;
    };
  }
  return { native, clear() { leases.clear(); } };
};
