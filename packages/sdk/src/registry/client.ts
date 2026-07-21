/**
 * Thin HTTPS client for the s2script.com registry API.
 */

export class RegistryError extends Error {
  constructor(
    message: string,
    public status: number,
    public body?: unknown
  ) {
    super(message);
    this.name = "RegistryError";
  }
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

  async deploy(opts: {
    manifest: Record<string, unknown>;
    s2sp: Buffer;
    types?: Buffer | null;
  }): Promise<{ name: string; version: string; reviewState: string; disclaimer?: string }> {
    const fd = new FormData();
    fd.set("manifest", JSON.stringify(opts.manifest));
    fd.set("s2sp", new Blob([new Uint8Array(opts.s2sp)]), "plugin.s2sp");
    if (opts.types && opts.types.length) {
      fd.set("types", new Blob([new Uint8Array(opts.types)]), "types.tgz");
    }
    const res = await this.fetchFn(`${this.baseUrl}/api/v1/deploy`, {
      method: "POST",
      headers: this.headers(),
      body: fd,
    });
    const body = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new RegistryError(
        (body as { error?: string }).error || `deploy failed (${res.status})`,
        res.status,
        body
      );
    }
    return body as { name: string; version: string; reviewState: string; disclaimer?: string };
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
    const res = await this.fetchFn(u, { headers: this.headers() });
    const body = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new RegistryError(
        (body as { message?: string }).message || `resolve failed (${res.status})`,
        res.status,
        body
      );
    }
    return body as {
      name: string;
      version: string;
      reviewState: string;
      hasTypes: boolean;
      publishes?: unknown;
    };
  }

  async downloadTypes(name: string, version: string): Promise<Buffer> {
    const u = new URL(`${this.baseUrl}/api/v1/download/types`);
    u.searchParams.set("name", name);
    u.searchParams.set("version", version);
    const res = await this.fetchFn(u, { headers: this.headers() });
    if (!res.ok) {
      const body = await res.json().catch(() => ({}));
      throw new RegistryError(
        (body as { message?: string }).message || `types download failed (${res.status})`,
        res.status,
        body
      );
    }
    return Buffer.from(await res.arrayBuffer());
  }

  async meta(name: string): Promise<unknown> {
    const u = new URL(`${this.baseUrl}/api/v1/meta`);
    u.searchParams.set("name", name);
    const res = await this.fetchFn(u, { headers: this.headers() });
    const body = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new RegistryError(`meta failed (${res.status})`, res.status, body);
    }
    return body;
  }
}
