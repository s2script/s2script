import { readFileSync } from 'node:fs';
import { unzipSync } from 'fflate';
import { summarizeFunctions } from '../engine-functions/normalize.ts';
import { canonicalJson } from '../engine-functions/canonical-json.ts';
import type { NormalizedBundle } from '../engine-functions/model.ts';

interface Summary {
  schemaVersion: number;
  bundleHash: string;
  functions: Array<{ canonicalId: string; contractHash: string; requirement: string; surfaces: string[]; mutates: boolean; suppresses: boolean }>;
}

/** Read archive metadata only; never imports or executes plugin.js. */
export function inspectArchive(path: string): string {
  const files = unzipSync(readFileSync(path));
  if (!files['manifest.json']) throw new Error(`${path}: missing manifest.json`);
  const manifest = JSON.parse(Buffer.from(files['manifest.json']).toString('utf8')) as { id?: string; permissions?: string[]; engineFunctions?: Summary };
  const lines = [`plugin: ${manifest.id ?? '(unknown)'}`];
  const summary = manifest.engineFunctions;
  if (!summary) return [...lines, 'engine functions: none'].join('\n');
  if (summary.schemaVersion !== 2 || !files['engine-functions.json']) throw new Error(`${path}: incomplete engine functions contract`);
  const bundle = JSON.parse(Buffer.from(files['engine-functions.json']).toString('utf8')) as NormalizedBundle;
  if (bundle.bundleHash !== summary.bundleHash) throw new Error(`${path}: engine functions bundle hash mismatch`);
  const { permissions: _permissions, ...derivedSummary } = summarizeFunctions(bundle);
  if (canonicalJson(derivedSummary) !== canonicalJson(summary)) throw new Error(`${path}: engine functions summary mismatch`);
  lines.push(`engine functions bundle: ${summary.bundleHash}`);
  const derivedPermissions = [
    ...(summary.functions.some(f => f.surfaces.includes('call')) ? ['engine:calls'] : []),
    ...(summary.functions.some(f => f.surfaces.includes('pre') || f.surfaces.includes('post')) ? ['engine:hooks'] : []),
  ];
  lines.push(`derived permissions: ${derivedPermissions.join(', ') || '(none)'}`);
  for (const f of summary.functions) {
    lines.push(`${f.canonicalId} contract ${f.contractHash}`);
    lines.push(`  requirement: ${f.requirement}; surfaces: ${f.surfaces.join(', ')}; risks: ${[f.mutates ? 'mutation' : '', f.suppresses ? 'suppression' : ''].filter(Boolean).join(', ') || 'none'}`);
  }
  return lines.join('\n');
}

export async function run(argv: string[]): Promise<void> {
  if (argv.length !== 1) throw new Error('Usage: s2s inspect <archive.s2sp>');
  console.log(inspectArchive(argv[0]!));
}
