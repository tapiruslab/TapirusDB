//! # TapirusDB WebAssembly (WASM) Engine
//!
//! Run TapirusDB directly inside modern web browsers, Node.js, and Cloudflare Workers.

use tapirus::{Connection, Row, Value};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn report_panic(message: &str);
}

/// Forward Rust panic details to the host console instead of an opaque WASM trap.
#[wasm_bindgen(start)]
pub fn initialize_runtime() {
    std::panic::set_hook(Box::new(|info| report_panic(&info.to_string())));
}

/// In-memory WebAssembly database connection
#[wasm_bindgen]
pub struct TapirusWasm {
    conn: Connection,
}

fn parameters(json: &str) -> Result<Vec<Value>, JsValue> {
    let values: Vec<serde_json::Value> =
        serde_json::from_str(json).map_err(|error| JsValue::from_str(&error.to_string()))?;
    values
        .iter()
        .map(Value::from_json_parameter)
        .collect::<tapirus::Result<_>>()
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

fn rows_json(rows: Vec<Row>) -> Result<String, JsValue> {
    let rows: Vec<serde_json::Map<String, serde_json::Value>> = rows
        .iter()
        .map(|row| {
            row.columns()
                .iter()
                .zip(row.values())
                .map(|(column, value)| {
                    let value = match value {
                        Value::Null => serde_json::Value::Null,
                        Value::Integer(number) if number.unsigned_abs() > 9_007_199_254_740_991 => {
                            serde_json::json!({"$integer": number.to_string()})
                        }
                        Value::Integer(number) => serde_json::json!(number),
                        Value::Real(number) => serde_json::json!(number),
                        Value::Text(text) => serde_json::json!(text),
                        Value::Blob(bytes) => serde_json::json!({"$blob": bytes}),
                        Value::Vector(vector) => serde_json::json!(vector),
                    };
                    (column.clone(), value)
                })
                .collect()
        })
        .collect();
    serde_json::to_string(&rows).map_err(|error| JsValue::from_str(&error.to_string()))
}

#[wasm_bindgen]
impl TapirusWasm {
    /// Create a new in-memory TapirusDB database instance in WebAssembly
    #[wasm_bindgen(constructor)]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Result<TapirusWasm, JsValue> {
        let conn = Connection::open_in_memory().map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(Self { conn })
    }

    /// Return engine version string
    #[wasm_bindgen]
    pub fn version() -> String {
        tapirus::VERSION.to_string()
    }

    /// Whether an explicit SQL transaction is active (not yet exportable).
    pub fn is_in_transaction(&self) -> bool {
        self.conn.is_in_transaction()
    }

    /// Read the actual engine catalog for browser schema navigation.
    pub fn schema_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.conn.tables())
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Execute a non-query SQL command (CREATE, INSERT, UPDATE, DELETE, BEGIN, COMMIT, etc.)
    #[wasm_bindgen]
    pub fn execute(&self, sql: &str) -> Result<i32, JsValue> {
        self.conn
            .execute(sql)
            .and_then(|affected| {
                i32::try_from(affected).map_err(|_| {
                    tapirus::Error::ConstraintViolation("Affected row count exceeds i32".into())
                })
            })
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Execute a SQL query and return rows as a JSON string
    #[wasm_bindgen]
    pub fn query_json(&self, sql: &str) -> Result<String, JsValue> {
        let rows = self
            .conn
            .query(sql)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        rows_json(rows)
    }

    /// Execute SQL with typed parameters, including vectors, blobs and exact integers.
    pub fn execute_params(&self, sql: &str, params_json: &str) -> Result<usize, JsValue> {
        self.conn
            .execute_with_params(sql, &parameters(params_json)?)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Return plain JSON row objects for parameterized SQL.
    pub fn query_params_json(&self, sql: &str, params_json: &str) -> Result<String, JsValue> {
        rows_json(
            self.conn
                .query_with_params(sql, &parameters(params_json)?)
                .map_err(|error| JsValue::from_str(&error.to_string()))?,
        )
    }

    /// Persist a document with an optional external model embedding and namespace/session.
    pub fn store_memory_json(&self, document_json: &str) -> Result<u64, JsValue> {
        let document = serde_json::from_str::<tapirus::MemoryDocument>(document_json)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        self.conn
            .store_memory(&document)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Hybrid retrieval with source IDs/text and separate lexical and vector scores.
    pub fn search_memories_json(&self, search_json: &str) -> Result<String, JsValue> {
        let search = serde_json::from_str::<tapirus::MemorySearch>(search_json)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let results = self
            .conn
            .search_memories(&search)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_json::to_string(&results).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Source lifecycle, configuration, retrieval diagnostics and storage health.
    pub fn memory_request_json(&self, operation: &str, payload: &str) -> Result<String, JsValue> {
        self.conn
            .memory_request_json(operation, payload)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Delete a persisted source, propagating storage errors.
    pub fn forget_memory(&self, id: u64) -> Result<bool, JsValue> {
        self.conn
            .memory_forget_checked(id)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Store a memory in WASM environment (AI Agent Memory)
    #[wasm_bindgen]
    pub fn memory_remember(
        &self,
        content: &str,
        importance: f32,
        tags_csv: &str,
    ) -> Result<u64, JsValue> {
        let tags: Vec<&str> = tags_csv
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
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
        serde_json::to_string(&results).map_err(|e| JsValue::from_str(&e.to_string()))
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
        serde_json::to_string(&results).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Recall memories via Reciprocal Rank Fusion (RRF) in WASM
    #[wasm_bindgen]
    pub fn memory_recall_rrf_json(
        &self,
        query: &str,
        limit: usize,
        rrf_k: f32,
    ) -> Result<String, JsValue> {
        let results = self.conn.memory_recall_rrf(query, None, limit, rrf_k);
        serde_json::to_string(&results).map_err(|e| JsValue::from_str(&e.to_string()))
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
        serde_json::to_string(&results).map_err(|e| JsValue::from_str(&e.to_string()))
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
        let conn =
            Connection::open_from_bytes(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(Self { conn })
    }

    /// Import a native snapshot with admission limits applied before source loading.
    pub fn import_bytes_with_memory_config(
        data: &[u8],
        config_json: &str,
    ) -> Result<TapirusWasm, JsValue> {
        let memory = serde_json::from_str(config_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid memory configuration: {e}")))?;
        Connection::open_from_bytes_with_config(
            data,
            tapirus::Config {
                memory,
                ..Default::default()
            },
            None,
        )
        .map(|conn| TapirusWasm { conn })
        .map_err(|e| JsValue::from_str(&e.to_string()))
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
    pub fn rabitq_quantize_json(
        &self,
        vector_json: &str,
        num_bits: usize,
    ) -> Result<String, JsValue> {
        let vec: Vec<f32> = serde_json::from_str(vector_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid vector JSON array: {e}")))?;
        let q = self.conn.rabitq_quantize(&vec, num_bits);
        serde_json::to_string(&q).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Classify text using the TAP heuristic engine in WASM.
    #[wasm_bindgen]
    pub fn tap_classify(&self, text: &str, candidates_csv: &str) -> Result<String, JsValue> {
        let candidates: Vec<&str> = candidates_csv
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        let res = self
            .conn
            .tap()
            .classify(text, &candidates)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        serde_json::to_string(&res).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Verify hypothesis against premise using TAP NLI in WASM
    #[wasm_bindgen]
    pub fn tap_verify(&self, premise: &str, hypothesis: &str) -> Result<String, JsValue> {
        let res = self
            .conn
            .tap()
            .verify(premise, hypothesis)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        serde_json::to_string(&res).map_err(|e| JsValue::from_str(&e.to_string()))
    }
}
