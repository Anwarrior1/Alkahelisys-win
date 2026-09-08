import { performance } from 'node:perf_hooks';
import { writeFile } from 'node:fs/promises';

const mode = process.argv[2] ?? 'baseline';
const output = process.argv[3];
const sizes = [150, 600, 2400, 9600];
const pageSize = 150;
const virtualRows = 28;
const virtualizationThreshold = 200;

function record(index) {
  return { id: `wash-${index}`, vehicle: `Toyota Camry ${index}`, worker: `Worker ${index % 40}`, amount: index % 250 };
}

function renderRow(item) {
  return `<div class="wash-row" data-id="${item.id}"><div><svg></svg><span><strong>${item.vehicle}</strong><small>plate</small></span></div><span>${item.worker}</span><span>2026-09-07</span><span>cash</span><strong>${item.amount}</strong><button>edit</button></div>`;
}

function currentMerge(existing, incoming) {
  const known = new Set(existing.map((item) => item.id));
  return [...existing, ...incoming.filter((item) => !known.has(item.id))];
}

function indexedMerge(existing, incoming, known) {
  const additions = [];
  for (const item of incoming) {
    if (known.has(item.id)) continue;
    known.add(item.id);
    additions.push(item);
  }
  return additions.length === 0 ? existing : existing.concat(additions);
}

function median(values) {
  const ordered = [...values].sort((a, b) => a - b);
  return ordered[Math.floor(ordered.length / 2)];
}

function time(operation, iterations = 9) {
  const samples = [];
  for (let index = 0; index < iterations; index += 1) {
    const started = performance.now();
    operation();
    samples.push(performance.now() - started);
  }
  return Number(median(samples).toFixed(3));
}

const results = sizes.map((size) => {
  const items = Array.from({ length: size }, (_, index) => record(index));
  const mounted = mode === 'baseline' || size <= virtualizationThreshold ? items : items.slice(0, virtualRows);
  const renderMs = time(() => mounted.map(renderRow).join(''));
  const markup = mounted.map(renderRow).join('');
  let accumulated = [];
  const known = new Set();
  const mergeMs = time(() => {
    accumulated = [];
    known.clear();
    for (let offset = 0; offset < items.length; offset += pageSize) {
      const page = items.slice(offset, offset + pageSize);
      accumulated = mode === 'baseline' ? currentMerge(accumulated, page) : indexedMerge(accumulated, page, known);
    }
  });
  return {
    loadedRows: size,
    mountedRows: mounted.length,
    approximateRowDomNodes: mounted.length * 14,
    markupBytes: Buffer.byteLength(markup),
    renderMs,
    cumulativePageMergeMs: mergeMs,
  };
});

const report = { mode, pageSize, virtualRows, virtualizationThreshold, results };
if (output) await writeFile(output, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));
