/**
 * TapirusDB Node.js & TypeScript SDK (tapirus)
 * Native Safe-Rust NAPI-RS binding with in-memory quad-model fallback.
 */

export type Value = string | number | boolean | null | number[] | Uint8Array;

export interface DatabaseOptions {
  pageSize?: number;
  passphrase?: string;
  autoCheckpoint?: boolean;
}

export class TapirusConnectionWrapper {
  execute(sql: string, params?: Value[]): number | Promise<number>;
  query<T = Record<string, any>>(sql: string, params?: Value[]): T[] | Promise<T[]>;
  vectorSearch<T = Record<string, any>>(table: string, vectorCol: string, queryVec: number[], limit?: number): T[] | Promise<T[]>;
  graphAlgorithm(algo: string, options?: Record<string, any>): Record<string, any>;
  close(): void;
}

export function open(filePath?: string, options?: DatabaseOptions): TapirusConnectionWrapper;

export const Tapirus: typeof TapirusConnectionWrapper;

export class TapirusDatabase {
  static open(path: string, options?: DatabaseOptions): TapirusDatabase;
  static openInMemory(options?: DatabaseOptions): TapirusDatabase;
  static version(): string;
  execute(sql: string, params?: Value[]): number | Promise<number>;
  query<T = Record<string, any>>(sql: string, params?: Value[]): any[] | Promise<any[]>;
  searchVector(vector: number[], limit?: number): any[];
  hybridSearch(params: any): any[];
  graphRagQuery(params: any): any;
  remember(content: string, importance?: number, tags?: string[]): number;
  recall(query: string, limit?: number): any[];
  recallPrompt(query: string, limit?: number): string;
  subscribe(table: string, listener: (change: any) => void): { unsubscribe(): void };
  vacuumInto(targetPath: string): void;
  close(): void;
}

export default {
  open,
  Tapirus,
  TapirusDatabase,
};
