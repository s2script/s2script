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

export function defaultRegistryUrl(): string {
  return process.env.S2SCRIPT_REGISTRY_URL?.replace(/\/$/, "") || "https://s2script.com";
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
      registryUrl: (j.registryUrl || defaultRegistryUrl()).replace(/\/$/, ""),
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
      { registryUrl: creds.registryUrl.replace(/\/$/, ""), token: creds.token },
      null,
      2
    ) + "\n",
    { mode: 0o600 }
  );
}
