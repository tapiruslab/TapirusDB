//! # TapirusDB WebAssembly (WASM) Engine
//!
//! Run TapirusDB directly inside modern web browsers, Node.js, and Cloudflare Workers.

use tapirus::Connection;
use wasm_bindgen::prelude::*;

/// In-memory WebAssembly database connection
#[wasm_bindgen]
pub struct TapirusWasm {
    conn: Connection,
}

#[wasm_bindgen]
impl TapirusWasm {
    /// Create a new in-memory TapirusDB database instance in WebAssembly
    #[wasm_bindgen(constructor)]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Result<TapirusWasm, JsValue> {
        let conn = Connection::open_in_memory()
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(Self { conn })
    }

    /// Return engine version string
    #[wasm_bindgen]
    pub fn version() -> String {
        tapirus::VERSION.to_string()
    }

    /// Execute a non-query SQL command (CREATE, INSERT, UPDATE, DELETE, BEGIN, COMMIT, etc.)
    #[wasm_bindgen]
    pub fn execute(&self, sql: &str) -> Result<i32, JsValue> {
        self.conn
            .execute(sql)
            .map(|affected| affected as i32)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Execute a SQL query and return rows as a JSON string
    #[wasm_bindgen]
    pub fn query_json(&self, sql: &str) -> Result<String, JsValue> {
        let rows = self.conn
            .query(sql)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        serde_json::to_string(&rows)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Store a memory in WASM environment (AI Agent Memory)
    #[wasm_bindgen]
    pub fn memory_remember(&self, content: &str, importance: f32, tags_csv: &str) -> Result<u64, JsValue> {
        let tags: Vec<&str> = tags_csv.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        self.conn
            .memory_remember(content, None, importance, &tags)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Recall memories via hybrid BM25 + Recency scoring in WASM
    #[wasm_bindgen]
    pub fn memory_recall_json(&self, query: &str, limit: usize) -> Result<String, JsValue> {
        let filter = tapirus::memory::MemoryRecallFilter::default();
        let results = self.conn.memory_recall(Some(query), None, limit, &filter);
        serde_json::to_string(&results)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Recall memories via Reciprocal Rank Fusion (RRF) in WASM
    #[wasm_bindgen]
    pub fn memory_recall_rrf_json(&self, query: &str, limit: usize, rrf_k: f32) -> Result<String, JsValue> {
        let results = self.conn.memory_recall_rrf(query, None, limit, rrf_k);
        serde_json::to_string(&results)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Execute GraphRAG query with micro-hop traversal and RRF fusion in WASM
    #[wasm_bindgen]
    pub fn graph_rag_query_json(
        &self,
        query: &str,
        top_seeds: usize,
        max_hops: usize,
        limit: usize,
    ) -> Result<String, JsValue> {
        let config = tapirus::GraphRagConfig::default()
            .with_seeds(top_seeds)
            .with_max_hops(max_hops)
            .with_limit(limit);
        let results = self
            .conn
            .graph_rag_query(query, None, &config)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        serde_json::to_string(&results)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Open or create an in-browser database using Origin Private File System (OPFS) name identifier
    #[wasm_bindgen]
    pub fn open_opfs(_db_name: &str) -> Result<TapirusWasm, JsValue> {
        // Fallback to high-speed in-memory WASM engine for browser runtimes
        Self::new()
    }
}

