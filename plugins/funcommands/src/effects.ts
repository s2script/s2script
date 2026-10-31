import { after, Clients, every, Vector } from "@s2script/sdk";
import type { Client, EntityRef, Timer } from "@s2script/sdk";
import { Beam, Fade, movement, Player } from "@s2script/cs2";
import type { BeamHandle, Pawn } from "@s2script/cs2";

interface Owner {
  slot: number;
  client: Client;
  controller: EntityRef;
  pawn: EntityRef;
}
interface Speed extends Owner { factor: number; original: number }
interface Drug extends Owner { roll: number; remaining: number; expiry?: Timer }
interface Beacon extends Owner { beams: BeamHandle[]; phase: number }
interface Freeze extends Owner { expiry?: Timer }

function same(a: EntityRef, b: EntityRef): boolean { return a.index === b.index && a.id === b.id; }
function owner(p: Player, pw: Pawn): Owner | null {
  const client = Clients.fromSlot(p.slot);
  return client?.isValid() && pw.isValid && pw.lifeState === 0
    ? { slot: p.slot, client, controller: p.ref, pawn: pw.ref } : null;
}
function pawnOf(o: Owner): Pawn | null {
  if (!o.client.isValid()) return null;
  const p = Player.fromSlot(o.slot);
  if (!p || !same(p.ref, o.controller)) return null;
  const pw = p.pawn;
  return pw?.isValid && same(pw.ref, o.pawn) ? pw : null;
}
function live(o: Owner): Pawn | null {
  const pw = pawnOf(o);
  return pw?.lifeState === 0 ? pw : null;
}
function clearFade(o: Owner): boolean {
  // FFADE_PURGE (0x10), transparent and zero duration. No STAYOUT bit: do not retain the blind.
  return o.client.isValid() && pawnOf(o) !== null
    && Fade.to(o.slot, { duration: 0, holdTime: 0, color: 0, flags: 0x10 });
}

/** Own the timers and world-owned beams explicitly; the timer ledger alone cannot delete a beam. */
export class FunEffects {
  private readonly speeds = new Map<number, Speed>();
  private readonly drugs = new Map<number, Drug>();
  private readonly beacons = new Map<number, Beacon>();
  private readonly freezes = new Map<number, Freeze>();
  private ticker?: Timer;
  private clearing = false;
  private ended = false;

  start(): void {
    movement.onProcessMove(view => {
      const p = view.player;
      if (!p) return;
      const state = this.speeds.get(p.slot);
      if (!state) return;
      const pw = live(state);
      if (!pw) { this.stopSpeed(state); return; }
      // m_flVelocityModifier is the engine's dimensionless movement multiplier. Reassert at the
      // movement boundary because CS2 recovers it towards 1. Never multiply the current velocity
      // every tick (exponential acceleration), or also multiply a derived maxspeed (double scaling).
      // Actual speed/weapon switching and >1 multipliers still require live CS2 acceptance.
      if (pw.velocityModifier === null) { this.stopSpeed(state); return; }
      pw.velocityModifier = state.factor;
    });
  }

  setSpeed(p: Player, pw: Pawn, factor: number): boolean {
    if (this.clearing || this.ended) return false;
    const o = owner(p, pw), original = pw.velocityModifier;
    if (!o || original === null || !Number.isFinite(original)) return false;
    let previous = this.speeds.get(o.slot);
    if (previous && !live(previous)) { this.stopSpeed(previous); previous = undefined; }
    pw.velocityModifier = factor;
    const applied = pw.velocityModifier;
    if (!live(o) || applied === null || !Number.isFinite(applied) || Math.abs(applied - factor) > .00001) return false;
    if (factor === 1) this.speeds.delete(o.slot);
    else this.speeds.set(o.slot, { ...o, factor, original: previous?.original ?? original });
    return true;
  }

  private stopSpeed(state: Speed): void {
    if (this.speeds.get(state.slot) !== state) return;
    this.speeds.delete(state.slot);
    const pw = pawnOf(state);
    if (pw && pw.velocityModifier !== null) pw.velocityModifier = state.original;
  }

