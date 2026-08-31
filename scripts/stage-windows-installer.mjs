import { copyFile, mkdir, readdir } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const bundleDirectory = resolve(
  repositoryRoot,
  'src-tauri',
  'target',
  'x86_64-pc-windows-msvc',
  'release',
  'bundle',
  'nsis',
);
const releaseDirectory = resolve(repositoryRoot, 'release');
const outputPath = resolve(releaseDirectory, 'Alkahelisys-Windows-x64-Setup.exe');

const installers = (await readdir(bundleDirectory, { withFileTypes: true }))
  .filter((entry) => entry.isFile() && entry.name.toLowerCase().endsWith('-setup.exe'))
  .map((entry) => resolve(bundleDirectory, entry.name));

if (installers.length !== 1) {
  throw new Error(`Expected exactly one NSIS installer in ${bundleDirectory}, found ${installers.length}.`);
}

await mkdir(releaseDirectory, { recursive: true });
await copyFile(installers[0], outputPath);
console.log(`Windows installer staged at ${outputPath}`);
