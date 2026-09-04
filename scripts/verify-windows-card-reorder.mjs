import { readFile, writeFile } from 'node:fs/promises';

const mode = process.argv[2];
const statePath = process.argv[3];
const apiRoot = 'http://127.0.0.1:8787/api';
const debuggerRoot = 'http://127.0.0.1:9222';
const username = 'windows-card-order-smoke';
const password = 'WindowsSmoke123!';
const oldSavedOrder = [
  'paidCustomerRevenue',
  'showroomDebts',
  'paidCarsProfitAfterDeductions',
  'businessExpenses',
  'showroomProfit',
  'paidCarsProfitBeforeDeductions',
  'workerCommissions',
  'showroomRevenue',
  'totalRevenue',
];
const newCardId = 'paidCustomerRevenueAfterDeductions';

if (!['exercise', 'verify'].includes(mode) || !statePath) {
  throw new Error('Usage: node scripts/verify-windows-card-reorder.mjs <exercise|verify> <state-file>');
}

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function retry(label, operation, attempts = 90) {
  let lastError;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      const value = await operation();
      if (value !== undefined && value !== false && value !== null) return value;
    } catch (error) {
      lastError = error;
    }
    await delay(500);
  }
  throw new Error(`${label} timed out${lastError ? `: ${lastError.message}` : ''}`);
}

async function api(path, { method = 'GET', token, body } = {}) {
  const response = await fetch(`${apiRoot}${path}`, {
    method,
    headers: {
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(body ? { 'Content-Type': 'application/json' } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const payload = await response.json();
  if (!response.ok) throw new Error(`${method} ${path} failed (${response.status}): ${JSON.stringify(payload)}`);
  return payload.data;
}

async function login() {
  const setup = await api('/setup/status');
  if (setup.needsSetup) {
    return (await api('/setup/initial-manager', {
      method: 'POST',
      body: { fullName: 'Windows Card Order Smoke', username, password },
    })).token;
  }
  return (await api('/auth/login', { method: 'POST', body: { username, password } })).token;
}

class CdpClient {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 1;
    this.pending = new Map();
    this.runtimeErrors = [];
    socket.addEventListener('message', (event) => {
      const message = JSON.parse(String(event.data));
      if (message.id) {
        const pending = this.pending.get(message.id);
        if (!pending) return;
        this.pending.delete(message.id);
        if (message.error) pending.reject(new Error(message.error.message));
        else pending.resolve(message.result);
      } else if (message.method === 'Runtime.exceptionThrown') {
        this.runtimeErrors.push(message.params?.exceptionDetails?.text ?? 'Unknown WebView runtime exception');
      }
    });
  }

  command(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  async evaluate(expression) {
    const result = await this.command('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) {
      throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text ?? 'WebView evaluation failed');
    }
    return result.result?.value;
  }

  close() {
    this.socket.close();
  }
}

async function connectToWebView() {
  const target = await retry('WebView2 debugging target', async () => {
    const response = await fetch(`${debuggerRoot}/json/list`);
    if (!response.ok) return undefined;
    const targets = await response.json();
    return targets.find((item) => item.type === 'page' && item.webSocketDebuggerUrl && item.url && item.url !== 'about:blank');
  });
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true });
    socket.addEventListener('error', reject, { once: true });
  });
  const client = new CdpClient(socket);
  await client.command('Runtime.enable');
  await client.command('Page.enable');
  return client;
}

async function openReports(client, token) {
  await retry('loaded WebView document', async () => {
    try {
      return await client.evaluate(`location.protocol !== 'about:' && document.readyState !== 'loading'`);
    } catch {
      return undefined;
    }
  });
  await client.evaluate(`sessionStorage.setItem('alkaheli.session-token', ${JSON.stringify(token)}); location.hash = '#/reports'; true`);
  await client.command('Page.reload', { ignoreCache: true });
  await retry('financial report cards', async () => {
    const order = await getOrder(client);
    return order.length === 10 && order.includes(newCardId) ? order : undefined;
  });
  const labels = await client.evaluate(`Array.from(document.querySelectorAll('[data-reorder-card-id] .metric-card__content > span')).map((node) => node.textContent.trim())`);
  if (!labels.includes('الإيراد الإجمالي بدون المعارض')) throw new Error('Existing non-showroom revenue card is missing');
  if (!labels.includes('الإيراد الإجمالي بعد المصروفات والمسحوبات')) throw new Error('New after-deductions revenue card is missing');
}

