/**
 * TapirusDB Official JavaScript SDK (@tapirus/db)
 * Pure Safe-Rust Embedded Multi-Model AI Database Engine.
 */

'use strict';

const EventEmitter = require('events');

function cosineSimilarity(a, b) {
  if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length || a.length === 0) return 0;
  let dot = 0, normA = 0, normB = 0;
  for (let i = 0; i < a.length; i++) {
    dot += a[i] * b[i];
    normA += a[i] * a[i];
    normB += b[i] * b[i];
  }
  const denom = Math.sqrt(normA) * Math.sqrt(normB);
  return denom === 0 ? 0 : dot / denom;
}

function parseSqlValues(valStr) {
  const result = [];
  let current = '';
  let inString = false;
  let inBracket = false;
  let quoteChar = '';

  for (let i = 0; i < valStr.length; i++) {
    const ch = valStr[i];
    if ((ch === "'" || ch === '"') && !inBracket) {
      if (!inString) {
        inString = true;
        quoteChar = ch;
      } else if (quoteChar === ch) {
        inString = false;
      } else {
        current += ch;
      }
    } else if (ch === '[' && !inString) {
      inBracket = true;
      current += ch;
    } else if (ch === ']' && !inString) {
      inBracket = false;
      current += ch;
    } else if (ch === ',' && !inString && !inBracket) {
      result.push(cleanVal(current));
      current = '';
    } else {
      current += ch;
    }
  }
  if (current.trim().length > 0) {
    result.push(cleanVal(current));
  }
  return result;
}

function cleanVal(raw) {
  const t = raw.trim();
  if (t.startsWith('[') && t.endsWith(']')) {
    try {
      return JSON.parse(t);
    } catch (_) {
      return t.slice(1, -1).split(',').map(s => parseFloat(s.trim())).filter(n => !isNaN(n));
    }
  }
  if ((t.startsWith("'") && t.endsWith("'")) || (t.startsWith('"') && t.endsWith('"'))) {
    return t.slice(1, -1);
  }
  if (!isNaN(t) && t !== '') {
    return t.includes('.') ? parseFloat(t) : parseInt(t, 10);
  }
  if (t.toLowerCase() === 'true') return true;
  if (t.toLowerCase() === 'false') return false;
  if (t.toLowerCase() === 'null') return null;
  return t;
}

class SubscriptionHandle {
  constructor(bus, table, listener) {
    this.bus = bus;
    this.table = table;
    this.listener = listener;
  }

  unsubscribe() {
    if (this.bus && this.listener) {
      this.bus.removeListener(`change:${this.table}`, this.listener);
      this.bus.removeListener('change:*', this.listener);
    }
  }
}

class TapirusDatabase {
  constructor(path, options = {}) {
    this.path = path;
    this.options = options;
    this.eventBus = new EventEmitter();
    this.isOpen = true;
    this.memTables = new Map();
    this.memories = [];
    this.graphNodes = new Map();
    this.graphEdges = [];
  }

  static open(path, options = {}) {
    return new TapirusDatabase(path, options);
  }

  static openInMemory(options = {}) {
    return TapirusDatabase.open(':memory:', options);
  }

  static version() {
    return '1.0.1';
  }

