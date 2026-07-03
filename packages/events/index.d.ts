/** @s2script/events — author-time stubs for the injected game-event API. NO runtime code. */

/** A live game-event accessor. Valid ONLY during the synchronous handler — read fields before any `await`;
 *  a stashed GameEvent used later reads defaults. The raw engine event never crosses to JS. */
export declare class GameEvent {
  readonly name: string;
  getInt(key: string): number;
  getFloat(key: string): number;
  getBool(key: string): boolean;
  getString(key: string): string;
  /** A 64-bit field as a decimal string (SM-parity, wire-safe). */
  getUint64(key: string): string;
  /** A player field (e.g. "userid"/"attacker") as a 0-based slot, or -1 if absent. Resolve with Player.fromSlot. */
  getPlayerSlot(key: string): number;
}

export declare const Events: {
  /** Subscribe to a game event by name. The handler runs synchronously when the event fires. */
  on(name: string, handler: (ev: GameEvent) => void): void;
  /**
   * Unsubscribe from a game event by name.
   *
   * **Note:** `off` removes ALL of the calling plugin's handlers for the given event name —
   * handler identity is NOT compared. This matches the engine-op semantics: the mux removes
   * every subscription the current plugin holds for `name` in one call. Avoid double-subscribing
   * the same name with different handlers if you need selective removal.
   */
  off(name: string, handler: (ev: GameEvent) => void): void;
};