async function getOrder(client) {
  return client.evaluate(`Array.from(document.querySelectorAll('[data-reorder-card-id]')).map((node) => node.dataset.reorderCardId)`);
}

function move(order, cardId, targetIndex) {
  const next = order.filter((id) => id !== cardId);
  next.splice(Math.max(0, Math.min(targetIndex, next.length)), 0, cardId);
  return next;
}

async function dragCard(client, sourceId, targetId) {
  const positions = await client.evaluate(`(() => {
    const sourceCard = document.querySelector('[data-reorder-card-id="${sourceId}"]');
    const targetCard = document.querySelector('[data-reorder-card-id="${targetId}"]');
    if (!sourceCard || !targetCard) return null;
    sourceCard.scrollIntoView({ block: 'center' });
    const source = sourceCard.querySelector('.metric-card__drag-handle').getBoundingClientRect();
    const target = targetCard.getBoundingClientRect();
    return { source: { x: source.left + source.width / 2, y: source.top + source.height / 2 }, target: { x: target.left + target.width / 2, y: target.top + target.height / 2 } };
  })()`);
  if (!positions) throw new Error(`Could not locate drag source ${sourceId} or target ${targetId}`);
  const { source, target } = positions;
  await client.command('Input.dispatchMouseEvent', { type: 'mouseMoved', x: source.x, y: source.y });
  await client.command('Input.dispatchMouseEvent', { type: 'mousePressed', x: source.x, y: source.y, button: 'left', buttons: 1, clickCount: 1 });
  await delay(100);
  await client.command('Input.dispatchMouseEvent', { type: 'mouseMoved', x: (source.x + target.x) / 2, y: (source.y + target.y) / 2, button: 'left', buttons: 1 });
  await delay(100);
  await client.command('Input.dispatchMouseEvent', { type: 'mouseMoved', x: target.x, y: target.y, button: 'left', buttons: 1 });
  await delay(150);
  await client.command('Input.dispatchMouseEvent', { type: 'mouseReleased', x: target.x, y: target.y, button: 'left', buttons: 0, clickCount: 1 });
}

function assertOrder(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`${label}: expected ${JSON.stringify(expected)}, received ${JSON.stringify(actual)}`);
  }
}

const token = await retry('installed application API', login);
const client = await connectToWebView();
try {
  if (mode === 'exercise') {
    await api('/preferences/financial-report-card-order', { method: 'PUT', token, body: { cardOrder: oldSavedOrder } });
    await openReports(client, token);
    const initialOrder = await getOrder(client);
    assertOrder(initialOrder, [...oldSavedOrder, newCardId], 'Old saved layout was not preserved with the new card appended');

    let expectedOrder = [...initialOrder];
    const newCardTarget = expectedOrder.at(-2);
    await dragCard(client, newCardId, newCardTarget);
    expectedOrder = move(expectedOrder, newCardId, expectedOrder.indexOf(newCardTarget));
    await retry('new card pointer reorder', async () => JSON.stringify(await getOrder(client)) === JSON.stringify(expectedOrder));

    const secondSource = expectedOrder[0];
    const secondTarget = expectedOrder[1];
    await dragCard(client, secondSource, secondTarget);
    expectedOrder = move(expectedOrder, secondSource, expectedOrder.indexOf(secondTarget));
    await retry('second pointer reorder', async () => JSON.stringify(await getOrder(client)) === JSON.stringify(expectedOrder));
    await retry('persisted reordered preference', async () => {
      const saved = (await api('/preferences/financial-report-card-order', { token })).cardOrder;
      return JSON.stringify(saved) === JSON.stringify(expectedOrder);
    });
    await writeFile(statePath, JSON.stringify({ expectedOrder }), 'utf8');
    console.log(`Installed WebView2 pointer reorder passed: ${JSON.stringify(expectedOrder)}`);
  } else {
    const { expectedOrder } = JSON.parse(await readFile(statePath, 'utf8'));
    const saved = (await api('/preferences/financial-report-card-order', { token })).cardOrder;
    assertOrder(saved, expectedOrder, 'Saved API preference did not survive application restart');
    await openReports(client, token);
    assertOrder(await getOrder(client), expectedOrder, 'Rendered card order did not survive application restart');
    console.log(`Installed WebView2 restart persistence passed: ${JSON.stringify(expectedOrder)}`);
  }
  if (client.runtimeErrors.length) throw new Error(`WebView runtime errors: ${client.runtimeErrors.join(' | ')}`);
} finally {
  client.close();
}
