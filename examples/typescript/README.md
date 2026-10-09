# 🦛 TapirusDB on TypeScript & Node.js

Run TapirusDB in Node.js and TypeScript via fast C ABI dynamic linking or WebAssembly.

## 🚀 Features
- **In-Process Speed:** Zero IPC or network daemon latency.
- **Quad-Model API:** Run SQL, JSON Documents, Vectors, and Knowledge Graphs from TypeScript.
- **Single-File Container:** Entire database stored in `app.tapir` with crash-safe WAL.

## 🛠️ Requirements & Building

Ensure the native shared library is compiled:
```bash
cargo build --release -p tapirus-ffi
```

Install dependencies:
```bash
npm install
```

## 💻 Quick Usage (`index.ts`)

```typescript
// See index.ts for full working implementation:
const db = new Tapirus(":memory:");

// 1. Relational SQL & AI Vectors
db.execute("CREATE TABLE users (id INT PRIMARY KEY, name TEXT, embedding VECTOR(3));");
db.execute("INSERT INTO users VALUES (1, 'Ada Lovelace', [0.1, 0.9, 0.0]);");

// 2. Query Rows
const rows = db.query("SELECT id, name FROM users;");
console.log("Query Results:", rows);

// 3. Vector Similarity Search
const nearest = db.query("SELECT id, name FROM users VECTOR NEAR embedding = [0.15, 0.85, 0.0] TOP 1;");
console.log("Nearest Vector Neighbor:", nearest);
```

## 🏃 Running the Example

```bash
npm start
```