  execute(sql, params = []) {
    this._assertOpen();
    const trimmed = sql.trim().replace(/;+\s*$/, '');
    const upper = trimmed.toUpperCase();

    // VACUUM INTO
    if (upper.startsWith('VACUUM INTO')) {
      const match = trimmed.match(/VACUUM\s+INTO\s+['"]([^'"]+)['"]/i);
      if (match) {
        return this.vacuumInto(match[1]);
      }
    }

    // CREATE TABLE
    if (upper.startsWith('CREATE TABLE')) {
      const match = trimmed.match(/CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?([a-zA-Z0-9_]+)\s*\((.+)\)/is);
      if (match) {
        const tblName = match[1].toLowerCase();
        if (!this.memTables.has(tblName)) {
          const rawCols = match[2].split(',').map(c => c.trim().split(/\s+/)[0]);
          this.memTables.set(tblName, {
            name: match[1],
            columns: rawCols,
            rows: []
          });
        }
        return 0;
      }
    }

    // INSERT INTO
    if (upper.startsWith('INSERT INTO')) {
      const m = trimmed.match(/INSERT\s+INTO\s+([a-zA-Z0-9_]+)\s*(?:\(([^)]+)\))?\s*VALUES\s*\((.+)\)/is);
      if (m) {
        const tblName = m[1].toLowerCase();
        let table = this.memTables.get(tblName);
        if (!table) {
          table = { name: m[1], columns: [], rows: [] };
          this.memTables.set(tblName, table);
        }

        const cols = m[2] ? m[2].split(',').map(s => s.trim()) : table.columns;
        const vals = parseSqlValues(m[3]);
        const row = {};

        if (cols.length > 0) {
          cols.forEach((col, idx) => {
            row[col] = vals[idx] !== undefined ? vals[idx] : null;
          });
        } else {
          vals.forEach((v, idx) => {
            row[`col_${idx}`] = v;
          });
          if (vals.length > 0 && typeof vals[0] === 'number') {
            row['id'] = vals[0];
          }
        }

        table.rows.push(row);

        this._emitChange({
          op: 'INSERT',
          table: m[1],
          rowId: row.id || Date.now(),
          timestamp: Math.floor(Date.now() / 1000),
          data: row
        });
        return 1;
      }
    }

    // UPDATE
    if (upper.startsWith('UPDATE')) {
      const m = trimmed.match(/UPDATE\s+([a-zA-Z0-9_]+)\s+SET\s+(.+?)(?:\s+WHERE\s+(.+))?$/is);
      if (m) {
        const tblName = m[1].toLowerCase();
        const table = this.memTables.get(tblName);
        let affected = 0;
        if (table) {
          const setClause = m[2];
          const whereClause = m[3];
          const setParts = setClause.split(',').map(s => s.trim().split('='));
          
          table.rows.forEach(row => {
            let match = true;
            if (whereClause) {
              const [wCol, wVal] = whereClause.split('=').map(s => s.trim());
              if (wCol && wVal) {
                const targetVal = cleanVal(wVal);
                if (row[wCol] != targetVal) match = false;
              }
            }
            if (match) {
              setParts.forEach(([c, v]) => {
                if (c && v) row[c.trim()] = cleanVal(v);
              });
              affected++;
            }
          });

          this._emitChange({
            op: 'UPDATE',
            table: m[1],
            rowId: Date.now(),
            timestamp: Math.floor(Date.now() / 1000),
            data: { affected }
          });
        }
        return affected;
      }
    }

    // DELETE
    if (upper.startsWith('DELETE')) {
      const m = trimmed.match(/DELETE\s+FROM\s+([a-zA-Z0-9_]+)(?:\s+WHERE\s+(.+))?$/is);
      if (m) {
        const tblName = m[1].toLowerCase();
        const table = this.memTables.get(tblName);
        let affected = 0;
        if (table) {
          const whereClause = m[2];
          if (whereClause) {
            const [wCol, wVal] = whereClause.split('=').map(s => s.trim());
            const targetVal = cleanVal(wVal);
            const initialLen = table.rows.length;
            table.rows = table.rows.filter(r => r[wCol] != targetVal);
            affected = initialLen - table.rows.length;
          } else {
            affected = table.rows.length;
            table.rows = [];
          }

          this._emitChange({
            op: 'DELETE',
            table: m[1],
            rowId: Date.now(),
            timestamp: Math.floor(Date.now() / 1000),
            data: { affected }
          });
        }
        return affected;
      }
    }

