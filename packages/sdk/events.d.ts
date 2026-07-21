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
  setInt(key: string, value: number): void;
  setFloat(key: string, value: number): void;
  setBool(key: string, value: boolean): void;
  setString(key: string, value: string): void;
  /** Set a 64-bit field from a decimal string (SM-parity, wire-safe). */
  setUint64(key: string, value: string): void;
}

/** Collapsed pre-hook result. Return from an `onPre` handler; `Handled`/`Stop` suppress the client
 *  broadcast (server still processes). Returning nothing = `Continue`. */
export declare const HookResult: { readonly Continue: 0; readonly Changed: 1; readonly Handled: 2; readonly Stop: 3 };
export type HookResultValue = 0 | 1 | 2 | 3;

export declare const Events: {
  /** Fire a game event. Returns the engine FireEvent result. */
  fire(name: string, fields?: Record<string, number | string | boolean | bigint>, dontBroadcast?: boolean): boolean;
  /** Fire a game event to ONE client (SourceMod FireToClient). Same field type-inference as `fire`. Returns false on miss. */
  fireToClient(slot: number, name: string, fields?: Record<string, string | number | boolean | bigint>): boolean;
};
