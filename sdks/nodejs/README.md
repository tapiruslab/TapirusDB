# TapirusDB Node.js & TypeScript SDK (`tapirus`)

Official Node.js & TypeScript client for **TapirusDB** — the 100% Safe-Rust Embedded Quad-Model AI Database Engine.

---

## ⚡ Installation

Install locally:
```bash
npm install ./sdks/nodejs
```

Or once published to npm:
```bash
npm install tapirus
```

---

## 🚀 Quickstart (JavaScript & TypeScript)

```typescript
import { open } from 'tapirus';

// 1. Open database connection (in-memory or single file)
const db = open(':memory:');

// 2. Execute DDL & DML
db.execute(`
  CREATE TABLE memories (
    id INTEGER PRIMARY KEY,
    content TEXT,
    importance REAL
  );
`);

db.execute(`
  INSERT INTO memories VALUES (1, 'User prefers dark mode and Rust', 0.95);
`);

// 3. Query
const rows = db.query('SELECT * FROM memories;');
console.log(rows);

// 4. Clean up
db.close();
```
