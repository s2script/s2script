/** @s2script/cookies — SM-parity client preference cookies. NO runtime code (injected as __s2pkg_cookies). */
import type { Client } from "@s2script/clients";
export enum CookieAccess { Public, Protected, Private }
export interface Cookie { readonly name: string; readonly access: CookieAccess; readonly default: string; }
export interface CookieOptions { description?: string; access?: CookieAccess; default?: string; }
export declare const Cookies: {
  /** Register (or return an existing) cookie definition. */
  register(name: string, opts?: CookieOptions): Cookie;
  /** Cache value for this client, else the cookie's default, else "". "" for bots. */
  get(client: Client, cookie: Cookie): string;
  /** Write the cache + mark dirty (flushed to the DB on disconnect). No-op for bots. */
  set(client: Client, cookie: Cookie, value: string): void;
  /** Has this client's cookies finished loading from the DB? */
  areCached(client: Client): boolean;
};
