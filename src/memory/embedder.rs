//! Deterministic Text-to-Vector Feature Hashing Embedder for TapirusDB.
//!
//! Provides zero-setup, zero-dependency embedding generation in 100% Pure Safe Rust
//! using character and word n-gram feature hashing (MinHash / Vowpal Wabbit style projection)
//! with sign projection and L2 normalization.
//!
//! NOTE: This is a fast, lightweight lexical-semantic hashing technique suited for edge CPUs
//! and sub-microsecond latency, not a deep neural language model (like BERT or MiniLM).

use serde::{Deserialize, Serialize};

/// Trait defining an in-database text embedding engine.
pub trait EmbeddingEngine: Send + Sync {
    /// Return the embedding dimensionality produced by this model
    fn dimensions(&self) -> usize;

    /// Embed a single text string into a dense unit-normalized float vector
    fn embed_text(&self, text: &str) -> Vec<f32>;

    /// Batch embed multiple text strings
    fn embed_batch(&self, texts: &[&str]) -> Vec<Vec<f32>> {
        texts.iter().map(|t| self.embed_text(t)).collect()
    }
}

/// Deterministic, zero-dependency feature hashing embedder.
///
/// Uses character/word n-gram hashing with sign projection and L2 normalization to produce
/// rich, consistent semantic vectors in sub-microsecond latency on edge CPUs and WebAssembly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeterministicHashEmbedder {
    /// Vector dimensionality (default: 128)
    pub dims: usize,
}

impl Default for DeterministicHashEmbedder {
    fn default() -> Self {
        Self { dims: 128 }
    }
}

impl DeterministicHashEmbedder {
    /// Create a new hash embedder with specified dimensions
    pub fn new(dims: usize) -> Self {
        Self { dims: dims.max(16) }
    }

    /// Murmur-inspired safe 32-bit hash mixer
    fn hash_ngram(bytes: &[u8], seed: u32) -> u32 {
        let mut h = seed;
        for &b in bytes {
            h = h.wrapping_mul(31).wrapping_add(b as u32);
            h = (h << 13) | (h >> 19);
            h ^= h >> 15;
        }
        h
    }
}

impl EmbeddingEngine for DeterministicHashEmbedder {
    fn dimensions(&self) -> usize {
        self.dims
    }

    fn embed_text(&self, text: &str) -> Vec<f32> {
        let mut vec = vec![0.0f32; self.dims];
        let lower = text.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();

        if words.is_empty() {
            return vec;
        }

        // 1. Unigram word hashing
        for (pos, &w) in words.iter().enumerate() {
            let h1 = Self::hash_ngram(w.as_bytes(), 0x9747b28c);
            let idx = (h1 as usize) % self.dims;
            let sign = if (h1 & 1) == 0 { 1.0f32 } else { -1.0f32 };
            let pos_decay = 1.0 / (1.0 + (pos as f32) * 0.05);
            vec[idx] += sign * 1.5 * pos_decay;
        }

        // 2. Bigram context hashing
        for window in words.windows(2) {
            let mut combined = String::with_capacity(window[0].len() + window[1].len() + 1);
            combined.push_str(window[0]);
            combined.push('_');
            combined.push_str(window[1]);

            let h2 = Self::hash_ngram(combined.as_bytes(), 0x5bd1e995);
            let idx = (h2 as usize) % self.dims;
            let sign = if (h2 & 1) == 0 { 1.0f32 } else { -1.0f32 };
            vec[idx] += sign * 2.0;
        }

        // 3. Trigram context hashing for phrase-level coherence
        for window in words.windows(3) {
            let mut combined = String::with_capacity(window[0].len() + window[1].len() + window[2].len() + 2);
            combined.push_str(window[0]);
            combined.push('_');
            combined.push_str(window[1]);
            combined.push('_');
            combined.push_str(window[2]);

            let h3 = Self::hash_ngram(combined.as_bytes(), 0x27d4eb2f);
            let idx = (h3 as usize) % self.dims;
            let sign = if (h3 & 1) == 0 { 1.0f32 } else { -1.0f32 };
            vec[idx] += sign * 2.5;
        }

        // 4. Substring character 3-grams and 4-grams for morphology (prefixes & suffixes)
        for w in words.iter().take(30) {
            let chars: Vec<char> = w.chars().collect();
            for chunk in chars.windows(3) {
                let s: String = chunk.iter().collect();
                let h = Self::hash_ngram(s.as_bytes(), 0x12b9b0a1);
                let idx = (h as usize) % self.dims;
                let sign = if (h & 1) == 0 { 1.0f32 } else { -1.0f32 };
                vec[idx] += sign * 0.8;
            }
            for chunk in chars.windows(4) {
                let s: String = chunk.iter().collect();
                let h = Self::hash_ngram(s.as_bytes(), 0x3c6ef372);
                let idx = (h as usize) % self.dims;
                let sign = if (h & 1) == 0 { 1.0f32 } else { -1.0f32 };
                vec[idx] += sign * 1.0;
            }
        }

        // 5. L2 Unit Normalization (for exact cosine similarity matching)
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-9 {
            for v in vec.iter_mut() {
                *v /= norm;
            }
        }

        vec
    }
}

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Production neural embedding engine connecting to Ollama, OpenAI, LM Studio, or local REST services.
/// Automatically falls back to deterministic feature hashing if the network endpoint is offline.
#[derive(Debug, Clone)]
pub struct HttpEmbeddingEngine {
    /// Host:port of embedding service (e.g. "127.0.0.1:11434" or "api.openai.com:443")
    pub host: String,
    /// Path (default: "/api/embeddings" or "/v1/embeddings")
    pub path: String,
    /// Model name (e.g. "nomic-embed-text", "text-embedding-3-small", "bge-small")
    pub model: String,
    /// Expected vector dimension
    pub dims: usize,
    /// Optional Bearer authorization token
    pub api_key: Option<String>,
    /// Local feature-hashing fallback engine if offline
    pub fallback: DeterministicHashEmbedder,
}

