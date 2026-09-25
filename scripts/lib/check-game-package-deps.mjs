import { createRequire } from 'node:module';
import { realpathSync } from 'node:fs';
import { dirname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const installed = join(root, 'node_modules/typescript');
try {
  const require = createRequire(join(root, 'package.json'));
  const resolved = realpathSync(require.resolve('typescript/package.json'));
  const expected = realpathSync(installed);
  if (resolved !== join(expected, 'package.json') || !resolved.startsWith(expected + sep))
    throw new Error('TypeScript resolved outside root installation');
} catch {
  console.error('ERROR: game-package function parser requires locked TypeScript; run npm ci at the repo root before packaging.');
  process.exitCode = 1;
}
