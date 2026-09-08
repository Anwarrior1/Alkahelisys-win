import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { once } from 'node:events';
import { performance } from 'node:perf_hooks';

const binary = process.argv[2];
const sizes = (process.argv[3] ?? '1000,5000,20000').split(',').map(Number);
const output = process.argv[4] ?? '/private/tmp/alkaheli-pagination-benchmark.json';
if (!binary) throw new Error('usage: node pagination-benchmark.mjs BINARY [sizes] [output]');
const root = 'http://127.0.0.1:8787/api';

const percentile = (values, p) => values.slice().sort((a, b) => a - b)[Math.min(values.length - 1, Math.ceil(values.length * p) - 1)];
async function raw(path, token) {
  const started = performance.now();
  const response = await fetch(`${root}${path}`, { headers: { Authorization: `Bearer ${token}` } });
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (!response.ok) throw new Error(`${path}: ${response.status} ${new TextDecoder().decode(bytes)}`);
  return { ms: performance.now() - started, bytes: bytes.length, body: JSON.parse(new TextDecoder().decode(bytes)).data };
}
async function measured(path, token) {
  const runs = [];
  let last;
  for (let index = 0; index < 6; index += 1) { last = await raw(path, token); if (index) runs.push(last.ms); }
  return { median_ms: +percentile(runs, .5).toFixed(3), p95_ms: +percentile(runs, .95).toFixed(3), worst_ms: +Math.max(...runs).toFixed(3), bytes: last.bytes };
}
async function waitFor(child) {
  for (let attempts = 0; attempts < 300; attempts += 1) {
    try { if ((await fetch(`${root}/health`)).ok) return; } catch {}
    if (child.exitCode !== null) throw new Error(`server exited ${child.exitCode}`);
    await new Promise(resolve => setTimeout(resolve, 25));
  }
  throw new Error('server startup timed out');
}
async function start(dataDir) {
  const child = spawn(binary, [], { env: { ...process.env, ALKAHILI_DATA_DIR: dataDir }, stdio: 'ignore' });
  await waitFor(child); return child;
}
async function stop(child) {
  if (child.exitCode !== null) return;
  child.kill('SIGINT'); await Promise.race([once(child, 'exit'), new Promise(resolve => setTimeout(resolve, 2000))]);
  if (child.exitCode === null) child.kill('SIGKILL');
}
function seed(db, size, userId) {
  const sql = `
PRAGMA foreign_keys=ON;
BEGIN;
INSERT INTO workers(id,full_name,is_active,created_at,updated_at) VALUES('page-worker','عامل الصفحات',1,'2025-01-01T00:00:00.000Z','2025-01-01T00:00:00.000Z');
INSERT INTO showrooms(id,name,is_active,created_at,updated_at) VALUES('page-showroom','معرض الصفحات',1,'2025-01-01T00:00:00.000Z','2025-01-01T00:00:00.000Z');
INSERT INTO payroll_employees(id,full_name,is_active,created_at,updated_at) VALUES('page-employee','موظف الصفحات',1,'2025-01-01T00:00:00.000Z','2025-01-01T00:00:00.000Z');
INSERT INTO payroll_salary_rates(employee_id,effective_month,salary_milli,set_by,created_at,updated_at) VALUES('page-employee','2026-09',1000000000,'${userId}','2026-09-01T00:00:00.000Z','2026-09-01T00:00:00.000Z');
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${size})
INSERT INTO wash_operations(id,vehicle_make,vehicle_model,price_milli,worker_id,payment_type,showroom_id,showroom_payment_method,occurred_at,commission_bps,commission_milli,business_share_milli,created_by,client_request_id,status,is_paid,created_at,updated_at)
SELECT printf('page-wash-%08d',x),'Toyota','Pagination',50000,'page-worker','showroom','page-showroom','cash',printf('2026-09-07T12:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000),5000,25000,25000,'${userId}',printf('page-request-%08d',x),'posted',0,printf('2026-09-07T12:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000),printf('2026-09-07T12:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${size})
INSERT INTO expenses(id,description,category,payment_method,amount_milli,occurred_at,notes,allocation_type,business_bps,workers_bps,business_amount_milli,workers_amount_milli,created_by,created_at)
SELECT printf('page-expense-%08d',x),printf('مصروف %d',x),'اختبار','cash',3000,printf('2026-09-07T13:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000),'pagination','business',10000,0,3000,0,'${userId}',printf('2026-09-07T13:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${size})
INSERT INTO showroom_payments(id,showroom_id,amount_milli,paid_at,notes,created_by,created_at)
SELECT printf('page-payment-%08d',x),'page-showroom',1000,printf('2026-09-07T14:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000),'pagination','${userId}',printf('2026-09-07T14:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${size})
INSERT INTO salary_withdrawals(id,employee_id,amount_milli,withdrawn_at,notes,created_by,created_at,updated_at)
SELECT printf('page-salary-w-%08d',x),'page-employee',1000,printf('2026-09-07T15:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000),'pagination','${userId}',printf('2026-09-07T15:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000),printf('2026-09-07T15:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${size})
INSERT INTO salary_deductions(id,employee_id,amount_milli,deduction_month,deducted_at,notes,created_by,created_at,updated_at)
SELECT printf('page-salary-d-%08d',x),'page-employee',1000,'2026-09',printf('2026-09-07T16:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000),'pagination','${userId}',printf('2026-09-07T16:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000),printf('2026-09-07T16:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${size})
INSERT INTO worker_withdrawal_returns(id,worker_id,transaction_type,amount_milli,occurred_at,notes,created_by,created_at)
SELECT printf('page-movement-%08d',x),'page-worker','withdrawal',1000,printf('2026-09-07T17:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000),'pagination','${userId}',printf('2026-09-07T17:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000) FROM n;
COMMIT;`;
  const result = spawnSync('sqlite3', [db], { input: sql, encoding: 'utf8', maxBuffer: 1024 * 1024 * 10 });
  if (result.status) throw new Error(result.stderr);
}

