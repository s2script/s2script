// Engine/host fixture for tests of the game package. The real Client boundary is tested in core.
export function installClientHost(ctx, connectedSlots = []) {
  const generations = new Map();
  let next = 1;
  const current = slot => generations.get(slot) ?? 0;
  const connect = slot => { if (slot < 0 || slot >= 64) throw new Error('invalid client slot'); generations.set(slot, next++); };
  connectedSlots.forEach(connect);
  class Client {
    constructor(slot) { this.slot = slot; this.token = current(slot); }
    isValid() { return this.token !== 0 && this.token === current(this.slot); }
    get userId() { return this.isValid() ? ctx.__s2_client_userid?.(this.slot) ?? -1 : -1; }
    get steamId() { return this.isValid() ? ctx.__s2_client_steamid?.(this.slot) ?? '0' : '0'; }
    kick(reason) { reason = String(reason); if (this.isValid()) ctx.__s2_client_kick?.(this.slot, reason); }
  }
  ctx.__s2pkg_clients = { Client, _same: (a, b) => !!a && !!b && a.slot === b.slot && a.token === b.token,
    Clients: { fromSlot: slot => current(slot) ? new Client(slot) : null, all: () => [...generations.keys()].map(slot => new Client(slot)) } };
  return { connect, retire: slot => generations.delete(slot), replace: connect };
}
