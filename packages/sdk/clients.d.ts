/**
 * @s2script/clients — engine-generic client handle + lifecycle events.
 * Resolved at runtime via globalThis.__s2pkg_clients. Import: import { Client, Clients } from "./clients";
 */

/** A connected client, identified by its 0-based slot (CPlayerSlot). Slot-backed; getters read live. */
export declare class Client {
  /** The client's 0-based engine slot (`CPlayerSlot`) — the handle's stable identity for its connection. */
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
  /**
   * Tell this client to run `cmd` in their own console, as if they had typed it
   * (SourceMod `ClientCommand`).
   *
   * Requires a real client: a bot has no console, so this is a no-op on bots — use
   * {@link Client.fakeCommand} for server-side execution. Returns `false` when the command was not
   * dispatched (empty text, bad slot, or the engine interface is unavailable on this build), never
   * a silent no-op.
   *
   * @example
   * client.command("play sounds/ui/beep.vsnd");
   */
  command(cmd: string): boolean;
  /**
   * Have the SERVER process `cmd` as if this client had sent it (SourceMod `FakeClientCommand`).
   *
   * Unlike {@link Client.command} this works on bots, because nothing is sent to a client — the
   * engine dispatches the command itself, attributed to this player's slot. Live-verified:
   * `fakeCommand("say hi")` on slot 0 prints as that player, not as Console.
   *
   * Returns `false` — never a silent no-op — for a bad slot, empty text, an unavailable interface,
   * or a name that is not a registered console **command** (a ConVar such as `mp_friendlyfire` is
   * refused: use `Server.command` for those).
   *
   * Engine commands (`say`, `kill`, …) execute. A command registered by an s2script plugin runs
   * that plugin's JS handler before this returns.
   *
   * @example
   * client.fakeCommand("say hello");   // as though the player typed it
   */
  fakeCommand(cmd: string): boolean;
}
/**
 * Look up connected clients by slot or enumerate them all.
 * @example
 * import { Clients } from "@s2script/sdk/clients";
 * console.log(`onLoad — all()=${Clients.all().length} clients`);
 */
export declare const Clients: {
  /** The client in `slot`, or null if the slot is empty. */
  fromSlot(slot: number): Client | null;
  /** Every currently-connected client (bots included). */
  all(): Client[];
};
