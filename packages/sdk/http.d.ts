/** @s2script/http — async HTTP. NO runtime code (injected as __s2pkg_http). */

/** Options for {@link fetch}. */
export interface FetchOptions {
  /** HTTP method (e.g. `"GET"`, `"POST"`). @defaultValue `"GET"` */
  method?: string;
  /** Request headers as a name→value map. */
  headers?: Record<string, string>;
  /** Request body (already-serialized string; set your own `content-type`). */
  body?: string;
  /** Abort the request after this many milliseconds. @defaultValue no timeout */
  timeoutMs?: number;
}

/** The response from a {@link fetch} call — a copied snapshot (no live socket). */
export interface Response {
  /** HTTP status code (e.g. `200`, `404`). */
  readonly status: number;
  /** True iff {@link Response.status} is in the 2xx range. */
  readonly ok: boolean;
  /** HTTP status reason phrase (e.g. `"Not Found"`). */
  readonly statusText: string;
  /** Response headers as a lower-cased name→value map. */
  readonly headers: Record<string, string>;
  /** The response body decoded as text. */
  text(): string;
  /** Parse the response body as JSON into `T` (throws on invalid JSON). */
  json<T = unknown>(): T;
}

/**
 * Perform an HTTP request off the game thread.
 * @param url - Absolute `http(s)://` URL.
 * @param options - Method, headers, body, and timeout (see {@link FetchOptions}).
 * @returns Resolves for ANY HTTP response — a 4xx/5xx resolves with `ok=false`, it does not reject.
 * @throws Rejects only on a network error or timeout.
 * @example
 * import { fetch } from "@s2script/sdk/http";
 * const r = await fetch("https://httpbin.org/get", { timeoutMs: 15000 });
 * if (r.ok) console.log(r.json<{ url: string }>().url);
 */
export declare function fetch(url: string, options?: FetchOptions): Promise<Response>;
