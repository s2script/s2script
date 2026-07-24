/**
 * Client half of the registry's device-authorization login (RFC 8628-shaped).
 * JSON POSTs on purpose — the registry rejects form encodings by design.
 */

export interface DeviceAuthStart {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  verificationUriComplete: string;
  expiresIn: number;
  interval: number;
}

const REQUEST_TIMEOUT_MS = 30_000;

/** Unwraps nested Error.cause chains — undici hides the real reason one or two levels down. */
function rootCause(err: unknown): string {
  let cur: unknown = err;
  for (let i = 0; i < 5 && cur instanceof Error && cur.cause; i++) cur = cur.cause;
  return cur instanceof Error ? cur.message : String(cur);
}

/**
 * Best available human detail from an error response: the {error} envelope, a
 * SvelteKit {message} error, plain text, or a marker for an HTML error page.
 */
async function describeError(res: Response): Promise<string> {
  const raw = await res.text().catch(() => "");
  try {
    const body = JSON.parse(raw) as { error?: unknown; message?: unknown };
    if (body && typeof body.error === "string") return body.error;
    if (body && typeof body.message === "string") return body.message;
  } catch {
    // not JSON — fall through
  }
  if (/^\s*</.test(raw)) return "server returned an HTML error page";
  if (raw.trim()) return raw.trim().slice(0, 200);
  return res.statusText || "no response body";
}

/** Returns null when the server predates device login (404/405) → caller falls back to paste. */
export async function startDeviceAuth(
  registryUrl: string,
  opts?: { fetchFn?: typeof fetch; client?: string }
): Promise<DeviceAuthStart | null> {
  const fetchFn = opts?.fetchFn ?? fetch;
  let res: Response;
  try {
    res = await fetchFn(`${registryUrl}/api/v1/auth/device/start`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ client: opts?.client }),
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
  } catch (e) {
    throw new Error(`could not reach ${registryUrl} — ${rootCause(e)}`);
  }
  if (res.status === 404 || res.status === 405) return null;
  if (!res.ok) {
    throw new Error(`login start failed (HTTP ${res.status}): ${await describeError(res)}`);
  }
  const body = (await res.json().catch(() => null)) as DeviceAuthStart | null;
  if (!body || typeof body.deviceCode !== "string" || typeof body.userCode !== "string") {
    throw new Error(`login start returned an unexpected response from ${registryUrl}`);
  }
  return body;
}

export async function pollForToken(
  registryUrl: string,
  start: DeviceAuthStart,
  opts?: { fetchFn?: typeof fetch; sleep?: (ms: number) => Promise<void> }
): Promise<{ token: string; tokenName?: string }> {
  const fetchFn = opts?.fetchFn ?? fetch;
  const sleep = opts?.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  let interval = Math.max(1, start.interval);
  const deadline = Date.now() + start.expiresIn * 1000;
  while (Date.now() < deadline) {
    await sleep(interval * 1000);
    let res: Response;
    try {
      res = await fetchFn(`${registryUrl}/api/v1/auth/device/poll`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ deviceCode: start.deviceCode }),
        signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
      });
    } catch (e) {
      throw new Error(`could not reach ${registryUrl} — ${rootCause(e)}`);
    }
    if (!res.ok) {
      throw new Error(`login poll failed (HTTP ${res.status}): ${await describeError(res)}`);
    }
    const body = (await res.json().catch(() => null)) as {
      status?: string;
      interval?: number;
      token?: string;
      tokenName?: string;
    } | null;
    if (!body) throw new Error("login poll returned a non-JSON response");
    switch (body.status) {
      case "pending":
        break;
      case "slow_down":
        interval += 5;
        break;
      case "approved":
        if (!body.token) throw new Error("server approved the login but sent no token");
        return { token: body.token, tokenName: body.tokenName };
      case "denied":
        throw new Error("login request was denied in the browser");
      case "expired":
        throw new Error("login request expired — run `s2s login` again");
      default:
        throw new Error(`unexpected login status: ${body.status}`);
    }
  }
  throw new Error("login request expired — run `s2s login` again");
}
