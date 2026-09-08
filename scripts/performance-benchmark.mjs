import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { once } from 'node:events';
import { performance } from 'node:perf_hooks';

const binary = process.argv[2];
const sizes = (process.argv[3] ?? '300,1000,10000,25000,50000,100000,250000')
  .split(',').map(Number).filter(Number.isFinite);
const output = process.argv[4] ?? '/private/tmp/alkaheli-perf-baseline.json';
if (!binary) throw new Error('usage: node benchmark.mjs BINARY [sizes] [output]');

const root = 'http://127.0.0.1:8787/api';
const percentile = (values, p) => values.slice().sort((a,b)=>a-b)[Math.min(values.length-1, Math.ceil(values.length*p)-1)];
const summary = (cold, warm) => ({
  cold_ms: +cold.toFixed(3),
  median_ms: +percentile(warm, .5).toFixed(3),
  p95_ms: +percentile(warm, .95).toFixed(3),
  worst_ms: +Math.max(...warm).toFixed(3),
});

async function request(path, token, options = {}) {
  const headers = new Headers(options.headers);
  if (token) headers.set('Authorization', `Bearer ${token}`);
  if (options.body && !headers.has('Content-Type')) headers.set('Content-Type', 'application/json');
  const start = performance.now();
  const response = await fetch(`${root}${path}`, {...options, headers});
  const bytes = new Uint8Array(await response.arrayBuffer());
  const elapsed = performance.now() - start;
  if (!response.ok && response.status !== 404) throw new Error(`${path}: ${response.status} ${new TextDecoder().decode(bytes)}`);
  return { elapsed, bytes, status: response.status };
}

async function waitForServer(child) {
  const start = performance.now();
  for (;;) {
    if (child.exitCode !== null) throw new Error(`server exited ${child.exitCode}`);
    try {
      const response = await fetch(`${root}/health`);
      if (response.ok) return performance.now() - start;
    } catch {}
    if (performance.now() - start > 15000) throw new Error('server readiness timeout');
    await new Promise(resolve => setTimeout(resolve, 5));
  }
}

async function startServer(dataDir) {
  const child = spawn(binary, [], {env: {...process.env, ALKAHILI_DATA_DIR: dataDir}, stdio: ['ignore','pipe','pipe']});
  let errors = '';
  child.stderr.on('data', chunk => { errors += chunk; });
  const readyMs = await waitForServer(child).catch(error => { throw new Error(`${error.message}\n${errors}`); });
  return {child, readyMs};
}

async function stopServer(child) {
  if (child.exitCode !== null) return;
  child.kill('SIGINT');
  await Promise.race([once(child, 'exit'), new Promise(resolve => setTimeout(resolve, 2000))]);
  if (child.exitCode === null) child.kill('SIGKILL');
}

