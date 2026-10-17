'use strict';
// Shared native loader source. scripts/sync-node-sdk.js copies this API to both npm packages.
const fs = require('node:fs');
const path = require('node:path');
let binding;

function loadBinding() {
  if (binding) return binding;
  const candidates = process.env.TAPIRUS_NATIVE_PATH
    ? [path.resolve(process.env.TAPIRUS_NATIVE_PATH)]
    : [path.join(__dirname, 'tapirus.node'), path.join(__dirname, 'build', 'Release', 'tapirus.node'),
      ...fs.readdirSync(__dirname).filter(file => /^tapirus\..+\.node$/.test(file)).map(file => path.join(__dirname, file))];
  const failures = [];
  for (const file of candidates) {
    if (!fs.existsSync(file)) continue;
    try {
      const native = { exports: {} };
      process.dlopen(native, file);
      if (typeof native.exports.TapirusClient !== 'function') throw new Error('Missing TapirusClient export');
      binding = native.exports;
      return binding;
    } catch (error) { failures.push(`${path.basename(file)}: ${error.message}`); }
  }
  throw new Error(`TapirusDB native engine is unavailable for ${process.platform}/${process.arch}. Install a package containing the matching .node binary or set TAPIRUS_NATIVE_PATH for a local build.${failures.length ? '\n' + failures.join('\n') : ''}`);
}

function paramsJson(params) {
  if (!Array.isArray(params)) throw new TypeError('Parameters must be an array');
  return JSON.stringify(params, (_key, value) => {
    if (typeof value === 'bigint') return { $integer: value.toString() };
    if (value instanceof Uint8Array || (value && value.type === 'Buffer' && Array.isArray(value.data))) return { $blob: Array.from(value.data || value) };
    if (typeof value === 'number' && (!Number.isFinite(value) || (Number.isInteger(value) && !Number.isSafeInteger(value)))) throw new TypeError('Use finite numbers and bigint for integers outside the safe JS range');
    if (value === undefined || typeof value === 'function' || typeof value === 'symbol') throw new TypeError('Unsupported SQL parameter');
    return value;
  });
}
function identifier(value) {
  if (typeof value !== 'string' || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(value)) throw new TypeError('Invalid SQL identifier');
  return value;
}
function limitValue(value) {
  if (!Number.isSafeInteger(value) || value < 0) throw new TypeError('Limit must be a nonnegative safe integer');
  return value;
}

function memoryPayload(value) {
  const names = { sessionId: 'session_id', externalId: 'external_id', sourceVersion: 'source_version', exceptSourceVersion: 'except_source_version', embeddingModel: 'embedding_model', expiresAt: 'expires_at', ttlSeconds: 'ttl_seconds', expectedRevision: 'expected_revision', candidateLimit: 'candidate_limit', rrfK: 'rrf_k', vectorWeight: 'vector_weight', lexicalWeight: 'lexical_weight', recencyWeight: 'recency_weight', importanceWeight: 'importance_weight', halfLifeSeconds: 'half_life_seconds', minScore: 'min_score', maxDocuments: 'max_documents', maxEstimatedBytes: 'max_estimated_bytes', maxDocumentBytes: 'max_document_bytes', maxVectorDimensions: 'max_vector_dimensions', maxVectorSpaces: 'max_vector_spaces', maxBatchDocuments: 'max_batch_documents', maxCandidates: 'max_candidates', hnswM: 'hnsw_m', efConstruction: 'ef_construction', efSearch: 'ef_search' };
  const normalize = item => {
    if (typeof item === 'number' && (!Number.isFinite(item) || (Number.isInteger(item) && !Number.isSafeInteger(item)))) throw new TypeError('Memory values require finite numbers and exact integer parameters');
    if (Array.isArray(item)) return item.map(normalize);
    if (item && typeof item === 'object') {
      const output = Object.create(null);
      for (const [key, entry] of Object.entries(item)) {
        const name = names[key] || key;
        if (Object.hasOwn(output, name)) throw new TypeError(`Duplicate memory field: ${name}`);
        output[name] = normalize(entry);
      }
      return output;
    }
    return item;
  };
  return JSON.stringify(normalize(value));
}

