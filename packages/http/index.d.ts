/** @s2script/http — async HTTP. NO runtime code (injected as __s2pkg_http). */
export interface FetchOptions { method?: string; headers?: Record<string, string>; body?: string; timeoutMs?: number; }
export interface Response {
  readonly status: number; readonly ok: boolean; readonly statusText: string;
  readonly headers: Record<string, string>;
  text(): string;
  json<T = unknown>(): T;
}
/** Perform an HTTP request off the game thread. Rejects on a network error / timeout; an HTTP
 *  status (incl. 4xx/5xx) RESOLVES with ok=false. */
export declare function fetch(url: string, options?: FetchOptions): Promise<Response>;