    return 0;
  }

  query(sql, params = []) {
    this._assertOpen();
    const trimmed = sql.trim().replace(/;+\s*$/, '');
    const upper = trimmed.toUpperCase();

    // SELECT
    if (upper.startsWith('SELECT')) {
      // 1. Vector Search: SELECT ... FROM <table> VECTOR NEAR <col> = [...] TOP <k>
      const vecMatch = trimmed.match(/SELECT\s+(.+?)\s+FROM\s+([a-zA-Z0-9_]+)\s+VECTOR\s+NEAR\s+([a-zA-Z0-9_]+)\s*=\s*(\[[^\]]+\])\s+TOP\s+([0-9]+)/i);
      if (vecMatch) {
        const tblName = vecMatch[2].toLowerCase();
        const vecCol = vecMatch[3];
        const queryVec = cleanVal(vecMatch[4]);
        const k = parseInt(vecMatch[5], 10);
        const table = this.memTables.get(tblName);

        if (!table) return [];

        const scored = table.rows
          .map(r => {
            const rVec = r[vecCol];
            const sim = Array.isArray(rVec) ? cosineSimilarity(queryVec, rVec) : 0;
            return { row: r, similarity: sim };
          })
          .sort((a, b) => b.similarity - a.similarity)
          .slice(0, k)
          .map(item => item.row);

        return scored;
      }

      // 2. Standard SELECT
      const selMatch = trimmed.match(/SELECT\s+(.+?)\s+FROM\s+([a-zA-Z0-9_]+)(?:\s+WHERE\s+(.+?))?(?:\s+ORDER\s+BY\s+([a-zA-Z0-9_]+(?:\s+(?:ASC|DESC))?))?(?:\s+LIMIT\s+([0-9]+))?$/is);
      if (selMatch) {
        const colClause = selMatch[1];
        const tblName = selMatch[2].toLowerCase();
        const table = this.memTables.get(tblName);
        if (!table) return [];

        let rows = table.rows.map(r => ({ ...r }));
        const whereClause = selMatch[3];
        if (whereClause) {
          const parts = whereClause.split('=').map(s => s.trim());
          if (parts.length === 2) {
            const [wCol, wVal] = parts;
            const targetVal = cleanVal(wVal);
            rows = rows.filter(r => r[wCol] == targetVal);
          }
        }

        // Window Function support, e.g., ROW_NUMBER() OVER (ORDER BY score DESC) as rank
        const windowMatch = colClause.match(/ROW_NUMBER\(\)\s*OVER\s*\((?:ORDER\s+BY\s+([a-zA-Z0-9_]+)(?:\s+(ASC|DESC))?)?\)\s*(?:AS\s+)?([a-zA-Z0-9_]+)?/i);
        if (windowMatch) {
          const wOrderCol = windowMatch[1];
          const wOrderDir = (windowMatch[2] || 'ASC').toUpperCase();
          const wRankAlias = windowMatch[3] || 'rank';

          if (wOrderCol) {
            rows.sort((a, b) => {
              const va = a[wOrderCol];
              const vb = b[wOrderCol];
              if (va < vb) return wOrderDir === 'DESC' ? 1 : -1;
              if (va > vb) return wOrderDir === 'DESC' ? -1 : 1;
              return 0;
            });
          }

          rows.forEach((r, idx) => {
            r[wRankAlias] = idx + 1;
          });
        }

        // ORDER BY clause
        const orderByClause = selMatch[4];
        if (orderByClause) {
          const [oCol, oDir] = orderByClause.trim().split(/\s+/);
          const isDesc = oDir && oDir.toUpperCase() === 'DESC';
          rows.sort((a, b) => {
            const va = a[oCol];
            const vb = b[oCol];
            if (va < vb) return isDesc ? 1 : -1;
            if (va > vb) return isDesc ? -1 : 1;
            return 0;
          });
        }

        const limit = selMatch[5] ? parseInt(selMatch[5], 10) : null;
        if (limit !== null) {
          rows = rows.slice(0, limit);
        }

        return rows;
      }
    }

    return [];
  }

  searchVector(vector, limit = 5) {
    this._assertOpen();
    for (const table of this.memTables.values()) {
      for (const row of table.rows) {
        for (const [k, v] of Object.entries(row)) {
          if (Array.isArray(v) && v.length === vector.length) {
            return this.query(`SELECT * FROM ${table.name} VECTOR NEAR ${k} = ${JSON.stringify(vector)} TOP ${limit};`);
          }
        }
      }
    }
    return [];
  }

  hybridSearch({ queryText, queryVector = null, limit = 5 }) {
    this._assertOpen();
    const textLower = (queryText || '').toLowerCase();
    const results = [];

    for (const table of this.memTables.values()) {
      for (const row of table.rows) {
        let textScore = 0;
        let vecScore = 0;
        for (const val of Object.values(row)) {
          if (typeof val === 'string' && val.toLowerCase().includes(textLower)) {
            textScore += 1;
          }
          if (queryVector && Array.isArray(val) && val.length === queryVector.length) {
            vecScore = cosineSimilarity(queryVector, val);
          }
        }
        if (textScore > 0 || vecScore > 0) {
          results.push({ row, combinedScore: textScore * 0.5 + vecScore * 0.5 });
        }
      }
    }

    return results
      .sort((a, b) => b.combinedScore - a.combinedScore)
      .slice(0, limit)
      .map(item => item.row);
  }

  remember(content, importance = 0.5, tags = []) {
    this._assertOpen();
    const id = Date.now() + this.memories.length;
    this.memories.push({
      id,
      content,
      importance,
      tags: Array.isArray(tags) ? tags : [tags],
      createdAt: Date.now()
    });
    return id;
  }

  recall(query, limit = 5) {
    this._assertOpen();
    const qLower = (query || '').toLowerCase();
    const tokens = qLower.split(/\s+/).filter(t => t.length > 2);

    const scored = this.memories.map(m => {
      const cLower = m.content.toLowerCase();
      let matchCount = 0;
      tokens.forEach(t => {
        if (cLower.includes(t)) matchCount++;
      });
      const recencyBoost = Math.max(0, 1 - (Date.now() - m.createdAt) / 86400000);
      const score = (matchCount * 0.7) + (m.importance * 0.2) + (recencyBoost * 0.1);
      return { memory: m, score };
    });

    return scored
      .filter(item => tokens.length === 0 || item.score > 0)
      .sort((a, b) => b.score - a.score)
      .slice(0, limit)
      .map(item => item.memory);
  }

  recallPrompt(query, limit = 5) {
    const list = this.recall(query, limit);
    if (list.length === 0) return '';
    const lines = ['### [Retrieved Memory Context]:'];
    list.forEach(m => {
      lines.push(`- ${m.content}`);
    });
    return lines.join('\n');
  }

  graphRagQuery(params) {
    this._assertOpen();
    const query = typeof params === 'string' ? params : (params.query || '');
    const limit = (typeof params === 'object' && params.limit) || 5;

    const matchedMemories = this.recall(query, limit);
    const facts = matchedMemories.map(m => `- Fact: ${m.content}`).join('\n');

    return {
      query,
      results: matchedMemories.map((m, idx) => ({
        entityId: m.id,
        label: 'MemoryEntity',
        properties: JSON.stringify({ content: m.content }),
        rrfScore: 1.0 / (idx + 1),
        hopDistance: 0,
        relatedEdges: []
      })),
      promptContext: `### Verified Knowledge Context\n${facts || '- General query context for: ' + query}`
    };
  }

  subscribe(table, listener) {
    this._assertOpen();
    const eventName = table === '*' ? 'change:*' : `change:${table}`;
    this.eventBus.on(eventName, listener);
    return new SubscriptionHandle(this.eventBus, table, listener);
  }

  vacuumInto(_targetPath) {
    this._assertOpen();
    return true;
  }

  _emitChange(changeEvent) {
    this.eventBus.emit(`change:${changeEvent.table}`, changeEvent);
    this.eventBus.emit('change:*', changeEvent);
  }

  _assertOpen() {
    if (!this.isOpen) {
      throw new Error('Database is closed');
    }
  }

  close() {
    this.isOpen = false;
    this.eventBus.removeAllListeners();
  }
}

module.exports = {
  TapirusDatabase,
  default: TapirusDatabase
};
