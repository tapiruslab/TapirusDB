/**
 * TapirusDB Official Node.js & TypeScript SDK
 */

export interface TapirusConfig {
  path?: string;
  passphrase?: string;
}

export interface QueryResult<T = Record<string, any>> {
  rows: T[];
  affected?: number;
}

export class TapirusConnection {
  constructor(config?: TapirusConfig | string);

  /**
   * Execute a non-query SQL command (CREATE, INSERT, UPDATE, DELETE).
   * Returns the number of affected rows.
   */
  execute(sql: string): number;

  /**
   * Execute a SQL query and return rows.
   */
  query<T = Record<string, any>>(sql: string): T[];

  /**
   * Close the connection and release resources.
   */
  close(): void;
}

export class Tapirus extends TapirusConnection {}

/**
 * Open a connection to TapirusDB.
 */
export function open(pathOrConfig?: string | TapirusConfig): TapirusConnection;

export default {
  open,
  Tapirus,
  TapirusConnection,
};
