import { migrateV1Package } from '../engine-functions/migrate-v1.ts';

/** Analyze the complete v1 declaration and source references before publishing v2 authoring. */
export async function run(argv: string[]): Promise<void> {
  const force = argv.includes('--force');
  const args = argv.filter(a => a !== '--force');
  if (args[0] !== 'engine-functions' || args.length > 2 || args.some(a => a.startsWith('-'))) {
    throw new Error('Usage: s2s migrate engine-functions [plugin-dir] [--force]');
  }
  const report = migrateV1Package(args[1] ?? process.cwd(), { force });
  const { output: _output, ...publicReport } = report;
  console.log(JSON.stringify(publicReport, null, 2));
  if (report.ambiguities.length) process.exitCode = 1;
}
