/**
 * The `custom_hud_layout` entity itself: create it, find it, tear it down.
 *
 * cs_script's model is that a scripted MAP owns this entity and its layout asset. This module is
 * the server-side equivalent — `createEntity` + `DispatchSpawn` — which is what lets a plugin on a
 * STOCK map find out whether the entity is usable without the map's cooperation.
 *
 * The open question this module exists to answer: `m_strLayout` names a layout asset, and the patch
 * notes describe custom_hud_layouts as "the entry point for scripted maps to provide custom UI". If
 * that asset must be compiled into the map's VPK, then on de_inferno there is nothing to point at —
 * the entity will spawn, its state will be writable, and no client will render anything. Reading
 * back the panel/class/dialog-variable counts after spawn is how we tell those two worlds apart:
 * a layout that loaded registers panel ids; one that did not reports zero.
 */
import { createEntity, Entity } from "@s2script/sdk";
import type { EntityRef } from "@s2script/sdk";

/** The entity class added by the update. */
export const HUD_CLASS = "custom_hud_layout";

/** Targetname stamped on entities this plugin creates, so teardown never touches a map's own. */
export const OWNED_TARGETNAME = "s2_hudlab_layout";

/** Every custom_hud_layout in the world, map-authored ones included. */
export function findAll(): EntityRef[] {
  return Entity.findByClass(HUD_CLASS);
}

/** Only the ones this plugin created (matched on targetname, not creation order). */
export function findOwned(): EntityRef[] {
  return findAll().filter((e) => e.name === OWNED_TARGETNAME);
}

/**
 * The layout entity commands should act on: prefer a map-authored one (it has a real layout asset
 * behind it), fall back to one of ours. Returns null when the world has none.
 */
export function preferred(): EntityRef | null {
  const all = findAll();
  const mapAuthored = all.find((e) => e.name !== OWNED_TARGETNAME);
  return mapAuthored ?? all[0] ?? null;
}

/**
 * OUR entity specifically, ignoring any map-authored one.
 *
 * `preferred()` deliberately favours the map's layout, which is right for driving a scripted map —
 * but it makes testing OUR OWN layout impossible on a map that ships one. On the zoo map every
 * apparent success was actually driving Valve's entity; ours was never the one rendering. Use this
 * when the point is to exercise a layout we supplied.
 */
export function ownEntity(): EntityRef | null {
  return findOwned()[0] ?? null;
}

/** What `create` did, in a form the caller can report verbatim. */
export interface CreateResult {
  ref: EntityRef | null;
  /** The keyvalues actually passed, for the reply — a spawn failure is usually a bad keyvalue. */
  keyvalues: Record<string, string>;
  error: string | null;
}

/**
 * Create + spawn a `custom_hud_layout`.
 *
 * `layout` is the keyvalue name — CONFIRMED on a live client, not inferred: the client rejects a bad
 * value with a message naming the layout field specifically, so the key is definitely parsed.
 *
 * THE EXTENSION MUST BE `.xml`. Not `.vxml`, not `.vxml_c`. The client validates the resource name
 * and rejects the compiled form outright:
 *
 *     Layout xml is an invalid resource name "panorama/layout/custom_game/foo.vxml"
 *
 * `.xml` is accepted and the resource system resolves the compiled `.vxml_c` behind it — the same
 * convention as `models/x.vmdl`. This is also the form cs_script_demo's map stores for its own
 * layout. Passing the compiled name is the single easiest way to get a silently dead panel.
 *
 * Passing an empty `layout` skips the keyvalue entirely, which still exercises spawn + the state
 * writes and is the safer first thing to run on a stock map.
 */
export function create(layout: string): CreateResult {
  const keyvalues: Record<string, string> = {
    targetname: OWNED_TARGETNAME,
    // `origin` MATTERS, despite this being a HUD manager with nothing to draw in the world. A
    // spawned entity with no origin has no position for the engine to network from, and a client
    // that never receives the entity never builds a panel for it — which is exactly the symptom
    // this plugin had: server-side state identical to a map-authored layout (byte-diffed), and
    // nothing on screen. The working reference implementations both set it; we did not.
    origin: "0 0 0",
  };
  if (layout) keyvalues.layout = layout;

  const ref = createEntity(HUD_CLASS, keyvalues);
  if (!ref) {
    return {
      ref: null,
      keyvalues,
      error:
        `createEntity("${HUD_CLASS}") returned null — the class is unknown to this build, or ` +
        `DispatchSpawn rejected the keyvalues (the entity is removed on spawn failure)`,
    };
  }
  return { ref, keyvalues, error: null };
}

/** Remove every layout entity this plugin created. Returns how many went away. */
export function removeOwned(): number {
  let n = 0;
  for (const e of findOwned()) if (e.remove()) n++;
  return n;
}
