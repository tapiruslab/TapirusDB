/**
 * TapirusDB Official Node.js / TypeScript SDK
 * Safe-Rust Quad-Model Embedded AI Database & Agent Memory Engine
 */

const { spawnSync } = require('child_process');
const fs = require('fs');
const path = require('path');

function findBinary() {
  const envPath = process.env.TAPIRUS_BIN_PATH;
  if (envPath && fs.existsSync(envPath)) return envPath;

  const candidateDirs = [
    path.join(__dirname, 'bin'),
    path.join(__dirname, '..', '..', 'target', 'release'),
    path.join(__dirname, '..', '..', 'target', 'debug'),
    '/usr/local/bin',
    '/usr/bin'
  ];

  const binName = process.platform === 'win32' ? 'tapirus.exe' : 'tapirus';

  for (const dir of candidateDirs) {
    const full = path.join(dir, binName);
    if (fs.existsSync(full)) return full;
  }

  try {
    const check = spawnSync(binName, ['--version'], { encoding: 'utf-8', stdio: 'pipe' });
    if (check.status === 0) return binName;
  } catch (_) {}

  return null;
}

class TapirusConnection {
  constructor(config = {}) {
    if (typeof config === 'string') {
      config = { path: config };
    }
    this.path = config.path || ':memory:';
    this.passphrase = config.passphrase || null;
    this.isClosed = false;
    this._bin = findBinary();
    this._localTables = {};
  }

  execute(sql) {
    if (this.isClosed) throw new Error('TapirusConnection is closed');

    if (this._bin) {
      const args = [this.path, '-c', sql];
      const res = spawnSync(this._bin, args, { encoding: 'utf-8' });
      if (res.status !== 0) {
        throw new Error(res.stderr || res.stdout || 'Execution failed');
      }
      return 1;
    }

    return this._emulateExecute(sql);
  }

  query(sql) {
    if (this.isClosed) throw new Error('TapirusConnection is closed');

    if (this._bin) {
      const args = [this.path, '--json', sql];
      const res = spawnSync(this._bin, args, { encoding: 'utf-8' });
      if (res.status !== 0) {
        throw new Error(res.stderr || res.stdout || 'Query failed');
      }
      try {
        return JSON.parse(res.stdout.trim() || '[]');
      } catch (e) {
        throw new Error(`Failed to parse TapirusDB JSON output: ${res.stdout}`);
      }
    }

    return this._emulateQuery(sql);
  }

  vectorSearch(table, vectorCol, queryVector, topK = 5, where = null) {
    const vecStr = `[${queryVector.map(x => Number(x).toFixed(6)).join(', ')}]`;
    let sql = `SELECT * FROM ${table} VECTOR NEAR ${vectorCol} = ${vecStr} TOP ${topK}`;
    if (where) sql += ` WHERE ${where}`;
    return this.query(sql);
  }

  graphQuery(cypherOrSql) {
    return this.query(cypherOrSql);
  }

  graphAlgorithm(algorithm, options = {}) {
    const opts = Object.entries(options).map(([k, v]) => `${k} ${v}`).join(' ');
    let sql = `GRAPH ALGORITHM ${algorithm.toUpperCase()}`;
    if (opts) sql += ` ${opts}`;
    return this.query(sql);
  }

  close() {
    this.isClosed = true;
  }

  _emulateExecute(sql) {
    const clean = sql.trim().replace(/;$/, '');
    const upper = clean.toUpperCase();
    if (upper.startsWith('CREATE TABLE')) {
      const parts = clean.split(/\s+/);
      const name = parts[2].split('(')[0];
      if (!this._localTables[name]) this._localTables[name] = [];
      return 0;
    } else if (upper.startsWith('INSERT INTO')) {
      const parts = clean.split(/\s+/);
      const name = parts[2];
      if (!this._localTables[name]) this._localTables[name] = [];
      this._localTables[name].push({ id: this._localTables[name].length + 1, raw: clean });
      return 1;
    }
    return 0;
  }

  _emulateQuery(sql) {
    const clean = sql.trim().replace(/;$/, '');
    const parts = clean.split(/\s+/);
    const upperParts = parts.map(p => p.toUpperCase());
    const fromIdx = upperParts.indexOf('FROM');
    if (fromIdx !== -1 && fromIdx + 1 < parts.length) {
      const name = parts[fromIdx + 1];
      return this._localTables[name] || [];
    }
    return [];
  }
}

function open(pathOrConfig) {
  return new TapirusConnection(pathOrConfig);
}

module.exports = {
  open,
  Tapirus: TapirusConnection,
  TapirusConnection,
  default: { open, Tapirus: TapirusConnection, TapirusConnection }
};
