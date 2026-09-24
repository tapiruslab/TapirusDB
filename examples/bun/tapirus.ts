import { dlopen, FFIType, ptr, CString } from "bun:ffi";

// Candidate paths for libtapirus shared library
const candidatePaths = [
  process.env.TAPIRUS_LIB || "",
  `${process.env.HOME}/.tapirusdb-target/release/libtapirus.so`,
  "./libtapirus.so",
  "../../target/release/libtapirus.so",
  "../../target/release/tapirus.dll",
];

let libPath = "libtapirus.so";
for (const p of candidatePaths) {
  if (p && (await Bun.file(p).exists())) {
    libPath = p;
    break;
  }
}

const { symbols } = dlopen(libPath, {
  tapirus_version: {
    args: [],
    returns: FFIType.cstring,
  },
  tapirus_open_in_memory: {
    args: [],
    returns: FFIType.ptr,
  },
  tapirus_open: {
    args: [FFIType.cstring],
    returns: FFIType.ptr,
  },
  tapirus_open_encrypted: {
    args: [FFIType.cstring, FFIType.cstring],
    returns: FFIType.ptr,
  },
  tapirus_close: {
    args: [FFIType.ptr],
    returns: FFIType.void,
  },
  tapirus_execute: {
    args: [FFIType.ptr, FFIType.cstring, FFIType.ptr],
    returns: FFIType.i32,
  },
  tapirus_query_json: {
    args: [FFIType.ptr, FFIType.cstring, FFIType.ptr, FFIType.ptr],
    returns: FFIType.i32,
  },
  tapirus_free_string: {
    args: [FFIType.ptr],
    returns: FFIType.void,
  },
  tapirus_checkpoint: {
    args: [FFIType.ptr],
    returns: FFIType.i64,
  },
});

/**
 * Idiomatic Bun client for TapirusDB using zero-overhead native FFI.
 */
export class Tapirus {
  private handle: any;

  constructor(path?: string, passphrase?: string) {
    if (!path || path === ":memory:") {
      this.handle = symbols.tapirus_open_in_memory();
    } else if (passphrase) {
      const p = Buffer.from(path + "\0");
      const pass = Buffer.from(passphrase + "\0");
      this.handle = symbols.tapirus_open_encrypted(ptr(p), ptr(pass));
    } else {
      const p = Buffer.from(path + "\0");
      this.handle = symbols.tapirus_open(ptr(p));
    }

    if (!this.handle) {
      throw new Error(`Failed to open TapirusDB database at: ${path}`);
    }
  }

  static version(): string {
    return symbols.tapirus_version();
  }

  execute(sql: string): number {
    const sqlBuf = Buffer.from(sql + "\0");
    const errPtr = new BigUint64Array(1);
    const affected = symbols.tapirus_execute(this.handle, ptr(sqlBuf), ptr(errPtr));
    if (affected < 0) {
      throw new Error(`TapirusDB execute failed: ${sql}`);
    }
    return affected;
  }

  query<T = any>(sql: string): T[] {
    const sqlBuf = Buffer.from(sql + "\0");
    const jsonPtr = new BigUint64Array(1);
    const errPtr = new BigUint64Array(1);

    const res = symbols.tapirus_query_json(this.handle, ptr(sqlBuf), ptr(jsonPtr), ptr(errPtr));
    if (res !== 0) {
      throw new Error(`TapirusDB query failed: ${sql}`);
    }

    const addr = jsonPtr[0];
    if (addr === 0n) return [];

    const cstr = new CString(Number(addr));
    const jsonStr = cstr.toString();
    symbols.tapirus_free_string(addr);

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
      symbols.tapirus_close(this.handle);
      this.handle = null;
    }
  }
}
