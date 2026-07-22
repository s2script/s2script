/** @s2script/events — author-time stubs for the injected game-event API. NO runtime code. */

/** A live game-event accessor. Valid ONLY during the synchronous handler — read fields before any `await`;
 *  a stashed GameEvent used later reads defaults. The raw engine event never crosses to JS. */
export declare class GameEvent {
  /** The event's name (e.g. `"player_death"`). */
  readonly name: string;
  /** Read an integer field. `0` if absent or non-integer. */
  getInt(key: string): number;
  /** Read a float field. `0` if absent or non-float. */
  getFloat(key: string): number;
  /** Read a boolean field. `false` if absent. */
  getBool(key: string): boolean;
  /** Read a string field. `""` if absent. */
  getString(key: string): string;
  /** A 64-bit field as a decimal string (SM-parity, wire-safe). */
  getUint64(key: string): string;
  /** A player field (e.g. "userid"/"attacker") as a 0-based slot, or -1 if absent. Resolve with Player.fromSlot. */
  getPlayerSlot(key: string): number;
  /** Overwrite an integer field on the live event (valid only in the synchronous handler). */
  setInt(key: string, value: number): void;
  /** Overwrite a float field on the live event (valid only in the synchronous handler). */
  setFloat(key: string, value: number): void;
  /** Overwrite a boolean field on the live event (valid only in the synchronous handler). */
  setBool(key: string, value: boolean): void;
  /** Overwrite a string field on the live event (valid only in the synchronous handler). */
  setString(key: string, value: string): void;
  /** Set a 64-bit field from a decimal string (SM-parity, wire-safe). */
  setUint64(key: string, value: string): void;
}

/** Collapsed pre-hook result. Return from an `onPre` handler; `Handled`/`Stop` suppress the client
 *  broadcast (server still processes). Returning nothing = `Continue`. */
export declare const HookResult: {
  /** No change — proceed normally. Equivalent to returning nothing. */
  readonly Continue: 0;
  /** Fields were modified in place; proceed with the changes (the client broadcast still happens). */
  readonly Changed: 1;
  /** Suppress the client broadcast (the server still processes the event). */
  readonly Handled: 2;
  /** Suppress the client broadcast AND stop running lower-priority handlers. The collapse takes the MAX. */
  readonly Stop: 3;
};
/** The numeric union an `onPre` handler may return — one of the {@link HookResult} values. */
export type HookResultValue = 0 | 1 | 2 | 3;

/**
 * Fire and target game events from JS.
 * @example
 * import { Events } from "@s2script/sdk/events";
 * // plugins/playercommands/src/plugin.ts:78 — announce a name change
 * Events.fire("player_changename", { userid: p.userId, oldname, newname });
 */
export declare const Events: {
  /** Fire a game event. Returns the engine FireEvent result. */
  fire(name: string, fields?: Record<string, number | string | boolean | bigint>, dontBroadcast?: boolean): boolean;
  /** Fire a game event to ONE client (SourceMod FireToClient). Same field type-inference as `fire`. Returns false on miss. */
  fireToClient(slot: number, name: string, fields?: Record<string, string | number | boolean | bigint>): boolean;
};
