/**
 * ~/.s2script/credentials.json — deploy token storage (v1 plaintext file).
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

export interface Credentials {
  registryUrl: string;
  token: string;
}

export function credentialsPath(): string {
  return join(homedir(), ".s2script", "credentials.json");
}

/**
 * Canonicalizes a registry URL: strips the trailing slash and rewrites the bare
 * apex to www. The apex 301s (which drops Authorization and turns POST into GET
 * for pre-exemption servers), and pre-0.8 credential files recorded it.
 */
export function normalizeRegistryUrl(url: string): string {
  const trimmed = url.replace(/\/$/, "");
  if (trimmed === "https://s2script.com") return "https://www.s2script.com";
  return trimmed;
}

export function defaultRegistryUrl(): string {
  const fromEnv = process.env.S2SCRIPT_REGISTRY_URL;
  if (fromEnv) return normalizeRegistryUrl(fromEnv);
  return "https://www.s2script.com";
}

export function loadCredentials(): Credentials | null {
  const fromEnv = process.env.S2SCRIPT_TOKEN || process.env.S2SCRIPT_DEPLOY_TOKEN;
  if (fromEnv) {
    return { registryUrl: defaultRegistryUrl(), token: fromEnv };
  }
  const p = credentialsPath();
  if (!existsSync(p)) return null;
  try {
    const j = JSON.parse(readFileSync(p, "utf8")) as Credentials;
    if (!j.token) return null;
    return {
      registryUrl: normalizeRegistryUrl(j.registryUrl || defaultRegistryUrl()),
      token: j.token,
    };
  } catch {
    return null;
  }
}

export function saveCredentials(creds: Credentials): void {
  const dir = join(homedir(), ".s2script");
  mkdirSync(dir, { recursive: true });
  writeFileSync(
    credentialsPath(),
    JSON.stringify(
      { registryUrl: normalizeRegistryUrl(creds.registryUrl), token: creds.token },
      null,
      2
    ) + "\n",
    { mode: 0o600 }
  );
}
