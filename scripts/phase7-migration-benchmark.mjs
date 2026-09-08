import { copyFileSync, mkdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { performance } from 'node:perf_hooks';

const [binary, label = 'run', output = `/private/tmp/alkaheli-phase7-migration-${label}.json`, ...templates] = process.argv.slice(2);
if (!binary || templates.length === 0) {
  throw new Error('usage: node phase7-migration-benchmark.mjs BINARY LABEL OUTPUT TEMPLATE_DB...');
}
const root = 'http://127.0.0.1:8787/api';
const rewindVersion = Number(process.env.PHASE7_REWIND_VERSION ?? 25);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const sqlite = (path, sql) => {
  const result = spawnSync('sqlite3', [path, sql], { encoding: 'utf8' });
  if (result.status !== 0) throw new Error(result.stderr || `sqlite exited ${result.status}`);
  return result.stdout.trim();
};
const diskKiB = (path) => Number(spawnSync('du', ['-sk', path], { encoding: 'utf8' }).stdout.trim().split(/\s+/)[0]) || 0;

async function stop(child) {
  if (child.exitCode !== null) return;
  child.kill('SIGINT');
  await Promise.race([once(child, 'exit'), sleep(2000)]);
  if (child.exitCode === null) child.kill('SIGKILL');
}

const report = { generated_at: new Date().toISOString(), label, rewind_version: rewindVersion, results: [] };
for (const [index, template] of templates.entries()) {
  const dataDir = `/private/tmp/alkaheli-phase7-migration-${label}-${process.pid}-${index}`;
  const dbPath = `${dataDir}/carwash.db`;
  rmSync(dataDir, { recursive: true, force: true });
  mkdirSync(dataDir, { recursive: true });
  copyFileSync(template, dbPath);
  sqlite(dbPath, `
    PRAGMA journal_mode=DELETE;
    BEGIN;
    DELETE FROM schema_migrations WHERE version > ${rewindVersion};
    DROP INDEX IF EXISTS idx_washes_posted_report_cover;
    DROP INDEX IF EXISTS idx_washes_showroom_posted_summary;
    DROP INDEX IF EXISTS idx_showroom_payments_time_showroom;
    DROP INDEX IF EXISTS idx_showroom_payments_showroom_history;
    DROP INDEX IF EXISTS idx_salary_withdrawals_time_employee;
    DROP INDEX IF EXISTS idx_salary_withdrawals_employee_history;
    DROP INDEX IF EXISTS idx_salary_deductions_time_employee;
    DROP INDEX IF EXISTS idx_salary_deductions_employee_history;
    DROP INDEX IF EXISTS idx_backup_history_status_time;
    DROP INDEX IF EXISTS idx_washes_paid_owner;
    DROP INDEX IF EXISTS idx_washes_occured;
    DROP INDEX IF EXISTS idx_washes_worker;
    DROP INDEX IF EXISTS idx_expenses_date;
    DROP INDEX IF EXISTS idx_audit_time;
    CREATE INDEX idx_washes_paid_owner ON wash_operations(is_paid,status,created_by,occurred_at DESC);
    CREATE INDEX idx_washes_occured ON wash_operations(occurred_at,status);
    CREATE INDEX idx_washes_worker ON wash_operations(worker_id,occurred_at);
    CREATE INDEX idx_expenses_date ON expenses(occurred_at);
    CREATE INDEX idx_audit_time ON audit_logs(created_at DESC);
    COMMIT;
  `);
  const rows = Number(sqlite(dbPath, 'SELECT COUNT(*) FROM wash_operations;'));
  const bytesBefore = statSync(dbPath).size;
  const diskBeforeKiB = diskKiB(dataDir);
  let peakDiskKiB = diskBeforeKiB;
  let sampling = true;
  const child = spawn(binary, [], { env: { ...process.env, ALKAHILI_DATA_DIR: dataDir }, stdio: ['ignore', 'pipe', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  const sampler = (async () => {
    while (sampling) {
      peakDiskKiB = Math.max(peakDiskKiB, diskKiB(dataDir));
      await sleep(2);
    }
  })();
  const started = performance.now();
  for (;;) {
    if (child.exitCode !== null) throw new Error(`server exited ${child.exitCode}: ${stderr}`);
    try {
      if ((await fetch(`${root}/health`)).ok) break;
    } catch {}
    if (performance.now() - started > 30000) throw new Error(`startup timeout: ${stderr}`);
    await sleep(2);
  }
  const migrationStartupMs = performance.now() - started;
  sampling = false;
  await sampler;
  await stop(child);
  report.results.push({
    rows,
    migration_startup_ms: +migrationStartupMs.toFixed(3),
    database_bytes_before: bytesBefore,
    database_bytes_after: statSync(dbPath).size,
    temporary_disk_peak_kib: peakDiskKiB - diskBeforeKiB,
    schema_version: Number(sqlite(dbPath, 'SELECT MAX(version) FROM schema_migrations;')),
    migration_count: Number(sqlite(dbPath, 'SELECT COUNT(*) FROM schema_migrations WHERE version BETWEEN 2 AND 29;')),
    integrity_check: sqlite(dbPath, 'PRAGMA integrity_check;'),
    foreign_key_check_rows: sqlite(dbPath, 'PRAGMA foreign_key_check;').split('\n').filter(Boolean).length,
  });
  rmSync(dataDir, { recursive: true, force: true });
}
writeFileSync(output, JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 2));
console.log(output);