class TapirusDatabase {
  constructor(filePath = ':memory:', options = {}) {
    const nativeOptions = { dbPath: filePath };
    if (options.passphrase !== undefined) nativeOptions.passphrase = options.passphrase;
    if (options.memory !== undefined) nativeOptions.memoryConfigJson = memoryPayload(options.memory);
    this._db = new (loadBinding().TapirusClient)(nativeOptions);
    this.isNative = true;
    this.isOpen = true;
    this.path = filePath;
  }
  static open(filePath = ':memory:', options = {}) { return new TapirusDatabase(filePath, options); }
  static openInMemory(options = {}) { return new TapirusDatabase(':memory:', options); }
  static version() { return loadBinding().version(); }
  execute(sql, params = []) { return this._db.execute(sql, paramsJson(params)); }
  query(sql, params = []) { return this._db.query(sql, paramsJson(params)); }
  vectorSearch(table, column, vector, limit = 5) {
    return this.query(`SELECT * FROM ${identifier(table)} VECTOR NEAR ${identifier(column)} = ? TOP ${limitValue(limit)};`, [vector]);
  }
  graphAlgorithm(algorithm) { return this.query(`GRAPH ALGORITHM ${identifier(algorithm)};`); }
  tapClassifyDetailed(text, candidates) { return JSON.parse(this._db.tapClassifyJson(text, candidates)); }
  tapClassify(text, candidates) { return this.tapClassifyDetailed(text, candidates).top_choice; }
  tapVerify(premise, hypothesis, threshold = 0.5) { return this._db.tapVerify(premise, hypothesis, threshold); }
  tapScore(text, criteria) { return this._db.tapScore(text, criteria); }
  tapRoute(state, routes) { return this._db.tapRoute(state, routes); }
  remember(text, importance = 0.5, tags = []) { return this._db.remember(text, importance, tags); }
  storeMemory(document) { return this._db.storeMemoryJson(memoryPayload(document)); }
  searchMemories(search) { return JSON.parse(this._db.searchMemoriesJson(memoryPayload(search))); }
  _memoryRequest(operation, payload = {}) { return JSON.parse(this._db.memoryRequestJson(operation, memoryPayload(payload))); }
  upsertMemory(document) { return this._memoryRequest('upsert', document); }
  storeMemoryBatch(documents) { return this._memoryRequest('store_batch', documents); }
  upsertMemoryBatch(documents) { return this._memoryRequest('upsert_batch', documents); }
  searchMemoriesDetailed(search) { return this._memoryRequest('search_detailed', search); }
  configureMemory(config) { return this._memoryRequest('configure', config); }
  memoryStats() { return this._memoryRequest('stats'); }
  storageHealth() { return this._memoryRequest('storage_health'); }
  prepareMemoryIndexes() { return this._memoryRequest('prepare'); }
  purgeExpiredMemories() { return this._memoryRequest('purge_expired'); }
  deleteMemories(scope) { return this._memoryRequest('delete', scope); }
  forgetMemory(id) {
    if (!Number.isSafeInteger(id) || id < 1) throw new TypeError('Memory ID must be a positive safe integer');
    return this._db.forgetMemory(String(id));
  }
  recall(query, limit = 5) { return JSON.parse(this._db.recallJson(query, limitValue(limit))); }
  recallPrompt(query, limit = 5) { return this._db.recallPrompt(query, limitValue(limit)); }
  graphRagQuery({ query, limit = 5 }) { return JSON.parse(this._db.graphRagJson(query, limitValue(limit))); }
  subscribe(table, listener) {
    const key = identifier(table).toLowerCase();
    const id = this._db.subscribe(key, (error, json) => {
      if (error) throw error;
      listener(JSON.parse(json));
    });
    return { unsubscribe: () => this._db.unsubscribe(id) };
  }
  checkpoint() { return this._db.checkpoint(); }
  vacuumInto(targetPath) { return this.execute('VACUUM INTO ?;', [targetPath]); }
  close() { this._db.close(); this.isOpen = false; }
}
class TapirusSessionDatabase {
  constructor(filePath = ':memory:', options = {}) {
    this._db = new (loadBinding().SessionDatabaseClient)({ dbPath: filePath, passphrase: options.passphrase, memoryConfigJson: options.memory === undefined ? undefined : memoryPayload(options.memory) });
  }
  session({ busyTimeoutMs = 0 } = {}) {
    if (!Number.isInteger(busyTimeoutMs) || busyTimeoutMs < 0 || busyTimeoutMs > 0xffffffff) throw new TypeError('Invalid session busyTimeoutMs');
    return new TapirusSession(this._db.session(busyTimeoutMs));
  }
  close() { this._db.close(); }
}
class TapirusSession {
  constructor(native) { this._db = native; }
  execute(sql, params = []) { return this._db.execute(sql, paramsJson(params)); }
  query(sql, params = []) { return this._db.query(sql, paramsJson(params)); }
  begin() { this.execute('BEGIN'); }
  commit() { this.execute('COMMIT'); }
  rollback() { this.execute('ROLLBACK'); }
  transaction(callback) {
    this.begin();
    try {
      const result = callback(this);
      if (result && typeof result.then === 'function') throw new TypeError('Session transaction callbacks must be synchronous');
      this.commit(); return result;
    } catch (error) { try { this.rollback(); } catch (rollback) { error.rollbackError = rollback; } throw error; }
  }
  checkpoint() { return this._db.checkpoint(); }
  close() { this._db.close(); }
}
for (const name of ['_memoryRequest', 'storeMemory', 'searchMemories', 'upsertMemory', 'storeMemoryBatch', 'upsertMemoryBatch', 'searchMemoriesDetailed', 'configureMemory', 'memoryStats', 'storageHealth', 'prepareMemoryIndexes', 'purgeExpiredMemories', 'deleteMemories', 'forgetMemory']) {
  Object.defineProperty(TapirusSession.prototype, name, { value: TapirusDatabase.prototype[name] });
}
module.exports = { open: TapirusDatabase.open, Tapirus: TapirusDatabase, TapirusDatabase, TapirusSessionDatabase, version: TapirusDatabase.version };
