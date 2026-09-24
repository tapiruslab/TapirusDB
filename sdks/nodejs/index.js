/**
 * TapirusDB Node.js SDK
 */

const fs = require('fs');
const path = require('path');

class TapirusConnection {
  constructor(config = {}) {
    if (typeof config === 'string') {
      config = { path: config };
    }
    this.path = config.path || ':memory:';
    this.passphrase = config.passphrase || null;
    this.isClosed = false;
    this._tables = {};
  }

  execute(sql) {
    if (this.isClosed) throw new Error('TapirusConnection is closed');
    const clean = sql.trim().replace(/;$/, '');
    const upper = clean.toUpperCase();

    if (upper.startsWith('CREATE TABLE')) {
      const parts = clean.split(/\s+/);
      const name = parts[2].split('(')[0];
      if (!this._tables[name]) this._tables[name] = [];
      return 0;
    } else if (upper.startsWith('INSERT INTO')) {
      const parts = clean.split(/\s+/);
      const name = parts[2];
      if (!this._tables[name]) this._tables[name] = [];
      this._tables[name].push({ id: this._tables[name].length + 1, raw: clean });
      return 1;
    }
    return 0;
  }

  query(sql) {
    if (this.isClosed) throw new Error('TapirusConnection is closed');
    const clean = sql.trim().replace(/;$/, '');
    const parts = clean.split(/\s+/);
    const upperParts = parts.map(p => p.toUpperCase());
    const fromIdx = upperParts.indexOf('FROM');

    if (fromIdx !== -1 && fromIdx + 1 < parts.length) {
      const name = parts[fromIdx + 1];
      return this._tables[name] || [];
    }
    return [];
  }

  close() {
    this.isClosed = true;
  }
}

function open(pathOrConfig) {
  return new TapirusConnection(pathOrConfig);
}

module.exports = {
  open,
  TapirusConnection,
};
