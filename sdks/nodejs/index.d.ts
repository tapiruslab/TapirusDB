export interface TapirusConfig {
  path?: string;
  passphrase?: string | null;
}

export interface GraphAlgorithmOptions {
  damping?: number;
  iterations?: number;
  tolerance?: number;
  normalized?: boolean;
  [key: string]: any;
}

export class TapirusConnection {
  path: string;
  passphrase: string | null;
  isClosed: boolean;

  constructor(config?: string | TapirusConfig);

  /**
   * Execute a non-query SQL command (CREATE TABLE, INSERT, UPDATE, DELETE).
   * @param sql SQL statement
   */
  execute(sql: string): number;

  /**
   * Execute a SQL query and return rows as an array of JSON objects.
   * @param sql SQL query string
   */
  query<T = Record<string, any>>(sql: string): T[];

  /**
   * Perform sub-millisecond vector similarity search using HNSW / IVF / Flat indexing.
   */
  vectorSearch<T = Record<string, any>>(
    table: string,
    vectorCol: string,
    queryVector: number[],
    topK?: number,
    where?: string | null
  ): T[];

  /**
   * Execute a Cypher/GQL-Lite pattern match or graph traversal query.
   */
  graphQuery<T = Record<string, any>>(cypherOrSql: string): T[];

  /**
   * Execute a native graph algorithm (PAGERANK, CONNECTED_COMPONENTS, BETWEENNESS, LOUVAIN).
   */
  graphAlgorithm<T = Record<string, any>>(
    algorithm: 'PAGERANK' | 'CONNECTED_COMPONENTS' | 'BETWEENNESS' | 'LOUVAIN' | string,
    options?: GraphAlgorithmOptions
  ): T[];

  /**
   * Close the database connection.
   */
  close(): void;
}

export function open(pathOrConfig?: string | TapirusConfig): TapirusConnection;

export const Tapirus: typeof TapirusConnection;

export default {
  open,
  Tapirus: TapirusConnection,
  TapirusConnection,
};
