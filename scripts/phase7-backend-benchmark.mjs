import { createHash } from 'node:crypto';
import { copyFileSync, mkdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { performance } from 'node:perf_hooks';

const [binary, templateDb, label = 'run', output = `/private/tmp/alkaheli-phase7-${label}.json`] = process.argv.slice(2);
if (!binary || !templateDb) {
  throw new Error('usage: node phase7-backend-benchmark.mjs BINARY TEMPLATE_DB [label] [output]');
}

const root = 'http://127.0.0.1:8787/api';
const dataDir = `/private/tmp/alkaheli-phase7-${label}-${process.pid}`;
const dbPath = `${dataDir}/carwash.db`;
const token = `phase7-${label}-token`;
const tokenHash = createHash('sha256').update(token).digest('hex');
const sqlQuote = (value) => `'${String(value).replaceAll("'", "''")}'`;
const sqlite = (sql) => {
  const result = spawnSync('sqlite3', [dbPath, sql], { encoding: 'utf8' });
  if (result.status !== 0) throw new Error(result.stderr || `sqlite exited ${result.status}`);
  return result.stdout.trim();
};
const percentile = (values, p) => values.slice().sort((a, b) => a - b)[Math.min(values.length - 1, Math.ceil(values.length * p) - 1)];
const summarize = (values) => ({
  count: values.length,
  median_ms: +percentile(values, 0.5).toFixed(3),
  p95_ms: +percentile(values, 0.95).toFixed(3),
  worst_ms: +Math.max(...values).toFixed(3),
});
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function request(path, options = {}) {
  const headers = new Headers(options.headers);
  if (options.auth !== false) headers.set('Authorization', `Bearer ${token}`);
  if (options.body && !headers.has('Content-Type')) headers.set('Content-Type', 'application/json');
  const started = performance.now();
  const response = await fetch(`${root}${path}`, { ...options, headers });
  const payload = await response.json().catch(() => ({}));
  const elapsed = performance.now() - started;
  if (!response.ok) throw new Error(`${path}: ${response.status} ${JSON.stringify(payload)}`);
  return { elapsed, data: payload.data };
}

async function waitForServer(child) {
  const started = performance.now();
  for (;;) {
    if (child.exitCode !== null) throw new Error(`server exited ${child.exitCode}`);
    try {
      if ((await fetch(`${root}/health`)).ok) return performance.now() - started;
    } catch {}
    if (performance.now() - started > 15000) throw new Error('server readiness timeout');
    await sleep(5);
  }
}

async function stopServer(child) {
  if (child.exitCode !== null) return;
  child.kill('SIGINT');
  await Promise.race([once(child, 'exit'), sleep(2000)]);
  if (child.exitCode === null) child.kill('SIGKILL');
}

async function repeat(count, operation) {
  const values = [];
  for (let index = 0; index < count; index += 1) values.push((await operation(index)).elapsed);
  return summarize(values);
}

rmSync(dataDir, { recursive: true, force: true });
mkdirSync(dataDir, { recursive: true });
copyFileSync(templateDb, dbPath);
const userId = sqlite('SELECT id FROM users WHERE deleted_at IS NULL ORDER BY created_at LIMIT 1;');
const workerId = sqlite('SELECT id FROM workers WHERE is_active=1 ORDER BY id LIMIT 1;');
const showroomId = sqlite('SELECT id FROM showrooms WHERE is_active=1 ORDER BY id LIMIT 1;');
const employeeId = sqlite('SELECT id FROM payroll_employees WHERE is_active=1 ORDER BY id LIMIT 1;');
const benchmarkUserId = 'phase7-benchmark-employee';
sqlite(`
  INSERT INTO sessions(id,token_hash,user_id,expires_at,created_at) VALUES('phase7-session',${sqlQuote(tokenHash)},${sqlQuote(userId)},'2099-01-01T00:00:00.000Z','2026-09-07T00:00:00.000Z');
  INSERT INTO users(id,full_name,username_norm,password_hash,is_active,created_at,updated_at)
  SELECT ${sqlQuote(benchmarkUserId)},'Phase7 Employee','phase7.employee',password_hash,1,'2026-09-07T00:00:00.000Z','2026-09-07T00:00:00.000Z'
  FROM users WHERE id=${sqlQuote(userId)};
  INSERT INTO user_roles(user_id,role_id) VALUES(${sqlQuote(benchmarkUserId)},'role-employee');
  INSERT INTO user_preferences(user_id,theme,updated_at) VALUES(${sqlQuote(benchmarkUserId)},'light','2026-09-07T00:00:00.000Z');
`);

const child = spawn(binary, [], {
  env: { ...process.env, ALKAHILI_DATA_DIR: dataDir },
  stdio: ['ignore', 'pipe', 'pipe'],
});
let stderr = '';
child.stderr.on('data', (chunk) => { stderr += chunk; });

const report = {
  generated_at: new Date().toISOString(),
  label,
  template_db: templateDb,
  database_bytes_before: statSync(dbPath).size,
  startup_ms: 0,
  writes: {},
  contention: {},
  long_run: {},
  wal: {},
  health: {},
};

try {
  report.startup_ms = +(await waitForServer(child)).toFixed(3);
  const timestamp = (index) => `2026-09-${String(1 + (index % 7)).padStart(2, '0')}T12:${String(index % 60).padStart(2, '0')}:00.000Z`;
  const washBody = (index, suffix = '') => JSON.stringify({
    vehicleMake: 'Phase7', vehicleModel: 'Benchmark', manufactureYear: 2024,
    licensePlate: `P7-${label}-${suffix}-${index}`, carColor: 'white', washType: 'full',
    price: '75', workerId, paymentType: 'cash', occurredAt: timestamp(index),
    clientRequestId: `phase7-${label}-${suffix}-${process.pid}-${index}`,
  });

  await request('/dashboard?date=2026-09-07');
  report.writes.create_wash = await repeat(30, (index) => request('/washes', { method: 'POST', body: washBody(index, 'wash') }));
  report.writes.create_expense = await repeat(30, (index) => request('/expenses', {
    method: 'POST',
    body: JSON.stringify({ description: `Phase7 expense ${index}`, category: 'benchmark', paymentMethod: 'cash', amount: '25', occurredAt: timestamp(index), allocationType: 'business' }),
  }));
  report.writes.showroom_payment = await repeat(30, (index) => request('/showroom-payments', {
    method: 'POST',
    body: JSON.stringify({ showroomId, amount: '20', paidAt: timestamp(index), notes: 'phase7' }),
  }));
  report.writes.salary_withdrawal = await repeat(30, (index) => request('/payroll/withdrawals', {
    method: 'POST',
    body: JSON.stringify({ employeeId, amount: '10', withdrawnAt: timestamp(index), notes: 'phase7' }),
  }));
  report.writes.worker_withdrawal = await repeat(30, (index) => request(`/workers/${workerId}/withdrawals-returns`, {
    method: 'POST',
    body: JSON.stringify({ transactionType: index % 2 ? 'return' : 'withdrawal', amount: '5', occurredAt: timestamp(index), notes: 'phase7' }),
  }));

  const paidWash = await request('/washes', { method: 'POST', body: washBody(500, 'paid') });
  report.writes.mark_paid = await repeat(20, (index) => request(`/washes/${paidWash.data.id}/paid?date=2026-09-07`, {
    method: 'PATCH', body: JSON.stringify({ isPaid: index % 2 === 0 }),
  }));
  const overnightWash = await request('/washes', { method: 'POST', body: washBody(501, 'overnight') });
  report.writes.overnight_transition = await repeat(20, (index) => request(`/washes/${overnightWash.data.id}/overnight`, {
    method: 'PATCH', body: JSON.stringify({ isOvernight: index % 2 === 0 }),
  }));
  report.writes.edit_wash = await repeat(20, (index) => request(`/washes/${paidWash.data.id}`, {
    method: 'PATCH',
    body: JSON.stringify({ vehicleMake: 'Phase7', vehicleModel: `Edited ${index}`, price: String(75 + index), workerId, paymentType: 'cash', occurredAt: timestamp(index) }),
  }));
  report.writes.payroll_salary = await repeat(20, (index) => request(`/payroll/employees/${employeeId}/salary`, {
    method: 'PUT', body: JSON.stringify({ month: '2026-09', salary: String(1000 + index) }),
  }));
  report.writes.user_update = await repeat(20, (index) => request(`/users/${benchmarkUserId}`, {
    method: 'PATCH', body: JSON.stringify({ fullName: `Phase7 Employee ${index}` }),
  }));
  report.writes.permission_update = await repeat(20, () => request(`/users/${benchmarkUserId}/permissions`, {
    method: 'PUT', body: JSON.stringify({ permissionCodes: ['operational.read', 'section.dashboard.access'] }),
  }));

  const mixed = [];
  for (let round = 0; round < 10; round += 1) {
    const operations = [
      request('/dashboard?date=2026-09-07'),
      request(`/workers/${workerId}?from=2025-09-01T00:00:00Z&to=2026-09-30T23:59:59Z`),
      request('/reports/financial?from=2025-09-01T00:00:00Z&to=2026-09-30T23:59:59Z'),
      request('/washes', { method: 'POST', body: washBody(1000 + round, 'mixed') }),
      request('/expenses', { method: 'POST', body: JSON.stringify({ description: `Mixed ${round}`, category: 'benchmark', amount: '5', allocationType: 'business' }) }),
    ];
    const started = performance.now();
    const values = await Promise.all(operations);
    mixed.push({ wall_ms: performance.now() - started, request_ms: values.map((value) => value.elapsed) });
  }
  report.contention.mixed_read_write = {
    wall: summarize(mixed.map((value) => value.wall_ms)),
    requests: summarize(mixed.flatMap((value) => value.request_ms)),
  };

  const loginAndWrites = [];
  for (let round = 0; round < 5; round += 1) {
    const started = performance.now();
    const values = await Promise.all([
      request('/auth/login', { auth: false, method: 'POST', body: JSON.stringify({ username: 'perfaudit', password: 'Performance-Audit-2026' }) }),
      request('/auth/login', { auth: false, method: 'POST', body: JSON.stringify({ username: 'perfaudit', password: 'Performance-Audit-2026' }) }),
      request('/washes', { method: 'POST', body: washBody(2000 + round, 'login-contention') }),
      request('/expenses', { method: 'POST', body: JSON.stringify({ description: `Login contention ${round}`, category: 'benchmark', amount: '5', allocationType: 'business' }) }),
    ]);
    loginAndWrites.push({ wall_ms: performance.now() - started, login_ms: values.slice(0, 2).map((value) => value.elapsed), write_ms: values.slice(2).map((value) => value.elapsed) });
  }
  report.contention.login_and_writes = {
    wall: summarize(loginAndWrites.map((value) => value.wall_ms)),
    logins: summarize(loginAndWrites.flatMap((value) => value.login_ms)),
    writes: summarize(loginAndWrites.flatMap((value) => value.write_ms)),
  };

  const cycleLatencies = [];
  const readLatencies = [];
  for (let cycle = 0; cycle < 300; cycle += 1) {
    cycleLatencies.push((await request('/washes', { method: 'POST', body: washBody(3000 + cycle, 'long-run') })).elapsed);
    if (cycle % 3 === 0) {
      cycleLatencies.push((await request('/expenses', {
        method: 'POST', body: JSON.stringify({ description: `Long run ${cycle}`, category: 'benchmark', amount: '5', allocationType: 'business' }),
      })).elapsed);
    }
    if (cycle % 5 === 0) {
      cycleLatencies.push((await request('/showroom-payments', {
        method: 'POST', body: JSON.stringify({ showroomId, amount: '5', paidAt: timestamp(cycle), notes: 'long-run' }),
      })).elapsed);
    }
    if (cycle % 10 === 0) readLatencies.push((await request('/dashboard?date=2026-09-07')).elapsed);
  }
  report.long_run = {
    cycles: 300,
    writes: summarize(cycleLatencies),
    first_50_writes: summarize(cycleLatencies.slice(0, 50)),
    last_50_writes: summarize(cycleLatencies.slice(-50)),
    dashboard_reads: summarize(readLatencies),
  };

  const walPath = `${dbPath}-wal`;
  report.wal.bytes_before_checkpoint = (() => { try { return statSync(walPath).size; } catch { return 0; } })();
  report.wal.passive_checkpoint = sqlite('PRAGMA wal_checkpoint(PASSIVE);');
  report.wal.bytes_after_checkpoint = (() => { try { return statSync(walPath).size; } catch { return 0; } })();
} finally {
  await stopServer(child);
}

report.database_bytes_after = statSync(dbPath).size;
report.health.integrity_check = sqlite('PRAGMA integrity_check;');
report.health.foreign_key_check_rows = sqlite('PRAGMA foreign_key_check;').split('\n').filter(Boolean).length;
report.health.financial_balance = sqlite("SELECT COALESCE(SUM(CASE entry_side WHEN 'debit' THEN amount_milli ELSE -amount_milli END),0) FROM ledger_entries;");
report.stderr = stderr;
writeFileSync(output, JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 2));
console.log(output);
rmSync(dataDir, { recursive: true, force: true });
