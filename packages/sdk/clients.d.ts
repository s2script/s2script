/**
 * @s2script/clients — engine-generic client handle + lifecycle events.
 * Resolved at runtime via globalThis.__s2pkg_clients. Import: import { Client, Clients } from "./clients";
 */

/**
 * A handle to one connection lifetime. Reusing a slot, even for the same SteamID, never revives it.
 * Stale handles return "0" for steamId, "" for name/ip, -1 for userId/signonState,
 * false for booleans; void actions do nothing and command actions return false.
 * OnClientDisconnect receives a read-only identity snapshot during its synchronous callback;
 * isValid() is already false. Capture identity before awaiting; afterward stale defaults apply.
 */
export declare class Client {
  /** The client's 0-based engine slot (`CPlayerSlot`) — not sufficient by itself to identify a connection. */
  readonly slot: number;
  /** True while this exact connection occupies the slot. */
  isValid(): boolean;
  /** Decimal SteamID64; "0" for a bot or an unauthenticated client. */
  readonly steamId: string;
  /** Display name; "" if unavailable. */
  readonly name: string;
  /** Engine user-id; -1 if none. */
  readonly userId: number;
  /** Tracked signon state: 0 = none/disconnected, 2 = connected, 5 = spawned, 6 = full (in-game); -1 for a stale handle. */
  readonly signonState: number;
  /** True for a fake client (bot) — false for stale handles. */
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
  /**
   * Ask this client for its own value of a cvar (SourceMod `QueryClientConVar`). The client
   * answers over the network, so this resolves on a later frame.
   *
   * Resolves `null` when no query was sent (a bot, a disconnected client, a name that is empty or
   * contains whitespace, or a build where the reply hook is unavailable) or when the client
   * disconnects or has not answered within 15 seconds. `value` is `""` unless `status` is `"ok"`.
   *
   * The value is what the client reports. A modified client can report anything, so treat it as a
   * hint, not proof.
   *
   * @example
   * const reply = await client.queryConVar("cl_interp_ratio");
   * if (reply?.status === "ok") console.log(`${client.name}: ${reply.value}`);
   */
  queryConVar(name: string): Promise<ConVarQueryResult | null>;
  /**
   * Set this client's own copy of a replicated cvar without touching the server's value
   * (SourceMod `SendConVarValue`). The client applies it only to cvars the server is allowed to
   * replicate; the next server-side change to that cvar overwrites it. Booleans send `"1"`/`"0"`.
   *
   * Returns `false` for a bot, a disconnected client or an empty name.
   *
   * @example
   * client.replicateConVar("sv_cheats", true);   // client-side only, e.g. for a practice HUD
   */
  replicateConVar(name: string, value: string | number | boolean): boolean;
  /**
   * One snapshot of this client's network counters, read from the engine's net channel
   * (SourceMod `GetClientLatency`, `GetClientAvgLoss`, `GetClientAvgChoke`, `GetClientAvgData`,
   * `GetClientAvgPackets`, `GetClientTime`, `IsClientTimingOut` and `GetClientDataRate` in one
   * call). Read it again for fresh values; nothing in it updates on its own.
   *
   * Returns `null` for a bot (bots have no net channel), a disconnected client, or a server build
   * whose net-channel layout this build has not been verified against.
   *
   * @example
   * const stats = client.netStats();
   * if (stats) console.log(`${client.name}: ${Math.round(stats.avgLatency * 1000)} ms, ` +
   *   `${(stats.avgLoss.incoming * 100).toFixed(1)}% loss`);
   */
  netStats(): ClientNetStats | null;
}
/** A value measured separately for each direction, as seen from the server. */
export interface NetFlowPair {
  /** Server to client. */
  outgoing: number;
  /** Client to server. */
  incoming: number;
}
/** What {@link Client.netStats} returns. Times are in seconds. */
export interface ClientNetStats {
  /** Average packet latency. */
  avgLatency: number;
  /** The engine's current round-trip estimate: more accurate than `avgLatency`, but jittery. */
  engineLatency: number;
  /** Average packet loss, from 0 to 1. */
  avgLoss: NetFlowPair;
  /** Average choke, from 0 to 1. */
  avgChoke: NetFlowPair;
  /** Average data rate in bytes per second. */
  avgData: NetFlowPair;
  /** Average packets per second. */
  avgPackets: NetFlowPair;
  /** Bytes sent or received since the client connected. */
  totalData: NetFlowPair;
  /** Packets sent or received since the client connected. */
  totalPackets: NetFlowPair;
  /** How long the client has been connected. */
  timeConnected: number;
  /** Time since the last packet from the client arrived. */
  timeSinceLastReceived: number;
  /** Whether the channel is about to time out. */
  isTimingOut: boolean;
  /** The send data rate the channel is allowed, in bytes per second. */
  dataRate: number;
}
/**
 * How a client answered {@link Client.queryConVar}: `"ok"` (value is set), `"notFound"` (no such
 * cvar), `"notValid"` (the name is a command, not a cvar), `"protected"` (the client refused to
 * reveal it), or `"unknown"` for a status this build does not recognise.
 */
export type ConVarQueryStatus = "ok" | "notFound" | "notValid" | "protected" | "unknown";
/** The client's answer to {@link Client.queryConVar}. */
export interface ConVarQueryResult {
  status: ConVarQueryStatus;
  /** The client's value, or `""` when `status` is not `"ok"`. */
  value: string;
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
