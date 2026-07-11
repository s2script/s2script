/**
 * @s2script/zones — the zone system's inter-plugin interface, published by the first-party
 * @s2script/zones plugin. Import it like any other @s2script/* module; as a hard dependency it resolves
 * to a producer-backed proxy that throws `InterfaceUnavailable` while the plugin is unloaded (probe with
 * a method call and defer subscribing if it throws — the producer may load after the consumer). NO runtime code.
 */
export interface Vec3 { x: number; y: number; z: number; }
export interface Zone { name: string; min: Vec3; max: Vec3; }
export interface ZoneEvent {
  /** The zone's name. */
  zone: string;
  /** The 0-based player slot. */
  slot: number;
  /** The player's engine user-id (re-resolve via Player.fromUserId if the slot churns). */
  userId: number;
}
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
/** Subscribe to a zone event. `enter`/`leave` fire on boundary crossings; `stay` fires each tick while inside. */
export declare function on(event: "enter" | "leave" | "stay", handler: (p: ZoneEvent) => void): number;
/** Unsubscribe a handler from an event. */
export declare function off(event: string, handler: (...args: unknown[]) => void): void;
