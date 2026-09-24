import koffi from "koffi";
import path from "path";
import os from "os";

// Find native library
const candidatePaths = [
  process.env.TAPIRUS_LIB || "",
  path.join(os.homedir(), ".tapirusdb-target/release/libtapirus.so"),
  path.join(__dirname, "../../target/release/libtapirus.so"),
  path.join(__dirname, "../../target/release/tapirus.dll"),
  "libtapirus.so",
];

let libPath = "libtapirus.so";
for (const p of candidatePaths) {
  try {
    if (p && require("fs").existsSync(p)) {
      libPath = p;
      break;
    }
  } catch (_) {}
}

const lib = koffi.load(libPath);

// Define C ABI types
const TapirusConn = koffi.opaque("TapirusConn");
const tapirus_version = lib.func("const char* tapirus_version()");
const tapirus_open = lib.func("TapirusConn* tapirus_open(const char*)");
const tapirus_open_in_memory = lib.func("TapirusConn* tapirus_open_in_memory()");
const tapirus_open_encrypted = lib.func("TapirusConn* tapirus_open_encrypted(const char*, const char*)");
const tapirus_close = lib.func("void tapirus_close(TapirusConn*)");
const tapirus_execute = lib.func("int32_t tapirus_execute(TapirusConn*, const char*, _Out_ char**)");
const tapirus_query_json = lib.func("int32_t tapirus_query_json(TapirusConn*, const char*, _Out_ char**, _Out_ char**)");
const tapirus_free_string = lib.func("void tapirus_free_string(char*)");

class Tapirus {
  private handle: any;

  constructor(filePath?: string, passphrase?: string) {
    if (!filePath || filePath === ":memory:") {
      this.handle = tapirus_open_in_memory();
    } else if (passphrase) {
      this.handle = tapirus_open_encrypted(filePath, passphrase);
    } else {
      this.handle = tapirus_open(filePath);
    }

    if (!this.handle) {
      throw new Error(`Failed to open TapirusDB database at: ${filePath}`);
    }
  }

  static version(): string {
    return tapirus_version();
  }

  execute(sql: string): number {
    const errPtr: string[] = [null as any];
    const affected = tapirus_execute(this.handle, sql, errPtr);
    if (affected < 0) {
      const errMsg = errPtr[0] || "Unknown error";
      if (errPtr[0]) tapirus_free_string(errPtr[0] as any);
      throw new Error(`Execute failed: ${errMsg}`);
    }
    return affected;
  }

  query<T = any>(sql: string): T[] {
    const jsonPtr: string[] = [null as any];
    const errPtr: string[] = [null as any];
    const res = tapirus_query_json(this.handle, sql, jsonPtr, errPtr);
    if (res !== 0) {
      const errMsg = errPtr[0] || "Unknown error";
      if (errPtr[0]) tapirus_free_string(errPtr[0] as any);
      throw new Error(`Query failed: ${errMsg}`);
    }

    if (!jsonPtr[0]) return [];

    const jsonStr = jsonPtr[0];
    tapirus_free_string(jsonPtr[0] as any);

    const rawRows = JSON.parse(jsonStr);
    return rawRows.map((r: any) => {
      const cols = r.columns || [];
      const vals = r.values || [];
      const row: any = {};
      cols.forEach((c: string, i: number) => {
        const v = vals[i];
        row[c] = typeof v === "object" && v !== null ? Object.values(v)[0] : v;
      });
      return row;
    });
  }

  close(): void {
    if (this.handle) {
      tapirus_close(this.handle);
      this.handle = null;
    }
  }
}

// Runnable demonstration
console.log(`🚀 TapirusDB Native Engine v${Tapirus.version()} on Node.js / TypeScript\n`);

const db = new Tapirus(":memory:");

try {
  db.execute("CREATE TABLE users (id INT PRIMARY KEY, name TEXT, embedding VECTOR(3));");
  db.execute("INSERT INTO users VALUES (1, 'Ada Lovelace', [0.1, 0.9, 0.0]);");
  db.execute("INSERT INTO users VALUES (2, 'Alan Turing', [0.8, 0.2, 0.0]);");

  const rows = db.query("SELECT id, name FROM users;");
  console.log("📦 Query Results:", rows);

  const nearest = db.query("SELECT id, name FROM users VECTOR NEAR embedding = [0.15, 0.85, 0.0] TOP 1;");
  console.log("⚡ Nearest Vector Neighbor:", nearest);
} finally {
  db.close();
  console.log("\n✓ Connection closed safely.");
}
