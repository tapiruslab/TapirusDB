/**
 * TapirusDB Official TypeScript SDK (@tapirus/db)
 * High-performance embedded multi-model AI database engine with pure Safe-Rust core.
 */

export type Value = string | number | boolean | null | number[] | Uint8Array;

export interface DatabaseOptions {
  pageSize?: number;
  passphrase?: string;
  autoCheckpoint?: boolean;
}

export interface VectorSearchResult {
  id: number;
  distance: number;
  metadata?: Record<string, any>;
}

export interface HybridSearchParams {
  queryText: string;
  queryVector?: number[];
  limit?: number;
  bm25Weight?: number;
  vectorWeight?: number;
  rrfK?: number;
}

export interface HybridSearchResult {
  id: number;
  score: number;
  content: string;
  metadata?: Record<string, any>;
}

export interface GraphRagParams {
  query: string;
  queryVector?: number[];
  topSeeds?: number;
  maxHops?: number;
  limit?: number;
  vectorWeight?: number;
  graphWeight?: number;
  lexicalWeight?: number;
  rrfK?: number;
}

export interface GraphRagEntity {
  entityId: number;
  label: string;
  properties: string;
  rrfScore: number;
  hopDistance: number;
  seedSimilarity?: number;
  relatedEdges?: Array<{
    id: number;
    fromId: number;
    toId: number;
    label: string;
    weight: number;
  }>;
}

export interface GraphRagContext {
  query: string;
  results: GraphRagEntity[];
  promptContext: string;
}

export type ChangeOp = 'INSERT' | 'UPDATE' | 'DELETE';

export interface ChangeEvent {
  op: ChangeOp;
  table: string;
  rowId: number;
  timestamp: number;
  data: Record<string, any>;
}

export interface SubscriptionHandle {
  unsubscribe(): void;
}

export class TapirusDatabase {
  /**
   * Open or create an embedded TapirusDB database file (.tapir).
   */
  static open(path: string, options?: DatabaseOptions): Promise<TapirusDatabase>;

  /**
   * Open an ultra-fast transient in-memory TapirusDB database.
   */
  static openInMemory(options?: DatabaseOptions): Promise<TapirusDatabase>;

  /**
   * Return TapirusDB engine version string.
   */
  static version(): string;

  /**
   * Execute a non-query SQL command (CREATE, INSERT, UPDATE, DELETE, BEGIN, COMMIT).
   * Returns the count of affected rows.
   */
  execute(sql: string, params?: Value[]): Promise<number>;

  /**
   * Execute a SQL query and return structured rows.
   */
  query<T = Record<string, any>>(sql: string, params?: Value[]): Promise<T[]>;

  /**
   * Perform high-speed HNSW dense vector cosine similarity search.
   */
  searchVector(vector: number[], limit?: number): Promise<VectorSearchResult[]>;

  /**
   * Perform hybrid search fusing BM25 lexical inverted index and vector ranks via RRF.
   */
  hybridSearch(params: HybridSearchParams): Promise<HybridSearchResult[]>;

  /**
   * Execute an accelerated GraphRAG query combining PQ vector seeding, micro-hop graph traversal, and tri-modal RRF.
   */
  graphRagQuery(params: GraphRagParams | string): Promise<GraphRagContext>;

  /**
   * Subscribe to real-time table mutations (Reactive Live Queries / Change Data Capture).
   */
  subscribe(table: string, listener: (change: ChangeEvent) => void): SubscriptionHandle;

  /**
   * Execute a non-blocking hot online backup to target destination file.
   */
  vacuumInto(targetPath: string): Promise<void>;

  /**
   * Close the database connection and release resources.
   */
  close(): void;
}

export default TapirusDatabase;