function seedSql(size, userId) {
  const workers = Math.min(300, Math.max(20, Math.ceil(Math.sqrt(size))));
  const showrooms = Math.min(100, Math.max(10, Math.ceil(Math.sqrt(size) / 2)));
  const expenses = Math.max(20, Math.floor(size / 20));
  const audits = Math.max(50, Math.floor(size / 5));
  const payments = Math.max(10, Math.floor(size / 100));
  const payroll = Math.min(150, Math.max(20, Math.ceil(Math.sqrt(size) / 2)));
  return {workers, showrooms, expenses, audits, payments, payroll, sql: `
PRAGMA foreign_keys=ON;
PRAGMA journal_mode=WAL;
BEGIN;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${workers})
INSERT INTO workers(id,full_name,phone,notes,is_active,created_at,updated_at)
SELECT printf('worker-%06d',x),printf('عامل اختبار %06d',x),printf('091%07d',x),'linked benchmark worker',1,'2025-01-01T00:00:00.000Z','2025-01-01T00:00:00.000Z' FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${showrooms})
INSERT INTO showrooms(id,name,contact_name,is_active,created_at,updated_at)
SELECT printf('showroom-%06d',x),printf('معرض اختبار %06d',x),printf('مسؤول %06d',x),1,'2025-01-01T00:00:00.000Z','2025-01-01T00:00:00.000Z' FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${size})
INSERT INTO wash_operations(id,vehicle_make,vehicle_model,manufacture_year,license_plate,car_color,wash_type,price_milli,worker_id,payment_type,showroom_id,showroom_payment_method,occurred_at,commission_bps,commission_milli,business_share_milli,created_by,client_request_id,status,is_paid,paid_at,paid_by,created_at,updated_at)
SELECT printf('wash-%09d',x),'Toyota',printf('Model-%d',x%12),2010+(x%17),printf('TEST-%07d',x),'أبيض','غسيل كامل',50000+(x%20)*5000,printf('worker-%06d',1+(x%${workers})),CASE WHEN x%5=0 THEN 'showroom' ELSE 'cash' END,CASE WHEN x%5=0 THEN printf('showroom-%06d',1+(x%${showrooms})) ELSE NULL END,CASE WHEN x%5=0 THEN CASE WHEN x%2=0 THEN 'bank' ELSE 'cash' END ELSE NULL END,strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365),printf('+%d minutes',x%1440)),5000,(50000+(x%20)*5000)/2,(50000+(x%20)*5000)-((50000+(x%20)*5000)/2),'${userId}',printf('request-%09d',x),CASE WHEN x%97=0 THEN 'voided' ELSE 'posted' END,CASE WHEN x%3=0 THEN 1 ELSE 0 END,CASE WHEN x%3=0 THEN strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365),printf('+%d minutes',x%1440+5)) ELSE NULL END,CASE WHEN x%3=0 THEN '${userId}' ELSE NULL END,strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365),printf('+%d minutes',x%1440)),strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365),printf('+%d minutes',x%1440)) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${Math.max(1, Math.floor(size/1000))})
INSERT INTO overnight_cars(id,wash_id,marked_by,marked_at)
SELECT printf('overnight-%09d',x),printf('wash-%09d',x*1000-1),'${userId}','2026-09-07T22:00:00.000Z' FROM n WHERE (x*1000-1)%3<>0 AND (x*1000-1)%97<>0;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${expenses})
INSERT INTO expenses(id,description,category,payment_method,amount_milli,occurred_at,notes,allocation_type,business_bps,workers_bps,business_amount_milli,workers_amount_milli,created_by,created_at)
SELECT printf('expense-%09d',x),printf('مصروف اختبار %d',x),'تشغيل',CASE WHEN x%4=0 THEN 'bank' ELSE 'cash' END,10000+(x%50)*1000,strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365)),NULL,'shared',5000,5000,(10000+(x%50)*1000)/2,(10000+(x%50)*1000)-((10000+(x%50)*1000)/2),'${userId}',strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365)) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${expenses})
INSERT INTO expense_allocations(id,expense_id,worker_id,amount_milli,allocation_order,created_at)
SELECT printf('allocation-%09d',x),printf('expense-%09d',x),printf('worker-%06d',1+(x%${workers})),(10000+(x%50)*1000)-((10000+(x%50)*1000)/2),1,strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365)) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${payments})
INSERT INTO showroom_payments(id,showroom_id,amount_milli,paid_at,notes,created_by,created_at)
SELECT printf('showroom-payment-%09d',x),printf('showroom-%06d',1+(x%${showrooms})),25000+(x%10)*1000,strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365)),'benchmark','${userId}',strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365)) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${payroll})
INSERT INTO payroll_employees(id,full_name,is_active,created_at,updated_at)
SELECT printf('employee-%06d',x),printf('موظف راتب %06d',x),1,'2025-01-01T00:00:00.000Z','2025-01-01T00:00:00.000Z' FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${payroll})
INSERT INTO payroll_salary_rates(employee_id,effective_month,salary_milli,set_by,created_at,updated_at)
SELECT printf('employee-%06d',x),'2025-01',1000000+(x%10)*100000,'${userId}','2025-01-01T00:00:00.000Z','2025-01-01T00:00:00.000Z' FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${Math.max(payroll, Math.floor(size/1000))})
INSERT INTO salary_withdrawals(id,employee_id,amount_milli,withdrawn_at,notes,created_by,created_at,updated_at)
SELECT printf('salary-withdrawal-%09d',x),printf('employee-%06d',1+(x%${payroll})),50000,strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365)),'benchmark','${userId}',strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365)),strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365)) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${Math.max(payroll, Math.floor(size/1000))})
INSERT INTO salary_deductions(id,employee_id,amount_milli,deduction_month,notes,created_by,created_at,deducted_at,updated_at)
SELECT printf('salary-deduction-%09d',x),printf('employee-%06d',1+(x%${payroll})),25000,strftime('%Y-%m','2025-09-08',printf('+%d days',x%365)),'benchmark','${userId}',strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365)),strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365)),strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365)) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${audits})
INSERT INTO audit_logs(id,user_id,action,entity_type,entity_id,description,created_at)
SELECT printf('audit-%09d',x),'${userId}','BENCHMARK','wash',printf('wash-%09d',1+(x%${size})),'linked benchmark audit',strftime('%Y-%m-%dT%H:%M:%fZ','2025-09-08',printf('+%d days',x%365),printf('+%d minutes',x%1440)) FROM n;
COMMIT;
`};
}

function populate(dbPath, size, userId) {
  const seed = seedSql(size, userId);
  const result = spawnSync('sqlite3', [dbPath], {input: seed.sql, encoding:'utf8', maxBuffer: 50*1024*1024});
  if (result.status !== 0) throw new Error(`sqlite seed failed: ${result.stderr}`);
  return seed;
}

