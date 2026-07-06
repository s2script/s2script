/**
 * @s2script/clients — engine-generic client handle + lifecycle events.
 * Resolved at runtime via globalThis.__s2pkg_clients. Import: import { Client, Clients } from "@s2script/clients";
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
  /** Raw signon state; -1 if none. */
  readonly signonState: number;
  /** True for a fake client (bot) — derived from steamId === "0". */
  readonly isBot: boolean;
  /** Disconnect this client. */
  kick(reason?: string): void;
  /** Send a chat (SayText2) line to this client. */
  chat(message: string): void;
}
export declare const Clients: {
  /** Fires when a client connects (all clients incl. bots; carries name/xuid). May be async. */
  onConnect(handler: (client: Client) => void | Promise<void>): void;
  /** Fires when a client is put in the server (controller/pawn context now exists). May be async. */
  onPutInServer(handler: (client: Client) => void | Promise<void>): void;
  /** Fires when a client goes active (spawned / in-game). May be async. */
  onActive(handler: (client: Client) => void | Promise<void>): void;
  /** Fires when a client is fully connected. May be async. */
  onFullyConnect(handler: (client: Client) => void | Promise<void>): void;
  /** Fires when a client disconnects. Only `.slot` is guaranteed live here — capture identity earlier if needed. */
  onDisconnect(handler: (client: Client) => void): void;
  /** Fires when a client's settings (name/cvars) change. */
  onSettingsChanged(handler: (client: Client) => void): void;
  /** The client in `slot`, or null if the slot is empty. */
  fromSlot(slot: number): Client | null;
  /** Every currently-connected client. */
  all(): Client[];
};