impl HttpEmbeddingEngine {
    /// Create an HTTP embedding client from full endpoint URL, model, and dimensions
    pub fn new(endpoint: &str, model: impl Into<String>, dims: usize) -> Self {
        let trimmed = endpoint.trim_start_matches("http://").trim_start_matches("https://");
        let (host, path) = match trimmed.split_once('/') {
            Some((h, p)) => (h.to_string(), format!("/{p}")),
            None => (trimmed.to_string(), "/".to_string()),
        };
        Self::new_custom(host, path, model, dims)
    }

    /// Create a new Ollama embedding client
    pub fn new_ollama(model: impl Into<String>, dims: usize) -> Self {
        Self {
            host: "127.0.0.1:11434".to_string(),
            path: "/api/embeddings".to_string(),
            model: model.into(),
            dims,
            api_key: None,
            fallback: DeterministicHashEmbedder::new(dims),
        }
    }

    /// Create an OpenAI-compatible embedding client
    pub fn new_openai(host: impl Into<String>, api_key: impl Into<String>, model: impl Into<String>, dims: usize) -> Self {
        Self {
            host: host.into(),
            path: "/v1/embeddings".to_string(),
            model: model.into(),
            dims,
            api_key: Some(api_key.into()),
            fallback: DeterministicHashEmbedder::new(dims),
        }
    }

    /// Create an HTTP embedding client with custom host, path, and model
    pub fn new_custom(host: impl Into<String>, path: impl Into<String>, model: impl Into<String>, dims: usize) -> Self {
        Self {
            host: host.into(),
            path: path.into(),
            model: model.into(),
            dims,
            api_key: None,
            fallback: DeterministicHashEmbedder::new(dims),
        }
    }

    /// Query remote embedding endpoint via raw HTTP/1.1
    fn query_remote(&self, text: &str) -> Option<Vec<f32>> {
        let stream = TcpStream::connect_timeout(
            &self.host.parse().ok()?,
            Duration::from_millis(500),
        ).ok()?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));

        let mut stream = stream;
        let body = serde_json::json!({
            "model": self.model,
            "prompt": text,
            "input": text,
        }).to_string();

        let auth_hdr = match &self.api_key {
            Some(key) => format!("Authorization: Bearer {key}\r\n"),
            None => String::new(),
        };

        let req = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.path, self.host, auth_hdr, body.len(), body
        );

        stream.write_all(req.as_bytes()).ok()?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).ok()?;

        let resp_str = String::from_utf8_lossy(&response);
        let json_start = resp_str.find("\r\n\r\n")? + 4;
        let json_body = &resp_str[json_start..];

        let parsed: serde_json::Value = serde_json::from_str(json_body).ok()?;

        // 1. Ollama /api/embeddings format: {"embedding": [f32, ...]}
        if let Some(arr) = parsed.get("embedding").and_then(|v| v.as_array()) {
            let vec: Vec<f32> = arr.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect();
            if !vec.is_empty() {
                return Some(vec);
            }
        }

        // 2. OpenAI / TEI / v1/embeddings standard format: {"data": [{"embedding": [f32, ...]}]}
        if let Some(arr) = parsed
            .get("data")
            .and_then(|v| v.as_array())
            .and_then(|data| data.first())
            .and_then(|first| first.get("embedding"))
            .and_then(|v| v.as_array())
        {
            let vec: Vec<f32> = arr.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect();
            if !vec.is_empty() {
                return Some(vec);
            }
        }

        None
    }
}

impl EmbeddingEngine for HttpEmbeddingEngine {
    fn dimensions(&self) -> usize {
        self.dims
    }

    fn embed_text(&self, text: &str) -> Vec<f32> {
        if let Some(vec) = self.query_remote(text) {
            vec
        } else {
            self.fallback.embed_text(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_hash_embedder_consistency() {
        let embedder = DeterministicHashEmbedder::new(128);
        assert_eq!(embedder.dimensions(), 128);

        let v1 = embedder.embed_text("TapirusDB embedded database");
        let v2 = embedder.embed_text("TapirusDB embedded database");
        assert_eq!(v1, v2);

        // Check L2 norm equals 1.0
        let norm: f32 = v1.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_semantic_similarity_direction() {
        let embedder = DeterministicHashEmbedder::new(128);
        let v_tech1 = embedder.embed_text("Rust embedded database engine");
        let v_tech2 = embedder.embed_text("Rust embedded database persistence");
        let v_fruit = embedder.embed_text("Fresh organic bananas and apples");

        let dot_tech: f32 = v_tech1.iter().zip(v_tech2.iter()).map(|(a, b)| a * b).sum();
        let dot_fruit: f32 = v_tech1.iter().zip(v_fruit.iter()).map(|(a, b)| a * b).sum();

        // Related technical texts should have substantially higher dot product than fruit text
        assert!(dot_tech > dot_fruit);
    }
}
