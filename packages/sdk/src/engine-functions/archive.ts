import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseFunctionFile } from './parse.ts';
import { extractInlineFunctions, locateInlineError, mergeInlineFunctions } from './inline.ts';
import { normalizeFunctions, summarizeFunctions } from './normalize.ts';
import { canonicalJson } from './canonical-json.ts';
import type { NormalizedBundle } from './model.ts';

/** The plugin's engine functions: `gamedata/functions.jsonc` plus its inline declarations
 *  (`Engine.fromSignature`/`fromVirtual`). Both build and standalone typecheck use this path. */
export function loadEngineFunctions(pluginDir: string, ownerId: string): NormalizedBundle | undefined {
  const path = join(pluginDir, 'gamedata', 'functions.jsonc');
  const file = existsSync(path) ? parseFunctionFile(path, readFileSync(path, 'utf8')) : undefined;
  const inline = extractInlineFunctions(pluginDir);
  const merged = mergeInlineFunctions(file, inline);
  try { return normalizeFunctions(ownerId, merged); }
  catch (error) { throw locateInlineError(error, inline); }
}

export function engineFunctionsArchive(bundle: NormalizedBundle): string {
  return canonicalJson(bundle) + '\n';
}

/** Permissions are derived separately; the manifest summary never authors a second contract. */
export function engineFunctionsManifest(bundle: NormalizedBundle) {
  const { permissions, ...summary } = summarizeFunctions(bundle);
  return { summary, permissions };
}
