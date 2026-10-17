const element = id => document.getElementById(id);
const errorMessage = error => error instanceof Error ? error.message : String(error);
const status = (id, message, error = false) => {
  const node = element(id);
  node.textContent = message;
  node.classList.toggle('error', error);
  node.dataset.sequence = String(Number(node.dataset.sequence || 0) + 1);
};
let engine, database, busy = false, savedName = 'workspace.tapir';
const controls = [...document.querySelectorAll('button')];
const storage = new Promise((resolve, reject) => {
  const request = indexedDB.open('tapirus-rust-workspace', 1);
  request.onupgradeneeded = () => request.result.createObjectStore('databases');
  request.onsuccess = () => resolve(request.result);
  request.onerror = () => reject(request.error);
  request.onblocked = () => reject(new Error('Close older workspace tabs to upgrade browser storage'));
});
// Initialization reports storage errors separately; they do not masquerade as saved data.
storage.catch(error => status('storage-status', `Browser storage unavailable: ${errorMessage(error)}. Download backups.`, true));

async function readSaved(name) {
  const store = (await storage).transaction('databases', 'readonly').objectStore('databases');
  return new Promise((resolve, reject) => {
    const request = store.get(name);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
async function writeSaved(name, bytes) {
  const transaction = (await storage).transaction('databases', 'readwrite', { durability: 'strict' });
  transaction.objectStore('databases').put(bytes, name);
  return new Promise((resolve, reject) => {
    transaction.oncomplete = resolve;
    transaction.onerror = () => reject(transaction.error);
    transaction.onabort = () => reject(transaction.error || new Error('Browser save aborted'));
  });
}
function refresh() {
  element('schema-result').textContent = JSON.stringify(JSON.parse(database.schema_json()).filter(table => !table.name.startsWith('__sys_')), null, 2);
  element('transaction-status').textContent = database.is_in_transaction() ? 'TRANSACTION OPEN' : 'AUTOCOMMIT';
}
async function persist() {
  if (database.is_in_transaction()) {
    status('storage-status', 'Transaction open — COMMIT to save, ROLLBACK to discard.');
    return;
  }
  const bytes = database.export_bytes();
  await writeSaved(savedName, bytes);
  localStorage.setItem('tapirus-rust-workspace-active', savedName);
  status('storage-status', `Saved ${savedName} in this browser (${bytes.length.toLocaleString()} bytes).`);
}
async function action(callback, statusId = 'sql-status') {
  if (busy || !database) return;
  busy = true;
  controls.forEach(button => { button.disabled = true; });
  try { await callback(); refresh(); }
  catch (error) { status(statusId, errorMessage(error), true); }
  finally { busy = false; controls.forEach(button => { button.disabled = false; }); }
}
async function saveAfterMutation() {
  try { await persist(); }
  catch (error) { status('storage-status', `Changed in memory; browser save FAILED: ${errorMessage(error)}. Download a backup now.`, true); throw error; }
}
function requireCommitted() {
  if (database.is_in_transaction()) throw new Error('COMMIT or ROLLBACK before replacing or downloading this database.');
}
function normalizeName(value) {
  const name = value.trim().replace(/[^a-zA-Z0-9_.-]/g, '_');
  if (!name) throw new Error('Enter a database name');
  return name.endsWith('.tapir') ? name : `${name}.tapir`;
}

element('run-sql').addEventListener('click', () => action(async () => {
  const sql = element('sql').value.trim();
  const parameters = JSON.parse(element('parameters').value || '[]');
  if (!Array.isArray(parameters)) throw new Error('Parameters must be a JSON array');
  const start = performance.now();
  const query = /^\s*(SELECT|WITH|EXPLAIN|MATCH|CYPHER)\b/i.test(sql) || /\bRETURNING\b/i.test(sql);
  const result = query ? JSON.parse(database.query_params_json(sql, JSON.stringify(parameters))) : { affectedRows: database.execute_params(sql, JSON.stringify(parameters)) };
  element('sql-result').textContent = JSON.stringify(result, null, 2);
  const elapsed = performance.now() - start;
  refresh();
  await saveAfterMutation();
  status('sql-status', `Rust engine completed in ${elapsed.toFixed(2)} ms${query ? ` · ${result.length} rows` : ''}.`);
}));
element('sql').addEventListener('keydown', event => {
  if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') { event.preventDefault(); element('run-sql').click(); }
});
element('sample-sql').addEventListener('click', () => {
  element('sql').value = 'INSERT INTO documents VALUES (?, ?, ?);';
  element('parameters').value = JSON.stringify([1, 'Engine maintenance guide', [1, 0]]);
  element('parameter-details').open = true;
  element('sql').focus();
});
element('refresh-schema').addEventListener('click', () => action(async () => refresh()));

element('new-database').addEventListener('click', () => action(async () => {
  requireCommitted();
  const name = normalizeName(element('vault-name').value);
  const existing = await readSaved(name);
  if (existing) throw new Error('A saved database already uses this name. Use Open browser copy to load it.');
  const next = new engine.TapirusWasm();
  const previous = database; database = next; savedName = name; previous.free();
  await saveAfterMutation();
  element('sql-result').textContent = '[]'; status('sql-status', `Created ${savedName}.`);
}));
element('open-saved').addEventListener('click', () => action(async () => {
  requireCommitted();
  const name = normalizeName(element('vault-name').value);
  const bytes = await readSaved(name);
  if (!bytes) throw new Error('No browser copy uses this name. Open a .tapir file or create a new database.');
  const next = engine.TapirusWasm.import_bytes(bytes);
  const previous = database; database = next; savedName = name; previous.free();
  localStorage.setItem('tapirus-rust-workspace-active', name);
  status('storage-status', `Restored ${name} from browser storage.`);
  status('sql-status', `Opened saved browser copy ${name}.`);
}));
element('open-database').addEventListener('click', () => element('database-file').click());
element('database-file').addEventListener('change', event => {
  const file = event.target.files[0];
  if (!file) return;
  void action(async () => {
    requireCommitted();
    if (file.size > 50 * 1024 * 1024) throw new Error('Use the native SDK for database files larger than 50 MB.');
    let name = normalizeName(file.name);
    if (await readSaved(name)) name = name.replace(/\.tapir$/, `-imported-${Date.now()}.tapir`);
    const next = engine.TapirusWasm.import_bytes(new Uint8Array(await file.arrayBuffer()));
    const previous = database; database = next; savedName = name; previous.free();
    element('vault-name').value = name;
    await saveAfterMutation();
    status('sql-status', `Opened native database ${savedName}.`);
  }).finally(() => { event.target.value = ''; });
});
element('download-database').addEventListener('click', () => action(async () => {
  requireCommitted();
  const bytes = database.export_bytes();
  const url = URL.createObjectURL(new Blob([bytes], { type: 'application/octet-stream' }));
  const link = document.createElement('a'); link.href = url; link.download = savedName; link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  status('sql-status', `Downloaded ${savedName} (${bytes.length.toLocaleString()} bytes).`);
}));

function memoryRequest() {
  const vector = element('embedding').value.trim();
  const embedding = vector ? JSON.parse(vector) : undefined;
  if (embedding !== undefined && (!Array.isArray(embedding) || embedding.length === 0 || embedding.some(x => typeof x !== 'number' || !Number.isFinite(x)))) throw new Error('Embedding must be a nonempty array of finite numbers');
  return { embedding, namespace: element('namespace').value || undefined, session_id: element('session').value || undefined };
}
element('remember').addEventListener('click', () => action(async () => {
  const id = database.store_memory_json(JSON.stringify({ ...memoryRequest(), content: element('memory-content').value }));
  await saveAfterMutation();
  status('memory-status', `Stored source ${id.toString()} in the Rust memory engine.`);
}, 'memory-status'));
element('recall').addEventListener('click', () => action(async () => {
  const results = JSON.parse(database.search_memories_json(JSON.stringify({ ...memoryRequest(), query: element('memory-content').value || undefined, limit: 5 })));
  element('memory-result').textContent = JSON.stringify(results.map(({ entry, ...scores }) => ({ sourceId: entry.id, content: entry.content, namespace: entry.namespace, session: entry.session_id, ...scores })), null, 2);
  status('memory-status', `${results.length} sources retrieved · scores are retrieval components, not probabilities.`);
}, 'memory-status'));

controls.forEach(button => { button.disabled = true; });
try {
  const moduleUrl = new URL(document.querySelector('meta[name="tapirus-engine"]').content, document.baseURI);
  engine = await import(moduleUrl.href);
  await engine.default();
  try {
    savedName = localStorage.getItem('tapirus-rust-workspace-active') || savedName;
    const bytes = await readSaved(savedName);
    database = bytes ? engine.TapirusWasm.import_bytes(bytes) : new engine.TapirusWasm();
    status('storage-status', bytes ? `Restored ${savedName} from browser storage.` : 'New browser workspace. Download backups after important changes.');
  } catch (error) { throw new Error(`Saved database could not be restored: ${errorMessage(error)}. Existing browser data has been preserved.`); }
  element('vault-name').value = savedName;
  refresh();
  element('engine').textContent = `Rust WASM engine v${engine.TapirusWasm.version()}`;
  element('engine').dataset.engineReady = 'true';
  controls.forEach(button => { button.disabled = false; });
  status('sql-status', 'Rust engine ready. Create a table or open a native .tapir file.');
} catch (error) {
  status('engine', `Engine unavailable: ${errorMessage(error)}`, true);
  status('sql-status', 'The Rust engine could not start. Check the generated WASM assets and browser storage.', true);
}
