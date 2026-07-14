/**
 * @s2script/zones — the zone system's inter-plugin interface, published by the first-party
 * @s2script/zones plugin. Import it like any other @s2script/* module; as a hard dependency it resolves
 * to a producer-backed proxy that throws `InterfaceUnavailable` while the plugin is unloaded (probe with
 * a method call and defer subscribing if it throws — the producer may load after the consumer). NO runtime code.
 */
export interface Vec3 { x: number; y: number; z: number; }
export interface Zone { name: string; min: Vec3; max: Vec3; tags: string[]; }
export interface ZoneEvent {
  /** The zone's name. */
  zone: string;
  /** The 0-based player slot. */
  slot: number;
  /** The player's engine user-id (re-resolve via Player.fromUserId if the slot churns). */
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
/** Subscribe to a zone event. `enter`/`leave` fire on boundary crossings; `stay` fires each tick while inside. */
export declare function on(event: "enter" | "leave" | "stay", handler: (p: ZoneEvent) => void): number;
/** `created` fires on createZone/sm_zone_add/the editor save, and per zone loaded on a map's DB load. */
export declare function on(event: "created", handler: (p: ZoneCreatedEvent) => void): number;
/** `deleted` fires on deleteZone/sm_zone_delete, and per zone cleared on a map change. */
export declare function on(event: "deleted", handler: (p: ZoneDeletedEvent) => void): number;
/** Unsubscribe a handler from an event. */
export declare function off(event: string, handler: (...args: unknown[]) => void): void;
