import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { once } from 'node:events';
import { performance } from 'node:perf_hooks';

const [binary, mode = 'offset', output = `/private/tmp/alkaheli-keyset-${mode}.json`, sizeValue = '120000'] = process.argv.slice(2);
const size = Number(sizeValue);
if (!binary || !['offset', 'cursor'].includes(mode) || !Number.isFinite(size)) {
  throw new Error('usage: node keyset-benchmark.mjs BINARY offset|cursor [output] [size]');
}

const root = 'http://127.0.0.1:8787/api';
const dataDir = `/private/tmp/alkaheli-keyset-${mode}-${process.pid}`;
const depths = [0, 10_000, 50_000, 100_000].filter((depth) => depth < size);
const percentile = (values, p) => values.slice().sort((a, b) => a - b)[Math.min(values.length - 1, Math.ceil(values.length * p) - 1)];

async function request(path, token) {
  const started = performance.now();
  const response = await fetch(`${root}${path}`, { headers: { Authorization: `Bearer ${token}` } });
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (!response.ok) throw new Error(`${path}: ${response.status} ${new TextDecoder().decode(bytes)}`);
  return { ms: performance.now() - started, bytes: bytes.length, body: JSON.parse(new TextDecoder().decode(bytes)).data };
}

async function start() {
  const child = spawn(binary, [], { env: { ...process.env, ALKAHILI_DATA_DIR: dataDir }, stdio: ['ignore', 'pipe', 'pipe'] });
  let errors = '';
  child.stderr.on('data', (chunk) => { errors += chunk; });
  for (let attempts = 0; attempts < 600; attempts += 1) {
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

function seed(db, userId) {
  const sql = `
PRAGMA foreign_keys=ON;
BEGIN;
INSERT INTO workers(id,full_name,is_active,created_at,updated_at) VALUES('keyset-worker','عامل المؤشر',1,'2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z');
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${size})
INSERT INTO wash_operations(id,vehicle_make,vehicle_model,price_milli,worker_id,payment_type,occurred_at,commission_bps,commission_milli,business_share_milli,created_by,client_request_id,status,is_paid,created_at,updated_at)
SELECT printf('keyset-wash-%09d',x),'Benchmark','Keyset',50000,'keyset-worker','cash',printf('2026-09-07T00:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000),5000,25000,25000,'${userId}',printf('keyset-request-%09d',x),'posted',0,printf('2026-09-07T00:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000),printf('2026-09-07T00:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${size})
INSERT INTO expenses(id,description,category,payment_method,amount_milli,occurred_at,allocation_type,business_bps,workers_bps,business_amount_milli,workers_amount_milli,created_by,created_at)
SELECT printf('keyset-expense-%09d',x),'Benchmark','اختبار','cash',1000,printf('2026-09-07T00:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000),'business',10000,0,1000,0,'${userId}',printf('2026-09-07T00:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000) FROM n;
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<${size})
INSERT INTO audit_logs(id,user_id,action,entity_type,entity_id,description,created_at)
SELECT printf('keyset-audit-%09d',x),'${userId}','BENCHMARK','wash',printf('keyset-wash-%09d',x),'keyset benchmark',printf('2026-09-07T00:%02d:%02d.%03dZ',(x/60000)%60,(x/1000)%60,x%1000) FROM n;
COMMIT;`;
  const result = spawnSync('sqlite3', [db], { input: sql, encoding: 'utf8', maxBuffer: 20 * 1024 * 1024 });
  if (result.status !== 0) throw new Error(result.stderr);
}

const endpoints = {
  washes: { path: '/washes?date=2026-09-07&limit=100', table: 'wash_operations', timestamp: 'occurred_at', id: 'id', cursorField: 'nextCursor' },
  expenses: { path: '/expenses?date=2026-09-07&limit=100', table: 'expenses', timestamp: 'occurred_at', id: 'id', cursorField: 'nextCursor' },
  audit: { path: '/audit-logs?date=2026-09-07&limit=100', table: 'audit_logs', timestamp: 'created_at', id: 'id', cursorField: 'nextCursor' },
};

function boundary(db, config, depth) {
  if (depth === 0) return null;
  const sql = `SELECT json_object('timestamp',${config.timestamp},'id',${config.id}) FROM ${config.table} ORDER BY ${config.timestamp} DESC,${config.id} DESC LIMIT 1 OFFSET ${depth - 1};`;
  const result = spawnSync('sqlite3', [db, sql], { encoding: 'utf8' });
  if (result.status !== 0) throw new Error(result.stderr);
  return JSON.parse(result.stdout.trim());
}

async function measured(path, token) {
  const samples = [];
  let last;
  for (let run = 0; run < 8; run += 1) {
    last = await request(path, token);
    if (run > 0) samples.push(last.ms);
  }
  return {
    rows: last.body.items.length,
    bytes: last.bytes,
    median_ms: +percentile(samples, 0.5).toFixed(3),
    p95_ms: +percentile(samples, 0.95).toFixed(3),
    worst_ms: +Math.max(...samples).toFixed(3),
  };
}

rmSync(dataDir, { recursive: true, force: true });
mkdirSync(dataDir, { recursive: true });
let child = await start();
const setup = await fetch(`${root}/setup/initial-manager`, {
  method: 'POST', headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ fullName: 'Keyset Benchmark', username: 'keysetbench', password: 'Keyset-Benchmark-2026' }),
}).then((response) => response.json());
await stop(child);
const db = `${dataDir}/carwash.db`;
seed(db, setup.data.user.id);
child = await start();

const report = { generated_at: new Date().toISOString(), mode, size, depths, endpoints: {} };
for (const [name, config] of Object.entries(endpoints)) {
  report.endpoints[name] = {};
  const first = mode === 'cursor' ? await request(config.path, setup.data.token) : null;
  for (const depth of depths) {
    let path = config.path;
    if (mode === 'offset') {
      path += `&offset=${depth}`;
    } else if (depth > 0) {
      const key = boundary(db, config, depth);
      const template = JSON.parse(first.body[config.cursorField]);
      const cursor = JSON.stringify({ ...template, timestamp: key.timestamp, id: key.id });
      path += `&cursor=${encodeURIComponent(cursor)}`;
    }
    report.endpoints[name][depth] = await measured(path, setup.data.token);
  }
}
await stop(child);
writeFileSync(output, JSON.stringify(report, null, 2));
console.log(output);