  drug(p: Player, pw: Pawn, seconds: number): boolean {
    if (this.clearing || this.ended) return false;
    const o = owner(p, pw), angles = pw.eyeAngles;
    if (!o || !angles || !Number.isFinite(angles.z)) return false;
    const previous = this.drugs.get(o.slot);
    const roll = previous && live(previous) ? previous.roll : angles.z;
    if (previous) this.stopDrug(previous);
    if (this.clearing || this.ended || !live(o)) return false;
    const state: Drug = { ...o, roll, remaining: 0 };
    this.drugs.set(o.slot, state);
    try {
      if (seconds > 0) state.expiry = after(seconds * 1000, () => this.stopDrug(state));
      this.ensureTicker();
      this.pulseDrug(state);
      return this.drugs.get(o.slot) === state && live(state) !== null;
    } catch {
      this.stopDrug(state);
      return false;
    }
  }

  undrug(p: Player, pw: Pawn): boolean {
    const state = this.drugs.get(p.slot);
    return !!state && same(pw.ref, state.pawn) && this.stopDrug(state);
  }

  private stopDrug(state: Drug): boolean {
    if (this.drugs.get(state.slot) !== state) return false;
    this.drugs.delete(state.slot); // Detach before native calls, which may dispatch lifecycle events.
    state.expiry?.kill();
    const pw = pawnOf(state), angles = pw?.eyeAngles;
    let restored = false;
    if (pw && angles && Number.isFinite(angles.x) && Number.isFinite(angles.y))
      restored = pw.ref.teleport(null, [angles.x, angles.y, state.roll]);
    const cleared = clearFade(state);
    this.stopIdleTicker();
    return restored && cleared;
  }

  private pulseDrug(state: Drug): void {
    const pw = live(state);
    if (!pw) { this.stopDrug(state); return; }
    if (state.remaining > 0) { state.remaining--; return; }
    state.remaining = 3; // One fade/roll per second; the shared ticker is 250 ms.
    const channel = () => Math.floor(Math.random() * 256);
    const color = ((128 << 24) | (channel() << 16) | (channel() << 8) | channel()) >>> 0;
    if (!Fade.to(state.slot, { duration: 512, holdTime: 100, color, flags: 2 })) { this.stopDrug(state); return; }
    // Fade/teleport may synchronously dispatch another plugin. Re-resolve before every next write.
    if (this.drugs.get(state.slot) !== state) return;
    const current = live(state), angles = current?.eyeAngles;
    if (!current || !angles || !Number.isFinite(angles.x) || !Number.isFinite(angles.y)
      || !current.ref.teleport(null, [angles.x, angles.y, Math.floor(Math.random() * 61) - 30])) this.stopDrug(state);
  }

  beacon(p: Player, pw: Pawn): boolean {
    if (this.clearing || this.ended) return false;
    const previous = this.beacons.get(p.slot);
    if (previous) {
      const wasLive = live(previous) !== null;
      this.stopBeacon(previous);
      if (wasLive) return true;
    }
    const o = owner(p, pw);
    if (!o || !pw.origin) return false;
    const state: Beacon = { ...o, beams: [], phase: 0 };
    this.beacons.set(o.slot, state);
    try {
      const points = this.ring(pw, 16);
      if (!points) { this.stopBeacon(state); return false; }
      for (let i = 0; i < 16; i++) {
        const beam = Beam.draw(points[i]!, points[(i + 1) % 16]!, { color: [80, 180, 255, 255], width: 2 });
        if (!beam) { this.stopBeacon(state); return false; }
        // A lifecycle callback inside create/spawn may have removed this state before draw returns.
        if (this.beacons.get(o.slot) !== state || !live(state)) { beam.remove(); this.stopBeacon(state); return false; }
        state.beams.push(beam);
      }
      this.ensureTicker();
      return true;
    } catch {
      this.stopBeacon(state);
      return false;
    }
  }

  private ring(pw: Pawn, radius: number): Vector[] | null {
    const origin = pw.origin;
    if (!origin || ![origin.x, origin.y, origin.z].every(Number.isFinite)) return null;
    return Array.from({ length: 16 }, (_, i) => {
      const angle = i * Math.PI / 8;
      return new Vector(origin.x + Math.cos(angle) * radius, origin.y + Math.sin(angle) * radius, origin.z + 8);
    });
  }

