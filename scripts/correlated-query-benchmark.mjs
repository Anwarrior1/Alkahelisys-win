import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { once } from 'node:events';
import { performance } from 'node:perf_hooks';

const [binary, mode, output] = process.argv.slice(2);
if (!binary || !mode || !output) throw new Error('usage: node correlated-query-benchmark.mjs BINARY LABEL OUTPUT');
const root = 'http://127.0.0.1:8787/api';
const sizes = [100, 1000, 5000];
const percentile = (values, p) => values.slice().sort((a, b) => a - b)[Math.min(values.length - 1, Math.ceil(values.length * p) - 1)];

async function start(dataDir) {
  const child = spawn(binary, [], { env: { ...process.env, ALKAHILI_DATA_DIR: dataDir }, stdio: ['ignore', 'pipe', 'pipe'] });
  let errors = '';
  child.stderr.on('data', (chunk) => { errors += chunk; });
  for (let attempt = 0; attempt < 600; attempt += 1) {
    try { if ((await fetch(`${root}/health`)).ok) return child; } catch {}
    if (child.exitCode !== null) throw new Error(`server exited ${child.exitCode}: ${errors}`);
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(`server startup timed out: ${errors}`);
}

async function stop(child) {
  if (child.exitCode !== null) return;
  child.kill('SIGINT');
  await Promise.race([once(child, 'exit'), new Promise((resolve) => setTimeout(resolve, 2000))]);
  if (child.exitCode === null) child.kill('SIGKILL');
}

function seed(db, managerId, count) {
  const sql = `
PRAGMA foreign_keys=ON;
PRAGMA journal_mode=WAL;
BEGIN;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${count})
INSERT INTO workers(id,full_name,is_active,created_at,updated_at)
SELECT printf('p4-worker-%06d',x),printf('عامل %06d',x),1,'2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z' FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${count}), t(y) AS (SELECT 1 UNION ALL SELECT y+1 FROM t WHERE y<5)
INSERT INTO expenses(id,description,category,payment_method,amount_milli,occurred_at,allocation_type,business_bps,workers_bps,business_amount_milli,workers_amount_milli,created_by,created_at)
SELECT printf('p4-expense-%06d-%02d',x,y),'Benchmark','اختبار','cash',1000,'2026-09-07T12:00:00.000Z','workers',0,10000,0,1000,'${managerId}','2026-09-07T12:00:00.000Z' FROM n,t;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${count}), t(y) AS (SELECT 1 UNION ALL SELECT y+1 FROM t WHERE y<5)
INSERT INTO expense_allocations(id,expense_id,worker_id,amount_milli,allocation_order,created_at)
SELECT printf('p4-allocation-%06d-%02d',x,y),printf('p4-expense-%06d-%02d',x,y),printf('p4-worker-%06d',x),1000,0,'2026-09-07T12:00:00.000Z' FROM n,t;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${count})
INSERT INTO showrooms(id,name,is_active,created_at,updated_at)
SELECT printf('p4-showroom-%06d',x),printf('معرض %06d',x),1,'2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z' FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${count}), t(y) AS (SELECT 1 UNION ALL SELECT y+1 FROM t WHERE y<10)
INSERT INTO wash_operations(id,vehicle_make,vehicle_model,price_milli,worker_id,payment_type,showroom_id,showroom_payment_method,occurred_at,commission_bps,commission_milli,business_share_milli,created_by,client_request_id,status,is_paid,created_at,updated_at)
SELECT printf('p4-wash-%06d-%02d',x,y),'Benchmark','Group',50000,printf('p4-worker-%06d',x),'showroom',printf('p4-showroom-%06d',x),'cash','2026-09-07T12:00:00.000Z',5000,25000,25000,'${managerId}',printf('p4-request-%06d-%02d',x,y),'posted',0,'2026-09-07T12:00:00.000Z','2026-09-07T12:00:00.000Z' FROM n,t;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${count}), t(y) AS (SELECT 1 UNION ALL SELECT y+1 FROM t WHERE y<5)
INSERT INTO showroom_payments(id,showroom_id,amount_milli,paid_at,created_by,created_at)
SELECT printf('p4-payment-%06d-%02d',x,y),printf('p4-showroom-%06d',x),1000,'2026-09-07T13:00:00.000Z','${managerId}','2026-09-07T13:00:00.000Z' FROM n,t;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${count})
INSERT INTO payroll_employees(id,full_name,is_active,created_at,updated_at)
SELECT printf('p4-employee-%06d',x),printf('موظف %06d',x),1,'2025-01-01T00:00:00.000Z','2025-01-01T00:00:00.000Z' FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${count}), m(y) AS (SELECT 0 UNION ALL SELECT y+1 FROM m WHERE y<11)
INSERT INTO payroll_salary_rates(employee_id,effective_month,salary_milli,set_by,created_at,updated_at)
SELECT printf('p4-employee-%06d',x),strftime('%Y-%m',date('2025-10-01',printf('+%d months',y))),1000000+y,'${managerId}','2025-01-01T00:00:00.000Z','2025-01-01T00:00:00.000Z' FROM n,m;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${count}), t(y) AS (SELECT 1 UNION ALL SELECT y+1 FROM t WHERE y<3)
INSERT INTO salary_withdrawals(id,employee_id,amount_milli,withdrawn_at,created_by,created_at,updated_at)
SELECT printf('p4-withdrawal-%06d-%02d',x,y),printf('p4-employee-%06d',x),1000,'2026-09-07T14:00:00.000Z','${managerId}','2026-09-07T14:00:00.000Z','2026-09-07T14:00:00.000Z' FROM n,t;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${count}), t(y) AS (SELECT 1 UNION ALL SELECT y+1 FROM t WHERE y<3)
INSERT INTO salary_deductions(id,employee_id,amount_milli,deduction_month,deducted_at,created_by,created_at,updated_at)
SELECT printf('p4-deduction-%06d-%02d',x,y),printf('p4-employee-%06d',x),1000,'2026-09','2026-09-07T15:00:00.000Z','${managerId}','2026-09-07T15:00:00.000Z','2026-09-07T15:00:00.000Z' FROM n,t;
WITH RECURSIVE manager AS (SELECT password_hash FROM users WHERE id='${managerId}'),
n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${count})
INSERT INTO users(id,full_name,username_norm,password_hash,is_active,created_at,updated_at)
SELECT printf('p4-user-%06d',x),printf('مستخدم %06d',x),printf('p4.user.%06d',x),manager.password_hash,1,'2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z' FROM n,manager;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${count})
INSERT INTO user_roles(user_id,role_id) SELECT printf('p4-user-%06d',x),(SELECT id FROM roles WHERE code='employee') FROM n;
WITH RECURSIVE n(x) AS (SELECT 5 UNION ALL SELECT x+5 FROM n WHERE x+5<=${count})
INSERT INTO user_permission_profiles(user_id,updated_at) SELECT printf('p4-user-%06d',x),'2026-01-01T00:00:00.000Z' FROM n;
WITH RECURSIVE n(x) AS (SELECT 5 UNION ALL SELECT x+5 FROM n WHERE x+5<=${count})
INSERT INTO user_permissions(user_id,permission_id) SELECT printf('p4-user-%06d',x),(SELECT id FROM permissions ORDER BY code LIMIT 1) FROM n;
COMMIT;
ANALYZE;`;
  const result = spawnSync('sqlite3', [db], { input: sql, encoding: 'utf8', maxBuffer: 20 * 1024 * 1024 });
  if (result.status !== 0) throw new Error(result.stderr);
}

async function request(path, token) {
  const started = performance.now();
  const response = await fetch(`${root}${path}`, { headers: { Authorization: `Bearer ${token}` } });
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (!response.ok) throw new Error(`${path}: ${response.status} ${new TextDecoder().decode(bytes)}`);
  return { ms: performance.now() - started, bytes: bytes.length, data: JSON.parse(new TextDecoder().decode(bytes)).data };
}

async function measure(path, token) {
  const samples = [];
  let last;
  for (let run = 0; run < 8; run += 1) {
    last = await request(path, token);
    if (run > 0) samples.push(last.ms);
  }
  const rows = Array.isArray(last.data.items) ? last.data.items.length : last.data.employees.length;
  const digestData = path === '/users'
    ? { ...last.data, items: last.data.items.filter((item) => item.username !== 'phase4bench') }
    : last.data;
  return {
    rows,
    bytes: last.bytes,
    median_ms: +percentile(samples, 0.5).toFixed(3),
    p95_ms: +percentile(samples, 0.95).toFixed(3),
    worst_ms: +Math.max(...samples).toFixed(3),
    digest: createHash('sha256').update(JSON.stringify(digestData)).digest('hex'),
  };
}

const endpoints = {
  workers: '/workers?date=2026-09-07',
  showrooms: '/showrooms?date=2026-09-07&includeFinancials=true',
  payroll: '/payroll?month=2026-09',
  users: '/users',
};
const report = { generated_at: new Date().toISOString(), mode, sizes: {} };
for (const count of sizes) {
  const dataDir = `/private/tmp/alkaheli-correlated-${mode}-${count}-${process.pid}`;
  rmSync(dataDir, { recursive: true, force: true });
  mkdirSync(dataDir, { recursive: true });
  let child = await start(dataDir);
  const setup = await fetch(`${root}/setup/initial-manager`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ fullName: 'Phase 4 Benchmark', username: 'phase4bench', password: 'Phase4-Benchmark-2026' }),
  }).then((response) => response.json());
  await stop(child);
  seed(`${dataDir}/carwash.db`, setup.data.user.id, count);
  child = await start(dataDir);
  report.sizes[count] = {};
  for (const [name, path] of Object.entries(endpoints)) report.sizes[count][name] = await measure(path, setup.data.token);
  await stop(child);
}
writeFileSync(output, JSON.stringify(report, null, 2));
console.log(output);
