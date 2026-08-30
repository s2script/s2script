/**
 * The bomb half of the update: the four new lifecycle callbacks and the new C4 accessors.
 *
 * These need no RE at all. cs_script's `Instance.OnBombPlantStart` / `OnBombPlantAbort` /
 * `OnBombDefuseStart` / `OnBombDefuseAbort` are new bindings over game events CS2 has fired all
 * along, and all four are already in this repo's 272-event catalog:
 *
 *   OnBombPlantStart   -> bomb_beginplant     OnBombPlantAbort   -> bomb_abortplant
 *   OnBombDefuseStart  -> bomb_begindefuse    OnBombDefuseAbort  -> bomb_abortdefuse
 *
 * So the "new" surface here is reachable today; what this module adds is a watcher that proves it
 * on a live server, plus the C4 field reads behind `C4.GetPlantStartTime` / `GetPlantFinishTime`.
 *
 * The deprecations in the patch notes matter for the watcher: `planter` on OnBombPlant and
 * `defuser` on OnBombDefuse are deprecated, so the watcher reads the userid slot instead and does
 * not depend on either field.
 */
import { hook } from "@s2script/sdk/plugin";
import { Entity } from "@s2script/sdk/entity";
import type { EntityRef } from "@s2script/sdk/entity";
import { wrapEntity, Player, CsItem } from "@s2script/cs2";
import type { Pawn } from "@s2script/cs2";

/** The events behind the four new cs_script bomb callbacks, plus the three that already existed. */
export const WATCHED_EVENTS = [
  "bomb_beginplant",
  "bomb_abortplant",
  "bomb_planted",
  "bomb_begindefuse",
  "bomb_abortdefuse",
  "bomb_defused",
  "bomb_exploded",
] as const;

/** Flipped by `sm_bomb_watch`; the handlers stay subscribed and cheap-return when off. */
let watching = false;

export function isWatching(): boolean {
  return watching;
}
export function setWatching(on: boolean): boolean {
  watching = on;
  return watching;
}

/**
 * Subscribe once at load, for every watched event.
 *
 * Subscribing at load rather than on `sm_bomb_watch` is deliberate: `ctx.events.on` is load-scoped
 * and ledgered, so toggling subscriptions per command would churn the ledger for no benefit. The
 * flag gates the LOGGING, not the subscription.
 */
export function install(log: (line: string) => void): void {
  for (const name of WATCHED_EVENTS) {
    hook.on(name, (ev) => {
      if (!watching) return;
      const slot = ev.getPlayerSlot("userid");
      const who = slot >= 0 ? (Player.fromSlot(slot)?.playerName ?? `slot ${slot}`) : "<none>";
      const site = ev.getInt("site");
      log(`[bomb] ${name.padEnd(17)} by ${who}${site ? ` site=${site}` : ""}`);
    });
  }
}

/** Everything readable about the C4 in the world right now. */
export interface BombInfo {
  found: boolean;
  index: number | null;
  /** `m_bStartedArming` — true while a plant is in progress. Backs `C4.GetPlantStartTime`'s premise. */
  startedArming: boolean | null;
  /** `m_flArmedTime` — the absolute time the plant COMPLETES. This is the closest field to
   *  cs_script's `GetPlantFinishTime`; the mapping is inferred from the name, not verified. */
  armedTime: number | null;
  /** `m_bBombPlanted`. */
  bombPlanted: boolean | null;
  /** `m_bIsPlantingViaUse`. */
  isPlantingViaUse: boolean | null;
  /** The pawn holding the bomb, if any — this is `CSPlayerPawn.GetC4` read from the other end. */
  carrierName: string | null;
}

/**
 * Find the C4 and read its fields.
 *
 * NOTE on `GetPlantStartTime`: there is no `m_flPlantStartTime` in the schema. cs_script almost
 * certainly derives it (finish time minus the plant duration) rather than storing it, so this
 * reports the fields that DO exist and lets you check that arithmetic against a live plant instead
 * of asserting a mapping the dump does not support.
 */
export function readBomb(): BombInfo {
  const c4s = Entity.findByClass(CsItem.C4);
  const ref = c4s[0];
  if (!ref) {
    return {
      found: false, index: null, startedArming: null, armedTime: null,
      bombPlanted: null, isPlantingViaUse: null, carrierName: null,
    };
  }
  const c4 = wrapEntity("CC4", ref);
  return {
    found: true,
    index: ref.index,
    startedArming: c4.startedArming,
    armedTime: c4.armedTime,
    bombPlanted: c4.bombPlanted,
    isPlantingViaUse: c4.isPlantingViaUse,
    carrierName: findCarrier(c4s)?.controller?.playerName ?? null,
  };
}

/**
 * The pawn currently holding the C4 — the `CSPlayerPawn.GetC4` direction, via held weapons.
 *
 * The C4 entity list is resolved ONCE and turned into a key set: `findByClass` is an engine-side
 * scan, and calling it per held weapon per slot would run it a few hundred times per command.
 * Identity is (index, id) — the host-minted liveness id, not the engine serial — so a recycled slot
 * cannot masquerade as the bomb.
 */
export function findCarrier(c4s: EntityRef[] = Entity.findByClass(CsItem.C4)): Pawn | null {
  if (c4s.length === 0) return null;
  const keys = new Set(c4s.map(refKey));
  for (let slot = 0; slot < 64; slot++) {
    const pawn = Player.fromSlot(slot)?.pawn;
    if (!pawn?.isValid) continue;
    for (const w of pawn.weapons) {
      if (w.ref.isValid() && keys.has(refKey(w.ref))) return pawn;
    }
  }
  return null;
}

/** Stable identity for an entity ref: slot index plus the host-minted liveness id. */
function refKey(ref: EntityRef): string {
  return `${ref.index}:${ref.id}`;
}

/**
 * Probe `C4.AbortPlant` as an entity input.
 *
 * There is no schema field for "abort" — it is an action, so if it is reachable without RE at all
 * it is reachable as entity-IO. `acceptInput` returns whether the event was QUEUED, not whether the
 * C4 has such an input; an unknown input is dropped later, silently. Confirm with `sm_bomb_info`
 * (a real abort clears `startedArming`), never from the return value alone.
 */
export function abortPlant(): { queued: boolean; reason: string | null } {
  const ref = Entity.findByClass(CsItem.C4)[0];
  if (!ref) return { queued: false, reason: "no weapon_c4 in the world" };
  return { queued: ref.acceptInput("AbortPlant"), reason: null };
}
