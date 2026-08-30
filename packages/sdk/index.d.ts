/**
 * `@s2script/sdk` — the engine-generic authoring barrel.
 *
 * Subpath imports (`@s2script/sdk/commands`, `@s2script/sdk/plugin`, …) stay valid.
 * This root re-exports the same names so a plugin can write:
 *
 *   import { command, hook, HookResult, ADMFLAG } from "@s2script/sdk";
 *
 * `Player` / `Pawn` / CS2 schema types are NOT here — they live on `@s2script/cs2`.
 * `@s2script/sdk/unsafe` stays a deliberate subpath (not re-exported).
 *
 * NO runtime code in this file: the engine injects `globalThis.__s2pkg_sdk` at load
 * (`__s2require("@s2script/sdk")`).
 */

export * from "./admin";
export * from "./bans";
export * from "./chat";
export * from "./clients";
export * from "./commands";
export * from "./config";
export * from "./cookies";
export * from "./damage";
export * from "./db";
export * from "./entity";
export * from "./events";
export * from "./http";
export * from "./interfaces";
export * from "./math";
export * from "./menu";
export * from "./net";
export * from "./phrases";
export * from "./plugin";
export * from "./plugins";
export * from "./server";
export * from "./sound";
export * from "./timers";
export * from "./topmenu";
export * from "./trace";
export * from "./translations";
export * from "./transmit";
export * from "./usercmd";
export * from "./usermessages";
export * from "./voice";
export * from "./votes";
export * from "./ws";