  private stopBeacon(state: Beacon): void {
    if (this.beacons.get(state.slot) !== state) return;
    this.beacons.delete(state.slot);
    const beams = state.beams.splice(0);
    for (const beam of beams) beam.remove();
    this.stopIdleTicker();
  }

  private pulseBeacon(state: Beacon): void {
    const pw = live(state);
    if (!pw) { this.stopBeacon(state); return; }
    state.phase = (state.phase + 1) % 8;
    const points = this.ring(pw, 16 + state.phase * 16);
    if (!points) { this.stopBeacon(state); return; }
    for (let i = 0; i < state.beams.length; i++) {
      if (this.beacons.get(state.slot) !== state || !live(state)) { this.stopBeacon(state); return; }
      state.beams[i]!.update(points[i]!, points[(i + 1) % 16]!);
    }
  }

  freeze(p: Player, pw: Pawn, seconds: number): boolean {
    if (this.clearing || this.ended) return false;
    const o = owner(p, pw);
    if (!o || pw.moveType === null) return false;
    const previous = this.freezes.get(o.slot);
    if (previous) this.stopFreeze(previous);
    const state: Freeze = { ...o };
    this.freezes.set(o.slot, state);
    try {
      if (seconds > 0) state.expiry = after(seconds * 1000, () => this.stopFreeze(state));
      if (this.freezes.get(o.slot) !== state || !live(state)) { this.stopFreeze(state); return false; }
      pw.moveType = 0;
      if (pw.moveType !== 0) { this.stopFreeze(state); return false; }
      return true;
    } catch {
      this.stopFreeze(state);
      return false;
    }
  }

  unfreeze(p: Player, pw: Pawn): void {
    const state = this.freezes.get(p.slot);
    if (state) this.stopFreeze(state);
    pw.moveType = 2;
  }

  private stopFreeze(state: Freeze): void {
    if (this.freezes.get(state.slot) !== state) return;
    this.freezes.delete(state.slot);
    state.expiry?.kill();
    const pw = pawnOf(state);
    if (pw?.moveType === 0) pw.moveType = 2;
  }

  private ensureTicker(): void {
    if (this.ticker?.alive) return;
    this.ticker = every(250, () => {
      for (const state of [...this.drugs.values()]) this.pulseDrug(state);
      for (const state of [...this.beacons.values()]) this.pulseBeacon(state);
      this.stopIdleTicker();
    });
  }

  private stopIdleTicker(): void {
    if (this.drugs.size || this.beacons.size) return;
    const timer = this.ticker;
    this.ticker = undefined;
    timer?.kill();
  }

  frame(): void {
    for (const state of [...this.speeds.values()]) if (!live(state)) this.stopSpeed(state);
    for (const state of [...this.drugs.values()]) if (!live(state)) this.stopDrug(state);
    for (const state of [...this.beacons.values()]) if (!live(state)) this.stopBeacon(state);
    for (const state of [...this.freezes.values()]) if (!live(state)) this.stopFreeze(state);
  }

  disconnect(client: Client): void {
    // A late disconnect for the previous occupant must not remove the next occupant's effects.
    for (const state of [this.speeds.get(client.slot), this.drugs.get(client.slot), this.beacons.get(client.slot), this.freezes.get(client.slot)])
      if (state && !state.client.isValid()) this.removeSlot(state.slot);
  }

  removeSlot(slot: number): void {
    const speed = this.speeds.get(slot), drug = this.drugs.get(slot), beacon = this.beacons.get(slot), freeze = this.freezes.get(slot);
    if (speed) this.stopSpeed(speed);
    if (drug) this.stopDrug(drug);
    if (beacon) this.stopBeacon(beacon);
    if (freeze) this.stopFreeze(freeze);
  }

  clear(end = false): void {
    if (end) this.ended = true;
    if (this.clearing) return;
    this.clearing = true;
    try {
      for (const slot of new Set([...this.speeds.keys(), ...this.drugs.keys(), ...this.beacons.keys(), ...this.freezes.keys()])) this.removeSlot(slot);
    } finally {
      this.stopIdleTicker();
      this.clearing = false;
    }
  }
}
