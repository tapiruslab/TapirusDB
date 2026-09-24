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
import { Database } from "./db";

const db = new Database("app.tapir");

// Execute SQL
db.execute("CREATE TABLE notes (id INT PRIMARY KEY, content TEXT);");
db.execute("INSERT INTO notes VALUES (1, 'Typed edge persistence with TapirusDB!');");

// Query JSON
const rows = db.query("SELECT * FROM notes WHERE id = 1;");
console.log("Query result:", rows);
```

## 🏃 Running the Example

```bash
npm start
```
