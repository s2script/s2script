import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseFunctionFile } from './parse.ts';
import { normalizeFunctions, summarizeFunctions } from './normalize.ts';
import { canonicalJson } from './canonical-json.ts';
import type { NormalizedBundle } from './model.ts';

/** The only v2 authoring location. Both build and standalone typecheck use this path. */
export function loadEngineFunctions(pluginDir: string, ownerId: string): NormalizedBundle | undefined {
  const path = join(pluginDir, 'gamedata', 'functions.jsonc');
  if (!existsSync(path)) return undefined;
  return normalizeFunctions(ownerId, parseFunctionFile(path, readFileSync(path, 'utf8')));
}

export function engineFunctionsArchive(bundle: NormalizedBundle): string {
  return canonicalJson(bundle) + '\n';
}

/** Permissions are derived separately; the manifest summary never authors a second contract. */
export function engineFunctionsManifest(bundle: NormalizedBundle) {
  const { permissions, ...summary } = summarizeFunctions(bundle);
  return { summary, permissions };
}
