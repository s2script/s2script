/** @s2script/cookies — SM-parity client preference cookies. NO runtime code (injected as __s2pkg_cookies). */
import type { Client } from "./clients";

/** SM-parity access level carried as metadata on a {@link Cookie} definition (client cookie-menu visibility/editability). */
export enum CookieAccess {
  /** Visible and editable by the client in the cookie menu. */
  Public,
  /** Visible to the client but not editable. */
  Protected,
  /** Hidden from the client; plugin-only. */
  Private,
}
/** A registered cookie definition — the handle passed to {@link Cookies.get}/{@link Cookies.set}. */
export interface Cookie {
  /** Unique cookie name (the DB key). */
  readonly name: string;
  /** Client visibility/editability, per {@link CookieAccess}. */
  readonly access: CookieAccess;
  /** Value returned by {@link Cookies.get} when the client has no stored value. */
  readonly default: string;
}
/** Options for {@link Cookies.register}. */
export interface CookieOptions {
  /** Human-readable description (shown in a cookie menu). */
  description?: string;
  /** Client visibility/editability. @defaultValue {@link CookieAccess.Public} */
  access?: CookieAccess;
  /** Fallback value when the client has no stored value. @defaultValue `""` */
  default?: string;
}
/** Entry point for registering and reading/writing client preference cookies. */
export declare const Cookies: {
  /**
   * Register (or return an existing) cookie definition. Idempotent per plugin context — the same name
   * returns the already-registered {@link Cookie}.
   * @example
   * import { Cookies } from "@s2script/sdk/cookies";
   * const boots = Cookies.register("demo_boots", { default: "0" });
   * const n = parseInt(Cookies.get(client, boots), 10);
   */
  register(name: string, opts?: CookieOptions): Cookie;
  /** Cache value for this client (a stored "" is a real value), else the cookie's default. Default for bots. */
  get(client: Client, cookie: Cookie): string;
  /** Accept a cache change into the bounded host persistence outbox. Returns false for invalid/stale/bot
   * clients or exhausted capacity, without changing the cache. True remains host-owned until DB ACK;
   * it is not a durability acknowledgement. Check false and report, shed, or retry the change. */
  set(client: Client, cookie: Cookie, value: string): boolean;
  /** Has this client's cookies finished loading from the DB? */
  areCached(client: Client): boolean;
  /** Unix timestamp of the cookie's last write (set or DB load), or 0 if never set. 0 for bots. */
  getTime(client: Client, cookie: Cookie): number;
  /** Accept an online or offline SteamID cookie change under the same bounded admission contract as set.
   * False leaves both cache and outbox unchanged (including empty/"0" identity). Accepted writes
   * survive clientprefs reload and same-process core reinit, but not process exit or library destruction. */
  setAuthId(steamId: string, cookie: Cookie, value: string): boolean;
};
