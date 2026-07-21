/**
 * @s2script/clients — engine-generic client handle + lifecycle events.
 * Resolved at runtime via globalThis.__s2pkg_clients. Import: import { Client, Clients } from "./clients";
 */
/** A connected client, identified by its 0-based slot (CPlayerSlot). Slot-backed; getters read live. */
export declare class Client {
  readonly slot: number;
  /** True while a client occupies this slot. */
  isValid(): boolean;
  /** Decimal SteamID64; "0" for a bot or an unauthenticated client. */
  readonly steamId: string;
  /** Display name; "" if unavailable. */
  readonly name: string;
  /** Engine user-id; -1 if none. */
  readonly userId: number;
  /** Tracked signon state: 0 = none/disconnected, 2 = connected, 5 = spawned, 6 = full (in-game); -1 if the slot is out of range. */
  readonly signonState: number;
  /** True for a fake client (bot) — derived from steamId === "0". */
  readonly isBot: boolean;
  /** Disconnect this client. */
  kick(reason?: string): void;
  /** Send a chat (SayText2) line to this client. */
  chat(message: string): void;
  /** Print one line to this client's developer console (skipped for bots). */
  print(message: string): void;
  /** This client's IP address (":port" stripped); "" for a bot. */
  readonly ip: string;
  /** Show `reason` (chat + console) once the client is in-game, then kick after `delaySeconds` (default 5). Intended to be called from a Clients.onConnect handler. */
  kickWithReason(reason: string, delaySeconds?: number): void;
  /**
   * Server-side voice mute: while true, this client's OUTGOING voice is silenced for every receiver.
   * Framework state (not an engine field): cleared automatically on disconnect, persists across map
   * changes while connected. If the voice descriptor is degraded (hook/validation failure — named
   * reason in the server log), setting is an inert no-op and reads stay false.
   */
  voiceMuted: boolean;
}
export declare const Clients: {
  /** The client in `slot`, or null if the slot is empty. */
  fromSlot(slot: number): Client | null;
  /** Every currently-connected client. */
  all(): Client[];
};
