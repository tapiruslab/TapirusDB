/**
 * Minimal Zero-Dependency HTTP Backend for TapirusDB Web Client
 * 
 * Works with Node.js (via koffi FFI) or Bun (via bun:ffi)
 * Serves index.html and provides /api/health and /api/sql endpoints.
 */

const http = require("http");
const fs = require("fs");
const path = require("path");

// Locate libtapirus shared library
const isWindows = process.platform === "win32";
const libName = isWindows ? "tapirus.dll" : "libtapirus.so";
const libPaths = [
    path.join(__dirname, "../../target/release", libName),
    path.join(__dirname, "../../target/debug", libName),
    path.join(__dirname, libName)
];

let conn = null;
let tapirus_execute = null;
let tapirus_query_json = null;

try {
    const koffi = require("koffi");
    const libPath = libPaths.find(p => fs.existsSync(p));
    if (libPath) {
        const lib = koffi.load(libPath);
        const tapirus_open_encrypted = lib.func("void* tapirus_open_encrypted(const char* path, const char* pass)");
        tapirus_execute = lib.func("int tapirus_execute(void* conn, const char* sql)");
        tapirus_query_json = lib.func("const char* tapirus_query_json(void* conn, const char* sql)");
        
        const DB_PATH = path.join(__dirname, "web_app.tapir");
        conn = tapirus_open_encrypted(DB_PATH, "web_secret_key");
        console.log(`🦛 TapirusDB Native Engine connected: ${DB_PATH}`);
        tapirus_execute(conn, "CREATE TABLE IF NOT EXISTS items (id INT PRIMARY KEY, name TEXT, price REAL);");
    } else {
        console.warn(`⚠️ Notice: ${libName} not found. Please build using 'cargo build --release --workspace'`);
    }
} catch (e) {
    console.warn("⚠️ Native FFI notice:", e.message);
}

const server = http.createServer((req, res) => {
    // Enable CORS for all origins
    res.setHeader("Access-Control-Allow-Origin", "*");
    res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
    res.setHeader("Access-Control-Allow-Headers", "Content-Type");

    if (req.method === "OPTIONS") {
        res.writeHead(204);
        return res.end();
    }

    // Serve HTML UI
    if (req.method === "GET" && (req.url === "/" || req.url === "/index.html")) {
        const html = fs.readFileSync(path.join(__dirname, "index.html"), "utf8");
        res.writeHead(200, { "Content-Type": "text/html" });
        return res.end(html);
    }

    // Health check endpoint
    if (req.method === "GET" && req.url === "/api/health") {
        res.writeHead(200, { "Content-Type": "application/json" });
        return res.end(JSON.stringify({
            status: "ok",
            engine: conn ? "Native Safe-Rust FFI" : "Not Loaded (Run cargo build --release)"
        }));
    }

    // SQL execution endpoint
    if (req.method === "POST" && req.url === "/api/sql") {
        let body = "";
        req.on("data", chunk => body += chunk);
        req.on("end", () => {
            try {
                if (!conn || !tapirus_query_json || !tapirus_execute) {
                    res.writeHead(503, { "Content-Type": "application/json" });
                    return res.end(JSON.stringify({
                        error: "TapirusDB native library is not loaded. Please build the project with 'cargo build --release --workspace'."
                    }));
                }

                const { sql } = JSON.parse(body);
                const trimmed = sql.trim();

                if (/^select/i.test(trimmed)) {
                    const rawJson = tapirus_query_json(conn, trimmed);
                    const rows = JSON.parse(rawJson || "[]");
                    const cleanRows = rows.map(r => {
                        const out = {};
                        for (const [k, v] of Object.entries(r)) {
                            out[k] = (v && typeof v === "object") ? Object.values(v)[0] : v;
                        }
                        return out;
                    });
                    res.writeHead(200, { "Content-Type": "application/json" });
                    return res.end(JSON.stringify({ rows: cleanRows }));
                } else {
                    const affected = tapirus_execute(conn, trimmed);
                    if (affected < 0) {
                        res.writeHead(400, { "Content-Type": "application/json" });
                        return res.end(JSON.stringify({ error: `Execution failed with code ${affected}` }));
                    }
                    res.writeHead(200, { "Content-Type": "application/json" });
                    return res.end(JSON.stringify({ affected }));
                }
            } catch (err) {
                res.writeHead(500, { "Content-Type": "application/json" });
                return res.end(JSON.stringify({ error: err.message }));
            }
        });
        return;
    }

    res.writeHead(404);
    res.end("Not Found");
});

const PORT = process.env.PORT || 3005;
server.on("error", (err) => {
    if (err.code === "EADDRINUSE") {
        console.error(`❌ Port ${PORT} is in use. Try: PORT=3006 node server.js`);
    } else {
        console.error(err);
    }
});
server.listen(PORT, () => {
    console.log(`🚀 TapirusDB Web Server running at http://localhost:${PORT}`);
    console.log(`👉 Open your browser at http://localhost:${PORT}`);
});