const paths = {
  expenses: ['/expenses?date=2026-09-07&limit=100', value => value.items.length],
  showroom_payments: ['/showroom-payments?date=2026-09-07&limit=100', value => value.items.length],
  salary_withdrawals: ['/payroll/withdrawals?date=2026-09-07&limit=100', value => value.items.length],
  salary_deductions: ['/payroll/deductions?date=2026-09-07&limit=100', value => value.items.length],
  worker_washes: ['/workers/page-worker?date=2026-09-07&limit=100', value => value.history.length],
  showroom_washes: ['/showrooms/page-showroom?date=2026-09-07&limit=100', value => value.history.length],
  showroom_financial_payments: ['/showrooms/page-showroom/financial?date=2026-09-07&limit=100', value => value.payments.length],
  worker_movements: ['/workers/page-worker/withdrawals-returns?date=2026-09-07&limit=100', value => value.transactions.length],
  showroom_debt_detail: ['/showroom-debts/page-showroom?date=2026-09-07&operationsLimit=100&paymentsLimit=100', value => value.operations.length + value.payments.length],
};
const report = { generated_at: new Date().toISOString(), binary, sizes: {} };
for (const size of sizes) {
  const dataDir = `/private/tmp/alkaheli-pagination-${process.pid}-${size}`;
  rmSync(dataDir, { recursive: true, force: true }); mkdirSync(dataDir, { recursive: true });
  let child = await start(dataDir);
  const setup = await fetch(`${root}/setup/initial-manager`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ fullName: 'Pagination Benchmark', username: 'pagebench', password: 'Pagination-Benchmark-2026' }) }).then(value => value.json());
  await stop(child); seed(`${dataDir}/carwash.db`, size, setup.data.user.id); child = await start(dataDir);
  const endpoints = {};
  for (const [name, [path, count]] of Object.entries(paths)) {
    const stats = await measured(path, setup.data.token); const response = await raw(path, setup.data.token);
    endpoints[name] = { ...stats, rows: count(response.body), has_more: response.body.hasMore ?? response.body.historyHasMore ?? response.body.paymentsHasMore ?? response.body.operationsHasMore ?? null };
  }
  report.sizes[size] = { endpoints };
  writeFileSync(output, JSON.stringify(report, null, 2));
  await stop(child);
  process.stdout.write(`${size}: expenses ${endpoints.expenses.rows} rows/${endpoints.expenses.bytes} bytes; debt ${endpoints.showroom_debt_detail.rows} rows/${endpoints.showroom_debt_detail.bytes} bytes\n`);
}
writeFileSync(output, JSON.stringify(report, null, 2));
console.log(output);
