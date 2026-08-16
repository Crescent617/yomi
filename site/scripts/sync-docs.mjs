// Syncs repo docs into the site content collection at build/dev time.
// Source of truth stays in ../docs; generated files are gitignored.
import { cpSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const site = dirname(fileURLToPath(import.meta.url));
const jobs = [['../../docs/CONFIG.md', '../src/content/docs/config.md']];

for (const [from, to] of jobs) {
  const dest = join(site, to);
  mkdirSync(dirname(dest), { recursive: true });
  cpSync(join(site, from), dest);
  console.log(`synced ${from} → ${to}`);
}
