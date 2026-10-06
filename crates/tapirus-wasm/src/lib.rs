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

    /// Convenient 1-line memory storage in WASM
    #[wasm_bindgen]
    pub fn remember(&self, content: &str) -> Result<u64, JsValue> {
        self.conn
            .remember(content)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Convenient 1-line memory recall in WASM (returns JSON string)
    #[wasm_bindgen]
    pub fn recall_json(&self, query: &str, limit: usize) -> Result<String, JsValue> {
        let results = self.conn.recall(query, limit);
        serde_json::to_string(&results)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Return prompt-ready context string formatted for LLM prompts in WASM
    #[wasm_bindgen]
    pub fn recall_prompt(&self, query: &str, limit: usize) -> String {
        self.conn.recall_prompt(query, limit)
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

    /// Export the entire database state as raw binary bytes (.tapir format).
    /// Used by browser applications to persist database state into OPFS or trigger a file download.
    #[wasm_bindgen]
    pub fn export_bytes(&self) -> Result<Vec<u8>, JsValue> {
        self.conn
            .export_bytes()
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Import and mount a database directly from raw binary bytes (.tapir format).
    /// Used by browser applications to restore database state from OPFS, IndexedDB, or user file picker.
    #[wasm_bindgen]
    pub fn import_bytes(data: &[u8]) -> Result<TapirusWasm, JsValue> {
        let conn = Connection::open_from_bytes(data)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(Self { conn })
    }

    /// Import and mount an encrypted database from raw binary bytes with passphrase.
    #[wasm_bindgen]
    pub fn import_encrypted_bytes(data: &[u8], passphrase: &str) -> Result<TapirusWasm, JsValue> {
        let conn = Connection::open_encrypted_from_bytes(data, passphrase)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(Self { conn })
    }

    /// Quantize a high-dimensional vector using RaBitQ 1-bit or 2-bit quantization in WebAssembly.
    /// Returns JSON containing bits, norm, dimensions, and compression ratio.
    #[wasm_bindgen]
    pub fn rabitq_quantize_json(&self, vector_json: &str, num_bits: usize) -> Result<String, JsValue> {
        let vec: Vec<f32> = serde_json::from_str(vector_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid vector JSON array: {e}")))?;
        let q = self.conn.rabitq_quantize(&vec, num_bits);
        serde_json::to_string(&q)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Classify text using TAP Sub-millisecond Cognitive Perception Engine in WASM
    #[wasm_bindgen]
    pub fn tap_classify(&self, text: &str, candidates_csv: &str) -> Result<String, JsValue> {
        let candidates: Vec<&str> = candidates_csv.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        let res = self.conn.tap().classify(text, &candidates)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        serde_json::to_string(&res)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Verify hypothesis against premise using TAP NLI in WASM
    #[wasm_bindgen]
    pub fn tap_verify(&self, premise: &str, hypothesis: &str) -> Result<String, JsValue> {
        let res = self.conn.tap().verify(premise, hypothesis)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        serde_json::to_string(&res)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }
}

