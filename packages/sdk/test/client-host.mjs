// Engine/host fixture for tests of the game package. The real Client boundary is tested in core.
export function installClientHost(ctx) {
  const generations = new Map();
  const current = slot => generations.get(slot) ?? 1;
  class Client {
    constructor(slot) { this.slot = slot; this.token = current(slot); }
    isValid() { return this.token === current(this.slot); }
    get userId() { return this.isValid() ? ctx.__s2_client_userid?.(this.slot) ?? -1 : -1; }
    get steamId() { return this.isValid() ? ctx.__s2_client_steamid?.(this.slot) ?? '0' : '0'; }
    kick(reason) { if (this.isValid()) ctx.__s2_client_kick?.(this.slot, reason); }
  }
  ctx.__s2pkg_clients = { Client, Clients: { fromSlot: slot => new Client(slot), all: () => [] } };
  return { replace: slot => generations.set(slot, current(slot) + 1) };
}
