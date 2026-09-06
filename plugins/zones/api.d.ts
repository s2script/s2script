/**
 * @s2script/zones — the zone system's contract, implemented by the first-party zones plugin.
 *
 * Protocol 2. Hard dep: `import { on, getZones } from "@s2script/zones"` or inferred `use`.
 * Optional dep: `watchOptional("@s2script/zones", (zones, scope) => { ... })` attaches after
 * both plugins become Active. Own subscriptions with `scope.own(zones.on(...))`.
 *
 * Methods describe CURRENT state; notifications never replay history. Query getZones() on
 * attachment for the existing layout. The CLI derives named methods and on() from Contract;
 * on() returns a disposable, consumer-ledgered Subscription. NO runtime code.
 */
import type { Notification } from "@s2script/sdk/interfaces";
export interface Vec3 { x: number; y: number; z: number; }
export interface Zone { name: string; min: Vec3; max: Vec3; tags: string[]; }
export interface ZoneEvent {
  /** The zone's name. */
  zone: string;
  /** The 0-based player slot at emission; never retain it as connection identity. */
  slot: number;
  /** Re-resolve with Player.fromUserId when responding; null means the connection ended. */
  userId: number;
}
/** Payload of the `created` event (also fired per zone on a map's DB load; a re-save re-fires it). */
export interface ZoneCreatedEvent { zone: string; min: Vec3; max: Vec3; tags: string[]; }
/** Payload of the `deleted` event (also fired per zone cleared on a map change). */
export interface ZoneDeletedEvent { zone: string; }
/** Create (or replace) a named zone from world-space corners on the current map; persisted. */
export declare function createZone(name: string, min: Vec3, max: Vec3): boolean;
/** Delete a named zone on the current map. */
export declare function deleteZone(name: string): boolean;
/** The current map's zones. */
export declare function getZones(): Zone[];
/** Whether the player at `slot` is currently inside the named zone. */
export declare function isInZone(slot: number, name: string): boolean;
/** The names of every zone the player at `slot` is currently in. */
export declare function zonesFor(slot: number): string[];
/** The current map's zones carrying `tag` (lowercased match). */
export declare function getZonesByTag(tag: string): Zone[];
/** Set/replace a zone's tags (empty array clears). Returns true if the zone exists on the current map. */
export declare function setZoneTags(name: string, tags: string[]): boolean;
/** Method surface retained for type imports. The CLI also checks the actual producer implementation. */
export interface Zones {
  createZone(name: string, min: Vec3, max: Vec3): boolean;
  deleteZone(name: string): boolean;
  getZones(): Zone[];
  isInZone(slot: number, name: string): boolean;
  zonesFor(slot: number): string[];
  getZonesByTag(tag: string): Zone[];
  setZoneTags(name: string, tags: string[]): boolean;
}

export interface Contract {
  methods: Zones;
  forwards: {
    /** Engine boundary crossing into a zone. */
    enter: Notification<ZoneEvent>;
    /** Engine boundary crossing out of a zone; disconnect/map reset do not synthesize leave. */
    leave: Notification<ZoneEvent>;
    /** Every eighth game frame while the same connection remains inside. */
    stay: Notification<ZoneEvent>;
    /** Create, replace, editor/import save, or per-zone map DB load after publication. */
    created: Notification<ZoneCreatedEvent>;
    /** Explicit deletion or each zone cleared on map change. */
    deleted: Notification<ZoneDeletedEvent>;
  };
}