async function benchmarkPath(path, token, iterations = 7) {
  const cold = (await request(path, token)).elapsed;
  const warm = [];
  let bytes = 0;
  for (let i=0;i<iterations;i++) {
    const result = await request(path, token);
    warm.push(result.elapsed); bytes = result.bytes.length;
  }
  return {...summary(cold, warm), bytes};
}

async function concurrency(paths, token) {
  const start = performance.now();
  const results = await Promise.all(paths.map(path => request(path, token)));
  return {
    wall_ms: +(performance.now()-start).toFixed(3),
    median_ms: +percentile(results.map(r=>r.elapsed),.5).toFixed(3),
    p95_ms: +percentile(results.map(r=>r.elapsed),.95).toFixed(3),
    worst_ms: +Math.max(...results.map(r=>r.elapsed)).toFixed(3),
  };
}

const endpointPaths = {
  me:'/auth/me',
  dashboard:'/dashboard?date=2026-09-07',
  washes:'/washes?date=2026-09-07&limit=300',
  paid_cars:'/paid-cars?date=2026-09-07',
  overnight:'/overnight-cars?date=2026-09-07',
  workers_day:'/workers?date=2026-09-07&include_financials=true',
  workers_full:'/workers?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z&include_financials=true',
  worker_detail:'/workers/worker-000001?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z',
  worker_financial:'/workers/worker-000001/financial?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z',
  showrooms_day:'/showrooms?date=2026-09-07&include_financials=true',
  showrooms_full:'/showrooms?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z&include_financials=true',
  showroom_debts:'/showroom-debts?date=2026-09-07',
  showroom_detail:'/showrooms/showroom-000001?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z',
  showroom_financial:'/showrooms/showroom-000001/financial?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z',
  payroll:'/payroll?month=2026-09',
  salary_withdrawals:'/payroll/withdrawals?month=2026-09',
  salary_deductions:'/payroll/deductions?month=2026-09',
  showroom_payments:'/showroom-payments?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z&limit=300',
  expenses:'/expenses?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z',
  finance:'/finance/overview?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z',
  operational_report:'/reports/operational?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z',
  financial_report:'/reports/financial?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z',
  settings:'/settings', users:'/users', roles:'/roles',
  audit:'/audit-logs?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z&limit=500',
  backups:'/backups?from=2025-09-08T00:00:00Z&to=2026-09-07T23:59:59Z',
  dashboard_order:'/preferences/dashboard-card-order',
  report_order:'/preferences/financial-report-card-order',
};

const report = {generated_at:new Date().toISOString(), binary, sizes:{}};
for (const size of sizes) {
  const dataDir = `/private/tmp/alkaheli-perf-baseline-${size}`;
  rmSync(dataDir, {recursive:true, force:true}); mkdirSync(dataDir, {recursive:true});
  let server = await startServer(dataDir);
  const setupResponse = await fetch(`${root}/setup/initial-manager`, {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({fullName:'Performance Audit',username:'perfaudit',password:'Performance-Audit-2026'})});
  const setup = await setupResponse.json();
  const token = setup.data.token; const userId = setup.data.user.id;
  await stopServer(server.child);
  const shape = populate(`${dataDir}/carwash.db`, size, userId);
  server = await startServer(dataDir);
  const result = {shape:{size,...shape,sql:undefined}, startup_ms:+server.readyMs.toFixed(3), endpoints:{}, concurrency:{}};
  for (const [name,path] of Object.entries(endpointPaths)) result.endpoints[name] = await benchmarkPath(path, token, size>=100000?5:7);
  if ([300,10000,25000,50000,100000].includes(size)) {
    for (const name of ['dashboard','workers_full','settings','showrooms_full','finance','operational_report','financial_report','backups']) {
      result.concurrency[`${name}_10`] = await concurrency(Array(10).fill(endpointPaths[name]),token);
      result.concurrency[`${name}_25`] = await concurrency(Array(25).fill(endpointPaths[name]),token);
    }
    result.concurrency.mixed_10 = await concurrency([endpointPaths.dashboard,endpointPaths.settings,endpointPaths.workers_full,endpointPaths.settings,endpointPaths.operational_report,endpointPaths.dashboard,endpointPaths.financial_report,endpointPaths.settings,endpointPaths.showrooms_full,endpointPaths.dashboard],token);
    result.concurrency.mixed_25 = await concurrency(Array.from({length:25},(_,i)=>[endpointPaths.dashboard,endpointPaths.settings,endpointPaths.workers_full,endpointPaths.operational_report,endpointPaths.financial_report][i%5]),token);
  }
  report.sizes[size] = result;
  writeFileSync(output, JSON.stringify(report,null,2));
  await stopServer(server.child);
  process.stdout.write(`${size}: dashboard ${result.endpoints.dashboard.median_ms}ms workers-full ${result.endpoints.workers_full.median_ms}ms financial-report ${result.endpoints.financial_report.median_ms}ms\n`);
}
writeFileSync(output, JSON.stringify(report,null,2));
console.log(output);
