import assert from 'node:assert/strict';
import test from 'node:test';
import { writeFile, unlink } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { build } from 'esbuild';

globalThis.sessionStorage = { getItem: () => null, setItem: () => {}, removeItem: () => {} };
const bundlePath = `${process.cwd()}/.final-invoice-test-${process.pid}.mjs`;
let ShowroomFinalInvoice;
let ShowroomDebtPrintReport;
let ShowroomDebtPrintControls;
let loadCompleteFinalInvoice;
try {
  const result = await build({ entryPoints: ['src/App.tsx'], bundle: true, platform: 'node', format: 'esm', packages: 'external', jsx: 'automatic', write: false, define: { 'import.meta.env.VITE_API_URL': 'undefined' } });
  await writeFile(bundlePath, result.outputFiles[0].contents);
  ({ ShowroomFinalInvoice, ShowroomDebtPrintReport, ShowroomDebtPrintControls, loadCompleteFinalInvoice } = await import(pathToFileURL(bundlePath).href));
} finally {
  await unlink(bundlePath).catch(() => {});
}

test('final invoice keeps debt-report branding but only invoice content', () => {
  const html = renderToStaticMarkup(createElement(ShowroomFinalInvoice, {
    data: {
      showroom: { id: 'bank', name: 'مصرف الاختبار', phone: '0910000000', address: 'طرابلس', created_at: '2026-08-01T00:00:00Z', is_bank_account: true },
      operations: [
        { id: 'wash-1', vehicle_make: 'Toyota', vehicle_model: 'Camry', wash_type: 'غسيل كامل', performed_at: '2026-08-01T10:00:00Z', price_milli: '1001', license_plate: null, car_color: null },
        { id: 'wash-2', vehicle_make: 'Ford', vehicle_model: 'Focus', wash_type: null, performed_at: '2026-08-31T21:59:59Z', price_milli: '2500', license_plate: null, car_color: null },
      ],
      wash_count: 2, total_amount_milli: '3501',
    },
    range: { from: '2026-08-01', to: '2026-08-31' },
    generatedAt: new Date('2026-09-01T10:00:00Z'),
  }));
  assert.match(html, /الفاتورة النهائية/);
  assert.match(html, /مركز الكحيلي لغسيل السيارات/);
  assert.match(html, /الفاتورة من تاريخ/);
  assert.match(html, /نوع السيارة<\/th><th>نوع الغسيل/);
  assert.match(html, /غسيل كامل/);
  assert.match(html, /غير محدد/);
  assert.match(html, /1\.001/);
  assert.match(html, /2\.50/);
  assert.match(html, /3\.501/);
  for (const debtOnly of ['صدر بواسطة', 'إجمالي الدين', 'إجمالي المدفوع', 'إجمالي المتبقي', 'تقرير دين المعرض']) {
    assert.doesNotMatch(html, new RegExp(debtOnly));
  }
});

test('the original debt report retains its issuer and debt summary', () => {
  const html = renderToStaticMarkup(createElement(ShowroomDebtPrintReport, {
    profile: {
      showroom: { id: 'bank', name: 'مصرف الاختبار', created_at: '2026-08-01T00:00:00Z' },
      operations: [], payments: [], outstanding_wash_count: 0,
      total_charges: 0, total_payments: 0, total_outstanding: 0,
    },
    range: { from: '2026-08-01', to: '2026-08-31' },
    reportIssuer: 'إبراهيم الكحيلي',
    generatedAt: new Date('2026-09-01T10:00:00Z'),
  }));
  for (const original of ['تقرير دين المعرض', 'صدر بواسطة', 'إبراهيم الكحيلي', 'إجمالي الدين', 'إجمالي المدفوع', 'إجمالي المتبقي']) {
    assert.match(html, new RegExp(original));
  }
  assert.doesNotMatch(html, /الفاتورة النهائية/);
});

test('bank accounts see adjacent print actions using one date range', () => {
  const props = {
    range: { from: '2026-08-01', to: '2026-08-31' },
    onRangeChange: () => {}, onDebtPrint: () => {}, onInvoicePrint: () => {},
    loading: false, preparingPrint: false, disabled: false,
  };
  const bank = renderToStaticMarkup(createElement(ShowroomDebtPrintControls, { ...props, isBankAccount: true }));
  assert.match(bank, /value="2026-08-01"/);
  assert.match(bank, /value="2026-08-31"/);
  assert.match(bank, /طباعة تقرير الدين[\s\S]*طباعة الفاتورة النهائية/);
  const ordinary = renderToStaticMarkup(createElement(ShowroomDebtPrintControls, { ...props, isBankAccount: false }));
  assert.match(ordinary, /طباعة تقرير الدين/);
  assert.doesNotMatch(ordinary, /طباعة الفاتورة النهائية/);
});

