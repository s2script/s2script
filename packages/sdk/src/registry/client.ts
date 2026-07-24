/**
 * Thin HTTPS client for the s2script.com registry API.
 *
 * Every request goes through request(): per-op timeouts, network-failure
 * framing, and one error extractor that understands every shape the server
 * emits — {error} envelopes (deploy/device endpoints), SvelteKit {message}
 * errors (resolve/meta/download), plain text (the CSRF guard), and HTML pages.
 */

import { zipSync } from "fflate";

export class RegistryError extends Error {
  /** HTTP status, or 0 for network-level failures (DNS, refused, timeout). */
  status: number;
  body?: unknown;

  // Fields are declared/assigned explicitly rather than as constructor parameter
  // properties: the test runner loads src/*.ts under node's strip-only type
  // stripping, which rejects `constructor(public status: number)`.
  constructor(message: string, status: number, body?: unknown) {
    super(message);
    this.name = "RegistryError";
    this.status = status;
    this.body = body;
  }
}

const TIMEOUTS_MS = {
  deploy: 300_000, // up to ~40 MiB on a slow uplink
  resolve: 30_000,
  meta: 30_000,
  "types download": 120_000,
} as const;

type Op = keyof typeof TIMEOUTS_MS;

/** Unwraps nested Error.cause chains — undici hides the real reason one or two levels down. */
function rootCause(err: unknown): string {
  let cur: unknown = err;
  for (let i = 0; i < 5 && cur instanceof Error && cur.cause; i++) cur = cur.cause;
  return cur instanceof Error ? cur.message : String(cur);
}

/** What the user should do next, per status. Empty when there is nothing useful to say. */
function hintFor(status: number, raw: string): string {
  if (status === 401) return "run `s2s login` (or set S2SCRIPT_TOKEN)";
  if (status === 403 && /cross-site/i.test(raw)) {
    return "the request was rejected before it reached the registry API — this s2s CLI and the server are incompatible; run: npm i -g @s2script/sdk@latest";
  }
  if (status === 403) return "the token may lack permission for this package, or belong to another account — check `s2s login`";
  if (status === 413) return "the plugin or its types exceed the registry size limits — trim the archive and retry";
  if (status === 415) return "the server rejected this CLI's request format; run: npm i -g @s2script/sdk@latest";
  if (status === 429) return "rate limited — wait a minute and retry";
  if (status >= 500) return "server-side failure — retry in a moment; if it persists, report it with the message above";
  return "";
}

async function errorFromResponse(res: Response, op: Op): Promise<RegistryError> {
  const raw = await res.text().catch(() => "");
  let body: unknown;
  let detail: string | undefined;
  try {
    body = JSON.parse(raw);
    const b = body as { error?: unknown; message?: unknown };
    if (b && typeof b.error === "string") detail = b.error;
    else if (b && typeof b.message === "string") detail = b.message;
  } catch {
    // not JSON — fall through to the text/HTML handling below
  }
  if (!detail) {
    if (/^\s*</.test(raw)) detail = "server returned an HTML error page";
    else if (raw.trim()) detail = raw.trim().slice(0, 200);
    else detail = res.statusText || "no response body";
  }
  let msg = `${op} failed (HTTP ${res.status}): ${detail}`;
  const hint = hintFor(res.status, raw);
  if (hint) msg += `\n  hint: ${hint}`;
  return new RegistryError(msg, res.status, body ?? raw);
}

export interface RegistryClientOpts {
  baseUrl: string;
  token?: string;
  fetch?: typeof fetch;
}

export class RegistryClient {
  readonly baseUrl: string;
  private token?: string;
  private fetchFn: typeof fetch;

  constructor(opts: RegistryClientOpts) {
    this.baseUrl = opts.baseUrl.replace(/\/$/, "");
    this.token = opts.token;
    this.fetchFn = opts.fetch ?? fetch;
  }

  private headers(extra?: Record<string, string>): Record<string, string> {
    const h: Record<string, string> = { ...(extra ?? {}) };
    if (this.token) h.Authorization = `Bearer ${this.token}`;
    return h;
  }

  private async request(
    op: Op,
    url: string | URL,
    init?: { method?: string; body?: Uint8Array; contentType?: string }
  ): Promise<Response> {
    const timeoutMs = TIMEOUTS_MS[op];
    const host = new URL(String(url)).host;
    let res: Response;
    try {
      res = await this.fetchFn(url, {
        method: init?.method ?? "GET",
        // Cast: a Uint8Array is a valid BodyInit at runtime, but the ambient
        // fetch typings here only accept ArrayBuffer-backed views.
        body: init?.body as BodyInit | undefined,
        headers: this.headers(init?.contentType ? { "content-type": init.contentType } : undefined),
        signal: AbortSignal.timeout(timeoutMs),
      });
    } catch (e) {
      const name = e instanceof Error ? e.name : "";
      if (name === "TimeoutError" || name === "AbortError") {
        throw new RegistryError(
          `${op}: timed out after ${timeoutMs / 1000}s talking to ${host}\n  hint: check your connection or set S2SCRIPT_REGISTRY_URL if you meant a different registry`,
          0
        );
      }
      throw new RegistryError(
        `${op}: network error talking to ${host} — ${rootCause(e)}\n  hint: check your connection, proxy, and that ${host} is reachable`,
        0,
        e
      );
    }
    if (!res.ok) throw await errorFromResponse(res, op);
    return res;
  }

  private async json<T>(res: Response, op: Op): Promise<T> {
    try {
      return (await res.json()) as T;
    } catch {
      throw new RegistryError(`${op}: server returned invalid JSON (HTTP ${res.status})`, res.status);
    }
  }

  async deploy(opts: {
    manifest: Record<string, unknown>;
    s2sp: Buffer;
    types?: Buffer | null;
  }): Promise<{ name: string; version: string; reviewState: string; disclaimer?: string }> {
    const entries: Record<string, Uint8Array> = {
      "manifest.json": new TextEncoder().encode(JSON.stringify(opts.manifest)),
      "plugin.s2sp": new Uint8Array(opts.s2sp),
    };
    if (opts.types && opts.types.length) {
      entries["types.tgz"] = new Uint8Array(opts.types);
    }
    // level 0: plugin.s2sp is already deflated; recompressing wastes CPU for ~0 gain.
    const body = zipSync(entries, { level: 0 });
    const res = await this.request("deploy", `${this.baseUrl}/api/v1/deploy`, {
      method: "POST",
      body,
      contentType: "application/octet-stream",
    });
    return this.json(res, "deploy");
  }

  async resolve(
    name: string,
    range = "*"
  ): Promise<{
    name: string;
    version: string;
    reviewState: string;
    hasTypes: boolean;
    publishes?: unknown;
  }> {
    const u = new URL(`${this.baseUrl}/api/v1/resolve`);
    u.searchParams.set("name", name);
    u.searchParams.set("range", range);
    const res = await this.request("resolve", u);
    return this.json(res, "resolve");
  }

  async downloadTypes(name: string, version: string): Promise<Buffer> {
    const u = new URL(`${this.baseUrl}/api/v1/download/types`);
    u.searchParams.set("name", name);
    u.searchParams.set("version", version);
    const res = await this.request("types download", u);
    return Buffer.from(await res.arrayBuffer());
  }

  async meta(name: string): Promise<unknown> {
    const u = new URL(`${this.baseUrl}/api/v1/meta`);
    u.searchParams.set("name", name);
    const res = await this.request("meta", u);
    return this.json(res, "meta");
  }
}
