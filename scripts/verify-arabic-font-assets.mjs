import { createHash } from 'node:crypto';
import { readdir, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const requiredFonts = [
  ['Public/fonts1/ArbFONTS-22326-alarabiyafont.ttf', 'dist/fonts1/ArbFONTS-22326-alarabiyafont.ttf'],
  ['Public/fonts1/ArbFONTS-4_C6.ttf', 'dist/fonts1/ArbFONTS-4_C6.ttf'],
  ['Public/fonts1/ArbFONTS-Alarabiya Normal Font.ttf', 'dist/fonts1/ArbFONTS-Alarabiya Normal Font.ttf'],
];

const representativeArabic = [
  'انور النعاس',
  'اختر العامل',
  'نوع الزبون',
  'زبون عادي',
  'أحدث العمليات',
  'لا يوجد رقم اتصال',
  'تعديل العامل',
  'الفترة المعروضة',
  'مركز الكحيلي لغسيل السيارات',
  'الإدارة والمالية',
  'السيارات المبيتة',
  'ديون المعارض',
].join('');

function inspectTrueType(contents) {
  const uint16 = (offset) => contents.readUInt16BE(offset);
  const uint32 = (offset) => contents.readUInt32BE(offset);
  const tables = new Map();
  for (let index = 0; index < uint16(4); index += 1) {
    const offset = 12 + index * 16;
    tables.set(contents.toString('ascii', offset, offset + 4), {
      offset: uint32(offset + 8),
      length: uint32(offset + 12),
    });
  }

  const nameTable = tables.get('name');
  const names = new Map();
  const nameCount = uint16(nameTable.offset + 2);
  const stringsOffset = nameTable.offset + uint16(nameTable.offset + 4);
  for (let index = 0; index < nameCount; index += 1) {
    const offset = nameTable.offset + 6 + index * 12;
    const platform = uint16(offset);
    const nameId = uint16(offset + 6);
    if (platform !== 3 || names.has(nameId)) continue;
    const length = uint16(offset + 8);
    const valueOffset = stringsOffset + uint16(offset + 10);
    let value = '';
    for (let cursor = valueOffset; cursor < valueOffset + length; cursor += 2) {
      value += String.fromCharCode(uint16(cursor));
    }
    names.set(nameId, value);
  }

  const codepoints = new Set();
  const cmap = tables.get('cmap');
  const cmapCount = uint16(cmap.offset + 2);
  for (let index = 0; index < cmapCount; index += 1) {
    const record = cmap.offset + 4 + index * 8;
    const platform = uint16(record);
    const encoding = uint16(record + 2);
    if (!((platform === 3 && encoding === 1) || platform === 0)) continue;
    const subtable = cmap.offset + uint32(record + 4);
    if (uint16(subtable) !== 4) continue;
    const segmentCount = uint16(subtable + 6) / 2;
    const endCodes = subtable + 14;
    const startCodes = endCodes + segmentCount * 2 + 2;
    const deltas = startCodes + segmentCount * 2;
    const rangeOffsets = deltas + segmentCount * 2;
    for (let segment = 0; segment < segmentCount; segment += 1) {
      const end = uint16(endCodes + segment * 2);
      const start = uint16(startCodes + segment * 2);
      const delta = uint16(deltas + segment * 2);
      const rangeOffsetAddress = rangeOffsets + segment * 2;
      const rangeOffset = uint16(rangeOffsetAddress);
      for (let codepoint = start; codepoint <= end && codepoint !== 0xffff; codepoint += 1) {
        let glyph = (codepoint + delta) & 0xffff;
        if (rangeOffset) {
          glyph = uint16(rangeOffsetAddress + rangeOffset + (codepoint - start) * 2);
          if (glyph) glyph = (glyph + delta) & 0xffff;
        }
        if (glyph) codepoints.add(codepoint);
      }
    }
  }

  const os2 = tables.get('OS/2');
  const head = tables.get('head');
  return {
    family: names.get(1),
    style: names.get(2),
    weightClass: uint16(os2.offset + 4),
    macStyle: uint16(head.offset + 44),
    hasArabicShaping: tables.has('GSUB'),
    codepoints,
  };
}

const digest = (contents) => createHash('sha256').update(contents).digest('hex');

for (const [sourceRelative, builtRelative] of requiredFonts) {
  const source = await readFile(resolve(repositoryRoot, sourceRelative));
  const built = await readFile(resolve(repositoryRoot, builtRelative));
  if (digest(source) !== digest(built)) {
    throw new Error(`Production font differs from its source asset: ${builtRelative}`);
  }

  const metadata = inspectTrueType(source);
  if (metadata.family !== 'Alarabiya Font' || metadata.style !== 'Normal') {
    throw new Error(`Unexpected font identity in ${sourceRelative}`);
  }
  // These files carry a malformed OS/2 weight value of 5, but their name and
  // style records identify a non-bold Normal face. CSS normalizes that face to 400.
  if (metadata.weightClass !== 5 || metadata.macStyle !== 0) {
    throw new Error(`Unexpected font style metadata in ${sourceRelative}`);
  }
  if (!metadata.hasArabicShaping) {
    throw new Error(`Arabic GSUB shaping data is missing from ${sourceRelative}`);
  }
  for (const character of `${representativeArabic}0123456789`) {
    if (character !== ' ' && !metadata.codepoints.has(character.codePointAt(0))) {
      throw new Error(`Required character ${character} is missing from ${sourceRelative}`);
    }
  }
}

const assetDirectory = resolve(repositoryRoot, 'dist', 'assets');
const cssFiles = (await readdir(assetDirectory)).filter((name) => name.endsWith('.css'));
const productionCss = (
  await Promise.all(cssFiles.map((name) => readFile(resolve(assetDirectory, name), 'utf8')))
).join('\n');

const requiredCss = [
  '/fonts1/ArbFONTS-Alarabiya%20Normal%20Font.ttf',
  'font-family:Alarabiya Font',
  'font-family:Alkaheli Numerals,Alarabiya Font,Segoe UI,Tahoma,Arial,sans-serif',
  'font-family:Alkaheli Numerals',
  'src:local("Segoe UI"),local("Arial")',
  'src:local("Segoe UI Bold"),local("Arial Bold")',
  'unicode-range:U+0030-0039',
  'font-weight:400',
  'font-weight:700',
  'font-display:swap',
  'font-synthesis:none',
  'font-feature-settings:normal',
  'font-variant-ligatures:common-ligatures contextual',
  ':lang(ar){letter-spacing:normal;word-spacing:normal}',
];

for (const declaration of requiredCss) {
  if (!productionCss.includes(declaration)) {
    throw new Error(`Production CSS is missing required Arabic-font declaration: ${declaration}`);
  }
}

if (/\/fonts\/(?:beIN|GE-SS)|font-family:(?:"Alkaheli UI"|Alkaheli UI)/.test(productionCss)) {
  throw new Error('Production CSS still references the previous primary font.');
}

console.log('Verified Arabic font files and declarations in the production frontend bundle.');
