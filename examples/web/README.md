# 🌐 TapirusDB for Web Developers (HTML, CSS & JavaScript)

This guide demonstrates how web developers can use **TapirusDB** in standard **HTML + CSS + JavaScript** websites.

There are **two primary architectures** depending on your application needs:

---

## 🏗️ Architecture 1: Client-Server Web App (Shared Database)

**Best for**: Multi-user websites, e-commerce stores, user portals, and collaborative applications where everyone accesses the same database file (`app.tapir`).

### How It Works
- **Frontend**: Pure standard HTML5, CSS3, and modern `fetch()` JavaScript. No frameworks or complex build tooling required.
- **Backend**: A minimal 40-line microservice (`server.js`) running Node.js, Bun, Python, Go, or PHP that interacts directly with TapirusDB via FFI and exposes REST endpoints.

### Quickstart
1. Ensure the shared library is compiled (optional for native FFI mode):
   ```bash
   cargo build --release
   ```
2. Start the lightweight web server:
   ```bash
   cd examples/web
   node server.js
   ```
3. Open your browser at `http://localhost:3005`. You will experience the modern enterprise landing page, featuring live Quad-Model query execution, Tapisaurus planetary architecture specifications, benchmarks, and multi-language developer guides!

### Frontend JavaScript Example (`app.js`):
```javascript
// 1. Query rows from TapirusDB
const res = await fetch('/api/sql', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ sql: "SELECT * FROM items;" })
});
const { rows } = await res.json();
console.log(rows); // [{ id: 1, name: 'Laptop Pro', price: 1299.99 }]

// 2. Insert new record
await fetch('/api/sql', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ sql: "INSERT INTO items VALUES (3, 'Headphones', 149.00);" })
});
```

---

## ⚡ Architecture 2: Pure Browser / WebAssembly (Zero Backend, $0 Hosting)

**Best for**: Offline-first web applications, personal productivity tools, local AI search (e.g. `@xenova/transformers` + vector search), browser extensions, or private local-only dashboards.

### How It Works
- The entire Quad-Model engine runs **100% inside the visitor's web browser** using WebAssembly (`crates/tapirus-wasm`).
- No server required. Can be hosted for free on GitHub Pages, Netlify, or Vercel.
- Data never leaves the user's computer.

### Frontend HTML Example:
```html
<!DOCTYPE html>
<html>
<head>
  <title>Local TapirusDB</title>
</head>
<body>
  <h1>🦛 In-Browser TapirusDB</h1>
  <ul id="items"></ul>

  <script type="module">
    import init, { TapirusWasm } from './tapirus_wasm.js';

    async function start() {
      // 1. Initialize WASM module in browser
      await init();
      const db = new TapirusWasm();

      // 2. Execute SQL directly inside client RAM
      db.execute("CREATE TABLE notes (id INT PRIMARY KEY, text TEXT);");
      db.execute("INSERT INTO notes VALUES (1, 'Finish thesis'), (2, 'Deploy AI agent');");

      // 3. Query results as JSON
      const notes = JSON.parse(db.query_json("SELECT * FROM notes;"));
      
      // 4. Render to DOM
      const list = document.getElementById("items");
      notes.forEach(note => {
        const li = document.createElement("li");
        li.innerText = note.text;
        list.appendChild(li);
      });
    }

    start();
  </script>
</body>
</html>
```

---

## 📊 Comparison: Which Should You Choose?

| Requirement | Architecture 1 (Client-Server REST) | Architecture 2 (Browser WASM) |
|---|---|---|
| **Multi-user shared data** | ✅ Yes (Single shared `.tapir` file) | ❌ No (Isolated per-browser) |
| **Server cost** | Low (Single microservice) | **$0.00 (Zero backend)** |
| **Offline functionality** | Requires network | ✅ 100% Offline (PWA compatible) |
| **Data privacy** | Encrypted on server | ✅ Data never leaves user's machine |
| **Setup complexity** | Minimal (1 small script) | Single `<script type="module">` |
