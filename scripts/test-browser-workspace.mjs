import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const base = path.join(root, 'crates', 'tapirus-wasm');
const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'tapirus-browser-'));
const executable = process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe' : '/usr/bin/google-chrome');
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const server = http.createServer((request, response) => {
  const pathname = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
  let file = path.resolve(base, `.${pathname}`);
  if (!file.startsWith(`${base}${path.sep}`) && file !== base) { response.writeHead(403).end(); return; }
  try {
    if (fs.statSync(file).isDirectory()) file = path.join(file, 'index.html');
    const type = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.wasm': 'application/wasm' }[path.extname(file)] || 'application/octet-stream';
    response.setHeader('Content-Type', type); response.end(fs.readFileSync(file));
  } catch { response.writeHead(404).end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
let browser, socket, browserClosed;
try {
  browser = spawn(executable, ['--headless=new', '--no-first-run', '--no-default-browser-check', '--disable-gpu', '--remote-debugging-port=0', `--user-data-dir=${temporary}`, 'about:blank'], { windowsHide: true, stdio: ['ignore', 'ignore', 'pipe'] });
  browserClosed = new Promise(resolve => browser.once('close', resolve));
  let startupError; browser.on('error', error => { startupError = error; });
  browser.stderr.resume();
  const portFile = path.join(temporary, 'DevToolsActivePort');
  const deadline = Date.now() + 20_000;
  while (!fs.existsSync(portFile) && Date.now() < deadline && !startupError) await pause(50);
  if (startupError) throw startupError;
  assert.ok(fs.existsSync(portFile), 'Headless browser did not start');
  const debugPort = Number(fs.readFileSync(portFile, 'utf8').split('\n')[0]);
  const target = await (await fetch(`http://127.0.0.1:${debugPort}/json/new?${encodeURIComponent(`${origin}/demo/`)}`, { method: 'PUT' })).json();
  socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true }); });
  let sequence = 0;
  const pending = new Map();
  const errors = [];
  socket.addEventListener('message', event => {
    const message = JSON.parse(event.data);
    if (message.method === 'Runtime.exceptionThrown') errors.push(message.params.exceptionDetails);
    if (message.id && pending.has(message.id)) {
      const { resolve, reject } = pending.get(message.id); pending.delete(message.id);
      message.error ? reject(new Error(JSON.stringify(message.error))) : resolve(message.result);
    }
  });
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++sequence; pending.set(id, { resolve, reject }); socket.send(JSON.stringify({ id, method, params }));
    setTimeout(() => { if (pending.delete(id)) reject(new Error(`CDP timeout: ${method}`)); }, 15_000).unref();
  });
  const evaluate = async expression => {
    const result = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  const until = async expression => {
    const end = Date.now() + 15_000;
    while (Date.now() < end) { if (await evaluate(expression)) return; await pause(50); }
    throw new Error(`Browser condition timed out: ${expression}`);
  };
  await call('Runtime.enable'); await call('Page.enable');
  const ready = "document.getElementById('engine')?.dataset.engineReady === 'true'";
  await until(ready);
  assert.equal(await evaluate("getComputedStyle(document.getElementById('database-file')).display"), 'none');
  const run = async (sql, parameters = []) => {
    const before = await evaluate("document.getElementById('sql-status').dataset.sequence");
    await evaluate(`document.getElementById('sql').value=${JSON.stringify(sql)};document.getElementById('parameters').value=${JSON.stringify(JSON.stringify(parameters))};document.getElementById('run-sql').click()`);
    await until(`document.getElementById('sql-status').dataset.sequence !== ${JSON.stringify(before)} && !document.getElementById('run-sql').disabled`);
    assert.equal(await evaluate("document.getElementById('sql-status').classList.contains('error')"), false, await evaluate("document.getElementById('sql-status').textContent"));
    return JSON.parse(await evaluate("document.getElementById('sql-result').textContent"));
  };
  await run('CREATE TABLE documents (id INTEGER PRIMARY KEY, content TEXT NOT NULL, embedding VECTOR(2));');
  await run('INSERT INTO documents VALUES (?, ?, ?);', [1, "O'Reilly <img src=x onerror=alert(1)>", [1, 0]]);
  assert.equal((await run('SELECT * FROM documents;'))[0].id, 1);
  assert.equal(await evaluate("document.querySelector('#sql-result img') === null"), true);
  await run('BEGIN;');
  await run('INSERT INTO documents VALUES (?, ?, ?);', [2, 'rollback', [0, 1]]);
  await run('ROLLBACK;');
  assert.equal((await run('SELECT * FROM documents;')).length, 1);
  for (const [width, height, mobile] of [[1440, 1000, false], [390, 844, true]]) {
    await call('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile });
    await pause(150);
    assert.equal(await evaluate('document.documentElement.scrollWidth <= window.innerWidth'), true, `Horizontal overflow at ${width}px`);
    const capture = await call('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(root, 'target', `browser-workspace-${width}.png`), Buffer.from(capture.data, 'base64'));
  }
  await evaluate("document.getElementById('memory-content').value='Engine maintenance guide';document.getElementById('namespace').value='manuals';document.getElementById('embedding').value='[1,0]';document.getElementById('remember').click()");
  await until("document.getElementById('memory-status').textContent.startsWith('Stored source') && !document.getElementById('remember').disabled");
  await call('Page.reload', { ignoreCache: true });
  await until(ready);
  assert.equal((await run('SELECT * FROM documents;')).length, 1, 'IndexedDB must survive a page reload');
  await evaluate("document.getElementById('memory-content').value='maintenance';document.getElementById('namespace').value='manuals';document.getElementById('embedding').value='[1,0]';document.getElementById('recall').click()");
  await until("document.getElementById('memory-status').textContent.includes('sources retrieved') && !document.getElementById('recall').disabled");
  const memory = JSON.parse(await evaluate("document.getElementById('memory-result').textContent"));
  assert.equal(memory[0].content, 'Engine maintenance guide');
  assert.ok(memory[0].semantic_score > 0.99);
  await evaluate("document.getElementById('vault-name').value='separate.tapir';document.getElementById('new-database').click()");
  await until("document.getElementById('sql-status').textContent.includes('Created separate.tapir') && !document.getElementById('new-database').disabled");
  assert.deepEqual(JSON.parse(await evaluate("document.getElementById('schema-result').textContent")), []);
  await evaluate("document.getElementById('vault-name').value='workspace.tapir';document.getElementById('open-saved').click()");
  await until("document.getElementById('sql-status').textContent.includes('Opened saved browser copy workspace.tapir') && !document.getElementById('open-saved').disabled");
  assert.equal((await run('SELECT * FROM documents;')).length, 1);
  assert.deepEqual(errors, []);
  const report = { engine: await evaluate("document.getElementById('engine').textContent"), checks: ['bound SQL', 'transaction rollback', 'HTML text escaping', 'IndexedDB reload', 'AI source persistence', 'hybrid retrieval', 'separate saved databases', '390px and 1440px viewport overflow'], browser: (await call('Browser.getVersion')).product };
  const destination = path.join(root, 'target', 'browser-workspace-results.json');
  fs.writeFileSync(destination, JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report));
  await call('Browser.close');
} finally {
  socket?.close();
  if (browser) {
    // Browser.close acknowledges the request before all subprocesses release the profile.
    await Promise.race([browserClosed, pause(5000)]);
    if (browser.exitCode === null && browser.signalCode === null) {
      browser.kill();
      await Promise.race([browserClosed, pause(5000)]);
    }
  }
  await new Promise(resolve => server.close(resolve));
  // Only remove the fresh, task-owned browser profile.
  await fs.promises.rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 250 });
}
