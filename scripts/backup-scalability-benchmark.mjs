import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { once } from 'node:events';
import { performance } from 'node:perf_hooks';
import { createHash } from 'node:crypto';

const [binary, label = 'baseline', output = `/private/tmp/alkaheli-phase6-${label}.json`, sizesArg = '8,32,64'] = process.argv.slice(2);
if (!binary) throw new Error('usage: node backup-scalability-benchmark.mjs BINARY [label] [output] [sizesMiB]');
const sizes = sizesArg.split(',').map(Number).filter((value) => Number.isFinite(value) && value > 0);
const root = 'http://127.0.0.1:8787/api';
const captureDisk = label.includes('disk');

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function waitForServer(child) {
  for (let attempt = 0; attempt < 600; attempt += 1) {
    if (child.exitCode !== null) throw new Error(`server exited with ${child.exitCode}`);
    try { if ((await fetch(`${root}/health`)).ok) return; } catch {}
    await sleep(25);
  }
  throw new Error('server readiness timeout');
}
async function stop(child) {
  if (child.exitCode !== null) return;
  child.kill('SIGINT');
  await Promise.race([once(child, 'exit'), sleep(1500)]);
  if (child.exitCode === null) child.kill('SIGKILL');
}
function rssKiB(pid) {
  const result = spawnSync('ps', ['-o', 'rss=', '-p', String(pid)], { encoding: 'utf8' });
  return Number(result.stdout.trim()) || 0;
}
function diskKiB(path) {
  const result = spawnSync('du', ['-sk', path], { encoding: 'utf8' });
  return Number(result.stdout.trim().split(/\s+/)[0]) || 0;
}
async function measure(child, operation, dataDir) {
  const baselineRssKiB = rssKiB(child.pid);
  let peakRssKiB = baselineRssKiB;
  const baselineDiskKiB = captureDisk ? diskKiB(dataDir) : null;
  let peakDiskKiB = baselineDiskKiB;
  let stopped = false;
  const sampler = (async () => {
    while (!stopped) {
      peakRssKiB = Math.max(peakRssKiB, rssKiB(child.pid));
      if (captureDisk) peakDiskKiB = Math.max(peakDiskKiB, diskKiB(dataDir));
      await sleep(5);
    }
  })();
  const started = performance.now();
  const value = await operation();
  const elapsedMs = performance.now() - started;
  stopped = true;
  await sampler;
  return {
    elapsedMs: +elapsedMs.toFixed(3),
    baselineRssKiB,
    peakRssKiB,
    rssDeltaKiB: peakRssKiB - baselineRssKiB,
    baselineDiskKiB,
    peakDiskKiB,
    diskDeltaKiB: captureDisk ? peakDiskKiB - baselineDiskKiB : null,
    value,
  };
}
async function json(path, token, options = {}) {
  const headers = new Headers(options.headers);
  if (token) headers.set('Authorization', `Bearer ${token}`);
  if (options.body && !headers.has('Content-Type')) headers.set('Content-Type', 'application/json');
  const response = await fetch(`${root}${path}`, { ...options, headers });
  const payload = await response.json();
  if (!response.ok) throw new Error(`${path}: ${response.status} ${JSON.stringify(payload)}`);
  return payload.data;
}
async function download(path, token) {
  const response = await fetch(`${root}${path}`, { headers: { Authorization: `Bearer ${token}` } });
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (!response.ok) throw new Error(`download: ${response.status}`);
  return { bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
}
async function uploadRestore(bytes, token) {
  const form = new FormData();
  form.append('backup', new Blob([bytes], { type: 'application/octet-stream' }), 'benchmark-backup.db');
  form.append('confirmation', 'RESTORE');
  const response = await fetch(`${root}/backups/restore-upload`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}` },
    body: form,
  });
  const payload = await response.json();
  if (!response.ok) throw new Error(`upload restore: ${response.status} ${JSON.stringify(payload)}`);
  return payload.data;
}
async function restoreWithReadProbe(path, token) {
  let complete = false;
  const probes = [];
  const restore = json('/backups/restore', token, {
    method: 'POST',
    body: JSON.stringify({ path, confirmation: 'RESTORE' }),
  }).finally(() => { complete = true; });
  await sleep(2);
  while (!complete) {
    const started = performance.now();
    const response = await fetch(`${root}/dashboard`, { headers: { Authorization: `Bearer ${token}` } });
    await response.arrayBuffer();
    probes.push({ status: response.status, elapsedMs: +(performance.now() - started).toFixed(3) });
    if (!complete) await sleep(2);
  }
  const value = await restore;
  return {
    restore: value,
    readProbeCount: probes.length,
    successfulReadProbeCount: probes.filter((probe) => probe.status === 200).length,
    maxReadProbeMs: probes.length ? Math.max(...probes.map((probe) => probe.elapsedMs)) : null,
    probes,
  };
}
function inflateDatabase(path, targetMiB) {
  const chunks = Math.ceil(targetMiB / 2);
  const sql = `PRAGMA journal_mode=DELETE; CREATE TABLE IF NOT EXISTS phase6_payload(id INTEGER PRIMARY KEY,payload BLOB NOT NULL); BEGIN; ${Array.from({length: chunks}, (_, index) => `INSERT INTO phase6_payload(id,payload) VALUES(${index + 1},randomblob(2097152));`).join(' ')} COMMIT; VACUUM;`;
  const result = spawnSync('sqlite3', [path], { input: sql, encoding: 'utf8', maxBuffer: 1024 * 1024 });
  if (result.status !== 0) throw new Error(result.stderr);
}
function fileSha256(path) {
  const result = spawnSync('shasum', ['-a', '256', path], { encoding: 'utf8' });
  if (result.status !== 0) throw new Error(result.stderr);
  return result.stdout.trim().split(/\s+/)[0];
}
function databaseFingerprints(path) {
  const tablesResult = spawnSync('sqlite3', [path, "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"], { encoding: 'utf8' });
  if (tablesResult.status !== 0) throw new Error(tablesResult.stderr);
  const excluded = new Set(['audit_logs', 'backup_history', 'sessions']);
  return Object.fromEntries(tablesResult.stdout.trim().split('\n').filter(Boolean).filter((table) => !excluded.has(table)).map((table) => {
    const digest = spawnSync('sqlite3', [path, `.sha3sum --schema ${table}`], { encoding: 'utf8' });
    if (digest.status !== 0) throw new Error(digest.stderr);
    return [table, digest.stdout.trim().split(/\s+/)[0]];
  }));
}
function metadataCounts(path) {
  const result = spawnSync('sqlite3', [path, "SELECT (SELECT COUNT(*) FROM sessions),(SELECT COUNT(*) FROM backup_history WHERE status='completed'),(SELECT COUNT(*) FROM audit_logs WHERE action='BACKUP_RESTORED')"], { encoding: 'utf8' });
  if (result.status !== 0) throw new Error(result.stderr);
  const [sessions, completedBackups, restoreAudits] = result.stdout.trim().split('|').map(Number);
  return { sessions, completedBackups, restoreAudits };
}

const report = { label, binary, generatedAt: new Date().toISOString(), sizes: {} };
for (const requestedMiB of sizes) {
  const dataDir = `/private/tmp/alkaheli-phase6-${label}-${requestedMiB}-${process.pid}`;
  rmSync(dataDir, { recursive: true, force: true }); mkdirSync(dataDir, { recursive: true });
  const child = spawn(binary, [], { env: { ...process.env, ALKAHILI_DATA_DIR: dataDir }, stdio: ['ignore', 'ignore', 'pipe'] });
  let stderr = ''; child.stderr.on('data', (chunk) => { stderr += chunk; });
  await waitForServer(child).catch((error) => { throw new Error(`${error.message}\n${stderr}`); });
  const setup = await json('/setup/initial-manager', null, { method: 'POST', body: JSON.stringify({ fullName: 'Phase 6 Benchmark', username: 'phase6', password: 'Phase6-Benchmark-2026' }) });
  await stop(child);
  inflateDatabase(`${dataDir}/carwash.db`, requestedMiB);

  let server = spawn(binary, [], { env: { ...process.env, ALKAHILI_DATA_DIR: dataDir }, stdio: ['ignore', 'ignore', 'pipe'] });
  await waitForServer(server);
  let token = setup.token;
  const backup = await measure(server, () => json('/backups', token, { method: 'POST', body: '{}' }), dataDir);
  const backupPath = backup.value.path;
  const backupId = backup.value.id;
  const snapshotFingerprints = databaseFingerprints(backupPath);
  const exportedPath = `${dataDir}/export-${requestedMiB}.db`;
  const exported = await measure(server, () => json(`/backups/${backupId}/export`, token, { method: 'PUT', body: JSON.stringify({ path: exportedPath }) }), dataDir);
  const downloaded = await measure(server, () => download(`/backups/${backupId}/download`, token), dataDir);
  await stop(server);
  server = spawn(binary, [], { env: { ...process.env, ALKAHILI_DATA_DIR: dataDir }, stdio: ['ignore', 'ignore', 'pipe'] });
  await waitForServer(server);
  const restored = await measure(server, () => restoreWithReadProbe(backupPath, token), dataDir);
  token = (await json('/auth/login', null, { method: 'POST', body: JSON.stringify({ username: 'phase6', password: 'Phase6-Benchmark-2026' }) })).token;
  await stop(server);
  server = spawn(binary, [], { env: { ...process.env, ALKAHILI_DATA_DIR: dataDir }, stdio: ['ignore', 'ignore', 'pipe'] });
  await waitForServer(server);
  const uploadBytes = readFileSync(backupPath);
  const uploadedRestore = await measure(server, () => uploadRestore(uploadBytes, token), dataDir);
  token = (await json('/auth/login', null, { method: 'POST', body: JSON.stringify({ username: 'phase6', password: 'Phase6-Benchmark-2026' }) })).token;
  const integrity = spawnSync('sqlite3', [`${dataDir}/carwash.db`, 'PRAGMA integrity_check; PRAGMA foreign_key_check; SELECT COUNT(*) FROM phase6_payload;'], { encoding: 'utf8' });
  const fileBytes = Number(spawnSync('stat', ['-f', '%z', `${dataDir}/carwash.db`], { encoding: 'utf8' }).stdout.trim());
  const restoredFingerprints = databaseFingerprints(`${dataDir}/carwash.db`);
  const fingerprintDifferences = Object.keys(snapshotFingerprints).filter((table) => snapshotFingerprints[table] !== restoredFingerprints[table]);
  report.sizes[requestedMiB] = {
    fileBytes,
    backup,
    exported,
    downloaded,
    restored,
    uploadedRestore,
    correctness: {
      backupSha256MatchesMetadata: fileSha256(backupPath) === backup.value.sha256,
      exportSha256MatchesBackup: fileSha256(exportedPath) === fileSha256(backupPath),
      downloadSha256MatchesBackup: downloaded.value.sha256 === fileSha256(backupPath),
      businessTableCount: Object.keys(snapshotFingerprints).length,
      fingerprintDifferences,
      metadataCounts: metadataCounts(`${dataDir}/carwash.db`),
    },
    integrityOutput: integrity.stdout.trim(),
    reloginSucceeded: Boolean(token),
  };
  writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`);
  await stop(server);
  rmSync(dataDir, { recursive: true, force: true });
}
console.log(JSON.stringify(report, null, 2));
