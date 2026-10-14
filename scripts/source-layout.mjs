// Native builds consume frozen contracts/resources. Only provenance tools
// use the separate original Go/Web checkout.
import fs from 'node:fs';
import path from 'node:path';
export const repositoryRoot = path.resolve(import.meta.dirname, '..');
export const originalRoot = path.resolve(process.env.ARTEX_ORIGINAL_ROOT || path.join(repositoryRoot, '..', 'original'));
export function requireOriginalRoot() {
  if (!fs.existsSync(path.join(originalRoot, 'go.mod')) || !fs.existsSync(path.join(originalRoot, 'db/schema.sql'))) {
    throw new Error('Original ARTEX checkout not found. Set ARTEX_ORIGINAL_ROOT to its absolute directory. Native Cargo builds do not require this checkout.');
  }
  return originalRoot;
}
