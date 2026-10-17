export type Value = string | number | bigint | boolean | null | number[] | Uint8Array;

export interface DatabaseOptions {
  passphrase?: string;
  memory?: MemoryConfig;
}

export interface ChangeEvent {
  op: 'Insert' | 'Update' | 'Delete';
  table: string;
  row_id: number;
  timestamp: number;
  data: unknown;
}

export interface Subscription {
  unsubscribe(): boolean;
}

export interface MemoryDocument {
  content: string;
  embedding?: number[];
  importance?: number;
  tags?: string[];
  namespace?: string;
  sessionId?: string;
  externalId?: string;
  sourceVersion?: string;
  embeddingModel?: string;
  expiresAt?: number;
  ttlSeconds?: number;
  expectedRevision?: number;
}

export interface MemorySearch {
  query?: string;
  embedding?: number[];
  limit?: number;
  namespace?: string;
  sessionId?: string;
  tags?: string[];
  embeddingModel?: string;
  candidateLimit?: number;
  fusion?: 'rrf' | 'weighted';
  rrfK?: number;
  vectorWeight?: number;
  lexicalWeight?: number;
  recencyWeight?: number;
  importanceWeight?: number;
  halfLifeSeconds?: number;
  minScore?: number;
  exact?: boolean;
}

export interface MemoryConfig {
  maxDocuments?: number;
  maxEstimatedBytes?: number;
  maxDocumentBytes?: number;
  maxVectorDimensions?: number;
  maxVectorSpaces?: number;
  maxBatchDocuments?: number;
  maxCandidates?: number;
  hnswM?: number;
  efConstruction?: number;
  efSearch?: number;
}
export interface MemoryStats {
  documents: number;
  estimated_resident_bytes: number;
  max_estimated_bytes: number;
  vector_spaces: number;
  indexed_vectors: number;
  pending_index_updates: number;
}
export interface MemoryDeleteScope {
  namespace?: string;
  sessionId?: string;
  externalId?: string;
  sourceVersion?: string;
  exceptSourceVersion?: string;
  tags?: string[];
  all?: boolean;
}
export interface MemorySearchResponse {
  results: MemoryResult[];
  diagnostics: {
    total_documents: number; candidate_limit: number; lexical_candidates: number;
    vector_candidates: number; scored_candidates: number; exact_vector_search: boolean;
    vector_indexes_used: number; fusion: 'rrf' | 'weighted';
  };
}

export interface StorageHealth {
  reopen_required: boolean;
  checkpoint_error: string | null;
  wal_frames: number;
}

export interface MemoryResult {
  entry: { id: number; content: string; vector: number[] | null; tags: string[]; namespace: string | null; session_id: string | null;
    external_id: string | null; source_version: string | null; embedding_model: string | null;
    expires_at: number | null; revision: number; updated_at: number; content_hash: string };
  combined_score: number;
  semantic_score: number;
  lexical_score: number;
  recency_score: number;
  is_associative: boolean;
}

export class TapirusDatabase {
  constructor(filePath?: string, options?: DatabaseOptions);
  static open(path?: string, options?: DatabaseOptions): TapirusDatabase;
  static openInMemory(options?: DatabaseOptions): TapirusDatabase;
  static version(): string;
  readonly isNative: true;
  readonly isOpen: boolean;
  readonly path: string;
  execute(sql: string, params?: Value[]): number;
  query<T = Record<string, unknown>>(sql: string, params?: Value[]): T[];
  vectorSearch<T = Record<string, unknown>>(table: string, vectorColumn: string, vector: number[], limit?: number): T[];
  graphAlgorithm(algorithm: string): Record<string, unknown>[];
  tapClassifyDetailed(text: string, candidates: string[]): Record<string, unknown>;
  tapClassify(text: string, candidates: string[]): string;
  tapVerify(premise: string, hypothesis: string, threshold?: number): boolean;
  tapScore(text: string, criteria: string): number;
  tapRoute(state: string, routes: string[]): string;
  remember(text: string, importance?: number, tags?: string[]): number;
  storeMemory(document: MemoryDocument): number;
  upsertMemory(document: MemoryDocument): number;
  storeMemoryBatch(documents: MemoryDocument[]): number[];
  upsertMemoryBatch(documents: MemoryDocument[]): number[];
  searchMemories(search: MemorySearch): MemoryResult[];
  searchMemoriesDetailed(search: MemorySearch): MemorySearchResponse;
  configureMemory(config: MemoryConfig): MemoryStats;
  memoryStats(): MemoryStats;
  storageHealth(): StorageHealth;
  prepareMemoryIndexes(): MemoryStats;
  purgeExpiredMemories(): number;
  deleteMemories(scope: MemoryDeleteScope): number;
  forgetMemory(id: number): boolean;
  recall(query: string, limit?: number): Record<string, unknown>[];
  recallPrompt(query: string, limit?: number): string;
  graphRagQuery(options: { query: string; limit?: number }): Record<string, unknown>;
  subscribe(table: string, listener: (change: ChangeEvent) => void): Subscription;
  checkpoint(): number;
  vacuumInto(targetPath: string): number;
  close(): void;
}

export function open(filePath?: string, options?: DatabaseOptions): TapirusDatabase;
export function version(): string;
export const Tapirus: typeof TapirusDatabase;

/** Sessions serialize access; a transaction belongs to one session. */
export class TapirusSessionDatabase {
  constructor(filePath?: string, options?: DatabaseOptions);
  session(options?: { busyTimeoutMs?: number }): TapirusSession;
  close(): void;
}
export interface TapirusSession extends Pick<TapirusDatabase,
  'execute' | 'query' | 'storeMemory' | 'upsertMemory' | 'storeMemoryBatch' |
  'upsertMemoryBatch' | 'searchMemories' | 'searchMemoriesDetailed' |
  'configureMemory' | 'memoryStats' | 'storageHealth' | 'prepareMemoryIndexes' |
  'purgeExpiredMemories' | 'deleteMemories' | 'forgetMemory' | 'checkpoint' | 'close'> {
  begin(): void;
  commit(): void;
  rollback(): void;
  /** The callback must be synchronous. */
  transaction<T>(callback: (session: TapirusSession) => T extends PromiseLike<unknown> ? never : T): T;
}

declare const _default: {
  open: typeof open;
  version: typeof version;
  Tapirus: typeof TapirusDatabase;
  TapirusDatabase: typeof TapirusDatabase;
  TapirusSessionDatabase: typeof TapirusSessionDatabase;
};
export default _default;
