import { createHash } from 'node:crypto';
import { readdir, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const requiredFonts = [
  ['public/fonts/beIN Normal.ttf', 'dist/fonts/beIN Normal.ttf'],
  ['public/fonts/beIN Black Black.ttf', 'dist/fonts/beIN Black Black.ttf'],
];

const digest = (contents) => createHash('sha256').update(contents).digest('hex');

for (const [sourceRelative, builtRelative] of requiredFonts) {
  const source = await readFile(resolve(repositoryRoot, sourceRelative));
  const built = await readFile(resolve(repositoryRoot, builtRelative));
  if (digest(source) !== digest(built)) {
    throw new Error(`Production font differs from its source asset: ${builtRelative}`);
  }
}

const assetDirectory = resolve(repositoryRoot, 'dist', 'assets');
const cssFiles = (await readdir(assetDirectory)).filter((name) => name.endsWith('.css'));
const productionCss = (
  await Promise.all(cssFiles.map((name) => readFile(resolve(assetDirectory, name), 'utf8')))
).join('\n');

const requiredCss = [
  '/fonts/beIN%20Normal.ttf',
  '/fonts/beIN%20Black%20Black.ttf',
  'font-weight:400',
  'font-weight:700',
  'font-synthesis:none',
  'font-feature-settings:normal',
  'font-variant-ligatures:common-ligatures contextual',
];

for (const declaration of requiredCss) {
  if (!productionCss.includes(declaration)) {
    throw new Error(`Production CSS is missing required Arabic-font declaration: ${declaration}`);
  }
}

if (/font-weight:100 500|font-weight:600 900/.test(productionCss)) {
  throw new Error('Production CSS still declares a static Arabic font as a variable weight range.');
}

console.log('Verified Arabic font files and declarations in the production frontend bundle.');