test('a multi-page invoice keeps every wash in one continuously paginated table', () => {
  const operations = Array.from({ length: 120 }, (_, index) => ({
    id: `wash-${index}`,
    vehicle_make: 'سيارة باسم طويل لاختبار التفاف السطر',
    vehicle_model: `موديل ${index + 1}`,
    wash_type: 'غسيل كامل من الداخل والخارج',
    performed_at: '2026-08-15T10:00:00Z',
    price_milli: '1001',
    license_plate: null,
    car_color: null,
  }));
  const html = renderToStaticMarkup(createElement(ShowroomFinalInvoice, {
    data: { showroom: { id: 'bank', name: 'مصرف الاختبار', is_bank_account: true }, operations, wash_count: 120, total_amount_milli: '120120' },
    range: { from: '2026-08-01', to: '2026-08-31' },
    generatedAt: new Date('2026-09-01T10:00:00Z'),
  }));
  assert.equal((html.match(/<table>/g) ?? []).length, 1);
  assert.equal((html.match(/<tbody><tr>|<\/tr><tr>/g) ?? []).length, 120);
  assert.match(html, /120\.12/);
});

test('shared invoice loader requests one complete export and forwards the business date range', async () => {
  const originalFetch = globalThis.fetch;
  const urls = [];
  globalThis.fetch = async (url, options) => {
    urls.push(String(url));
    assert.equal(options.method ?? 'GET', 'GET');
    const page = new URL(String(url), 'http://localhost');
    assert.equal(page.pathname, '/api/showroom-debts/settled/final-invoice');
    assert.equal(page.searchParams.get('from'), '2026-08-31T22:00:00.000Z');
    assert.equal(page.searchParams.get('to'), '2026-09-23T21:59:59.999Z');
    assert.equal(page.searchParams.has('cursor'), false);
    const operations = Array.from({ length: 101 }, (_, index) => ({
      id: `wash-${index}`, vehicleMake: 'Toyota', vehicleModel: 'Camry', washType: null,
      occurredAt: '2026-09-10T10:00:00.000Z', priceMilli: '1000', licensePlate: null, carColor: null,
    }));
    return new Response(JSON.stringify({ data: { showroom: { id: 'settled', name: 'مصرف الجمهورية', isBankAccount: true },
      operations, washCount: 101, totalAmountMilli: '101000' } }),
      { headers: { 'content-type': 'application/json' } });
  };
  try {
    const invoice = await loadCompleteFinalInvoice('settled', { from: '2026-09-01', to: '2026-09-23' });
    assert.equal(invoice.operations.length, 101);
    assert.equal(new Set(invoice.operations.map((row) => row.id)).size, 101);
    assert.equal(invoice.total_amount_milli, '101000');
    assert.equal(urls.length, 1);
  } finally { globalThis.fetch = originalFetch; }
});

test('empty invoice is explicit and API errors do not become zero totals', async () => {
  const originalFetch = globalThis.fetch;
  try {
    globalThis.fetch = async () => new Response(JSON.stringify({ data: {
      showroom: { id: 'settled', name: 'مصرف الجمهورية', isBankAccount: true },
      operations: [], washCount: 0, totalAmountMilli: '0',
    } }), { headers: { 'content-type': 'application/json' } });
    const empty = await loadCompleteFinalInvoice('settled', { from: '2026-09-01', to: '2026-09-23' });
    assert.equal(empty.wash_count, 0);
    globalThis.fetch = async () => new Response(JSON.stringify({ error: 'تعذر تحميل الفاتورة' }),
      { status: 500, headers: { 'content-type': 'application/json' } });
    await assert.rejects(loadCompleteFinalInvoice('settled', { from: '2026-09-01', to: '2026-09-23' }), /تعذر تحميل الفاتورة/);
  } finally { globalThis.fetch = originalFetch; }
});

test('invoice loader rejects incomplete, duplicated, and oversized exports', async () => {
  const originalFetch = globalThis.fetch;
  const row = { id: 'wash-1', vehicleMake: 'Toyota', vehicleModel: 'Camry', washType: null,
    occurredAt: '2026-09-10T10:00:00.000Z', priceMilli: '1000', licensePlate: null, carColor: null };
  const range = { from: '2026-09-01', to: '2026-09-23' };
  try {
    for (const operations of [[row], [row, row]]) {
      globalThis.fetch = async () => new Response(JSON.stringify({ data: {
        showroom: { id: 'settled', name: 'مصرف الجمهورية', isBankAccount: true },
        operations, washCount: 2, totalAmountMilli: String(operations.length * 1000),
      } }), { headers: { 'content-type': 'application/json' } });
      await assert.rejects(loadCompleteFinalInvoice('settled', range), /تعذر التحقق من اكتمال الفاتورة/);
    }
    globalThis.fetch = async () => new Response(JSON.stringify({ error: 'تتجاوز الفاتورة الحد الأقصى (5000 غسلة). اختر فترة أقصر.' }),
      { status: 413, headers: { 'content-type': 'application/json' } });
    await assert.rejects(loadCompleteFinalInvoice('settled', range), /اختر فترة أقصر/);
  } finally { globalThis.fetch = originalFetch; }
});
