/**
 * npm's package access, for `s2s deploy`: `public` (anyone can see and install it) or `restricted`
 * (only the owning org's members). Chosen the way npm chooses it — the `--access` flag, else
 * package.json `publishConfig.access` — and applied by the registry when the package is first
 * published. Neither set means the registry decides: public for a new package, unchanged for an
 * existing one.
 *
 * Not to be confused with package.json `"private": true`, which (as in npm) means "never publish"
 * and is refused by `assertDeployable`.
 */

import type { RegistryClient } from "./client.ts";

export type PackageAccess = "public" | "restricted";

function parseAccess(value: unknown, source: string): PackageAccess {
  if (value === "public" || value === "restricted") return value;
  throw new Error(`${source} must be "public" or "restricted", not ${JSON.stringify(value)}`);
}

/** The access a deploy asks for, or undefined to leave it to the registry. */
export function resolveAccess(
  pkg: { name?: string; publishConfig?: unknown },
  flag?: string,
): PackageAccess | undefined {
  if (flag !== undefined) return parseAccess(flag, "--access");
  const fromPkg = (pkg.publishConfig as { access?: unknown } | undefined)?.access;
  if (fromPkg === undefined) return undefined;
  return parseAccess(fromPkg, `${pkg.name ?? "package.json"} publishConfig.access`);
}

/** npm's rule, checked before anything builds: only a scoped name has an org to restrict it to. */
export function assertAccessAllowed(name: string, access: PackageAccess | undefined): void {
  if (access === "restricted" && !name.startsWith("@")) {
    throw new Error(
      `${name} is unscoped; only a scoped package (@scope/name) can be restricted, as on npm`,
    );
  }
}

/**
 * Refuse a restricted deploy unless the registry says it can keep the package restricted. A
 * registry that predates restricted packages ignores the manifest's `access` field, so without
 * this check the package would go out PUBLIC — exactly what the author asked to prevent.
 */
export async function assertRegistrySupportsAccess(
  client: RegistryClient,
  access: PackageAccess | undefined,
): Promise<void> {
  if (access !== "restricted") return;
  const supported = await client.supportedAccess();
  if (!supported.includes("restricted")) {
    throw new Error(
      `${client.baseUrl} does not support restricted packages yet, so nothing was published — ` +
        `deploying would make the package public. Drop --access restricted / publishConfig.access ` +
        `to publish it publicly, or wait for the registry to enable it.`,
    );
  }
}
