import { dlopen, FFIType, ptr, CString } from "bun:ffi";
import { fileURLToPath } from "node:url";

// Candidate paths for libtapirus shared library
const candidatePaths = process.env.TAPIRUS_LIB ? [process.env.TAPIRUS_LIB] : [
  `${process.env.HOME}/.tapirusdb-target/release/libtapirus.so`,
  "./libtapirus.so",
  ...['release', 'debug'].flatMap(profile => ['libtapirus.so', 'tapirus.dll', 'libtapirus.dylib']
    .map(library => fileURLToPath(new URL(`../../target/${profile}/${library}`, import.meta.url)))),
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

  private assertOpen(): void {
    if (!this.handle) throw new Error('Database connection is closed');
  }

  private takeString(address: bigint): string {
    if (address === 0n) return '';
    try { return new CString(Number(address)).toString(); }
    finally { symbols.tapirus_free_string(Number(address)); }
  }

  constructor(path?: string, passphrase?: string) {
    if (path?.includes('\0') || passphrase?.includes('\0')) throw new Error('Path and passphrase must not contain NUL bytes');
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
    this.assertOpen();
    if (sql.includes('\0')) throw new Error('SQL must not contain NUL bytes');
    const sqlBuf = Buffer.from(sql + "\0");
    const errPtr = new BigUint64Array(1);
    const affected = symbols.tapirus_execute(this.handle, ptr(sqlBuf), ptr(errPtr));
    if (affected < 0) {
      throw new Error(this.takeString(errPtr[0]) || 'TapirusDB execute failed');
    }
    return affected;
  }

  query<T = any>(sql: string): T[] {
    this.assertOpen();
    if (sql.includes('\0')) throw new Error('SQL must not contain NUL bytes');
    const sqlBuf = Buffer.from(sql + "\0");
    const jsonPtr = new BigUint64Array(1);
    const errPtr = new BigUint64Array(1);

    const res = symbols.tapirus_query_json(this.handle, ptr(sqlBuf), ptr(jsonPtr), ptr(errPtr));
    if (res !== 0) {
      throw new Error(this.takeString(errPtr[0]) || 'TapirusDB query failed');
    }

    const addr = jsonPtr[0];
    if (addr === 0n) return [];

    const jsonStr = this.takeString(addr);

    const rawRows = JSON.parse(jsonStr);
    return rawRows.map((r: any) => {
      const cols = r.columns || [];
      const vals = r.values || [];
      const row: any = {};
      cols.forEach((c: string, i: number) => {
        const v = vals[i];
        row[c] = v === 'Null' ? null : typeof v === "object" && v !== null ? Object.values(v)[0] : v;
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
