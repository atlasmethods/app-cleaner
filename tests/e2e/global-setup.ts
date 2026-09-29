import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '../..');

/** Builds nothing: only verifies the prerequisites exist so failures are obvious. */
export default function globalSetup(): void {
  const exe = process.platform === 'win32' ? 'clearsweep.exe' : 'clearsweep';
  const missing: string[] = [];
  if (!existsSync(path.join(root, 'dist', 'index.html'))) missing.push('dist/index.html (run `pnpm build`)');
  if (!existsSync(path.join(root, 'target', 'debug', exe)))
    missing.push(`target/debug/${exe} (run \`cargo build -p sweep-cli\`)`);
  else if (spawnSync(path.join(root, 'target', 'debug', exe), ['dev-fixture', '--help']).status !== 0)
    missing.push('target/debug/' + exe + ' without dev-fixture (build it with `cargo build -p sweep-cli --features testutil`)');
  if (missing.length) throw new Error(`E2E prerequisites missing:\n  - ${missing.join('\n  - ')}`);
}
