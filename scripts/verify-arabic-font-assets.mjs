import { createHash } from 'node:crypto';
import { readdir, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const requiredFonts = [
  {
    source: 'Public/fonts2/ArbFONTS-Cairo-Regular-4.ttf',
    built: 'dist/fonts2/ArbFONTS-Cairo-Regular-4.ttf',
    family: 'Cairo',
    style: 'Regular',
    postScriptName: 'Cairo-Regular',
    weight: 400,
  },
  {
    source: 'Public/fonts2/ArbFONTS-Cairo-SemiBold-3.ttf',
    built: 'dist/fonts2/ArbFONTS-Cairo-SemiBold-3.ttf',
    family: 'Cairo SemiBold',
    style: 'Regular',
    postScriptName: 'Cairo-SemiBold',
    weight: 600,
  },
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
    postScriptName: names.get(6),
    weightClass: uint16(os2.offset + 4),
    macStyle: uint16(head.offset + 44),
    hasArabicShaping: tables.has('GSUB'),
    codepoints,
  };
}

const digest = (contents) => createHash('sha256').update(contents).digest('hex');

for (const expected of requiredFonts) {
  const source = await readFile(resolve(repositoryRoot, expected.source));
  const built = await readFile(resolve(repositoryRoot, expected.built));
  if (digest(source) !== digest(built)) {
    throw new Error(`Production font differs from its source asset: ${expected.built}`);
  }

  const metadata = inspectTrueType(source);
  if (
    metadata.family !== expected.family
    || metadata.style !== expected.style
    || metadata.postScriptName !== expected.postScriptName
  ) {
    throw new Error(`Unexpected font identity in ${expected.source}`);
  }
  if (metadata.weightClass !== expected.weight || metadata.macStyle !== 0) {
    throw new Error(`Unexpected font style metadata in ${expected.source}`);
  }
  if (!metadata.hasArabicShaping) {
    throw new Error(`Arabic GSUB shaping data is missing from ${expected.source}`);
  }
  for (const character of `${representativeArabic}0123456789.,د.ل`) {
    if (character !== ' ' && !metadata.codepoints.has(character.codePointAt(0))) {
      throw new Error(`Required character ${character} is missing from ${expected.source}`);
    }
  }
}

const assetDirectory = resolve(repositoryRoot, 'dist', 'assets');
const cssFiles = (await readdir(assetDirectory)).filter((name) => name.endsWith('.css'));
const productionCss = (
  await Promise.all(cssFiles.map((name) => readFile(resolve(assetDirectory, name), 'utf8')))
).join('\n');

const requiredCss = [
  '/fonts2/ArbFONTS-Cairo-Regular-4.ttf',
  '/fonts2/ArbFONTS-Cairo-SemiBold-3.ttf',
  'font-family:Cairo Local',
  'font-family:Cairo Local,Segoe UI,Tahoma,Arial,sans-serif',
  'font-weight:400',
  'font-weight:600',
  'font-display:swap',
  'font-synthesis:none',
  'font-feature-settings:normal',
  'font-variant-ligatures:common-ligatures contextual',
  'font-variant-numeric:lining-nums tabular-nums',
  'line-height:1.55',
  'html:lang(ar) body *{letter-spacing:normal;word-spacing:normal}',
];

for (const declaration of requiredCss) {
  if (!productionCss.includes(declaration)) {
    throw new Error(`Production CSS is missing required Arabic-font declaration: ${declaration}`);
  }
}

if (
  /\/fonts1\/|font-family:(?:"?(?:Alarabiya Font|Alkaheli Numerals|Alkaheli UI)"?)/.test(productionCss)
) {
  throw new Error('Production CSS still references the previous primary font.');
}

console.log('Verified Cairo font files, weights, glyph coverage, and production CSS declarations.');
