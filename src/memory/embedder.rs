//! Deterministic Text-to-Vector Feature Hashing Embedder for TapirusDB.
//!
//! Provides zero-setup, zero-dependency embedding generation in 100% Pure Safe Rust
//! using character and word n-gram feature hashing (MinHash / Vowpal Wabbit style projection)
//! with sign projection and L2 normalization.
//!
//! NOTE: This is a fast, lightweight lexical-semantic hashing technique suited for edge CPUs
//! rather than a trained neural language model.

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
/// repeatable lexical vectors. Neural semantic quality is not implied.
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
            h = h.rotate_left(13);
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
            let mut combined =
                String::with_capacity(window[0].len() + window[1].len() + window[2].len() + 2);
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

/// Fallible model provider. A failed request never substitutes another vector space.
pub trait EmbeddingProvider: Send + Sync {
    /// Stable identity including model revision and preprocessing version.
    fn model_id(&self) -> String;
    /// Expected dimensionality of every vector.
    fn vector_dimensions(&self) -> usize;
    /// Generate one vector per input, in the same order.
    fn embed_documents(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>>;
}

/// Validate provider output before any database mutation.
pub fn validate_embeddings(vectors: &[Vec<f32>], count: usize, dims: usize) -> crate::Result<()> {
    if dims == 0
        || vectors.len() != count
        || vectors.iter().any(|v| {
            v.len() != dims || v.iter().any(|x| !x.is_finite()) || v.iter().all(|x| *x == 0.0)
        })
    {
        return Err(crate::Error::ConstraintViolation(
            "Embedding response requires the expected count, dimension and finite nonzero vectors"
                .into(),
        ));
    }
    Ok(())
}
impl EmbeddingProvider for DeterministicHashEmbedder {
    fn model_id(&self) -> String {
        format!("tapirus/feature-hash-v1/{}", self.dims)
    }
    fn vector_dimensions(&self) -> usize {
        self.dims
    }
    fn embed_documents(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
        let vectors = EmbeddingEngine::embed_batch(self, texts);
        validate_embeddings(&vectors, texts.len(), self.dims)?;
        Ok(vectors)
    }
}

/// HTTP provider with explicit errors and no embedding fallback.
/// Network support requires `http-embeddings`; native TLS is supplied by ureq.
#[derive(Clone)]
pub struct HttpEmbeddingEngine {
    /// Service authority, without credentials.
    pub host: String,
    /// HTTP request path.
    pub path: String,
    /// Application-pinned model identity sent to the service.
    pub model: String,
    /// Expected vector dimensions.
    pub dims: usize,
    /// Optional bearer token; omitted from Debug output.
    pub api_key: Option<String>,
    tls: bool,
}
impl std::fmt::Debug for HttpEmbeddingEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpEmbeddingEngine")
            .field("host", &self.host)
            .field("path", &self.path)
            .field("model", &self.model)
            .field("dims", &self.dims)
            .field("authenticated", &self.api_key.is_some())
            .finish()
    }
}
impl HttpEmbeddingEngine {
    /// Construct a client from an HTTP(S) endpoint.
    pub fn new(endpoint: &str, model: impl Into<String>, dims: usize) -> Self {
        let tls = endpoint.starts_with("https://");
        let trimmed = endpoint
            .strip_prefix("https://")
            .or_else(|| endpoint.strip_prefix("http://"))
            .unwrap_or(endpoint);
        let (host, path) = trimmed
            .split_once('/')
            .map_or((trimmed, "/".into()), |(h, p)| (h, format!("/{p}")));
        Self {
            host: host.into(),
            path,
            model: model.into(),
            dims,
            api_key: None,
            tls,
        }
    }
    /// Use the local Ollama batch embedding endpoint.
    pub fn new_ollama(model: impl Into<String>, dims: usize) -> Self {
        Self::new("http://127.0.0.1:11434/api/embed", model, dims)
    }
    /// Construct an unauthenticated HTTP client with a custom path.
    pub fn new_custom(
        host: impl Into<String>,
        path: impl Into<String>,
        model: impl Into<String>,
        dims: usize,
    ) -> Self {
        let host = host.into();
        let path = path.into();
        Self::new(&format!("http://{host}{path}"), model, dims)
    }
    /// Construct a TLS client for the compatible ordered embedding endpoint.
    pub fn new_openai(
        host: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        dims: usize,
    ) -> Self {
        let mut engine = Self::new(
            &format!("https://{}/v1/embeddings", host.into()),
            model,
            dims,
        );
        engine.api_key = Some(api_key.into());
        engine
    }
    /// Fallible replacement for the legacy infallible embedding trait.
    pub fn try_embed_text(&self, text: &str) -> crate::Result<Vec<f32>> {
        self.embed_documents(&[text])
            .map(|mut vectors| vectors.remove(0))
    }
    #[cfg(all(feature = "http-embeddings", not(target_arch = "wasm32")))]
    fn request(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
        use std::io::Read;
        let reject = |message: &str| crate::Error::ConstraintViolation(message.into());
        if self.host.contains(['\r', '\n', '@', '/', '?', '#'])
            || self.path.contains(['\r', '\n', '#'])
            || !self.path.starts_with('/')
            || self.model.trim().is_empty()
            || self.dims == 0
        {
            return Err(reject("Invalid embedding endpoint, model or dimensions"));
        }
        if self.api_key.is_some() && !self.tls {
            return Err(reject("Embedding credentials require HTTPS"));
        }
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let body = if self.path.ends_with("/api/embeddings") {
            if texts.len() != 1 {
                return Err(reject(
                    "Legacy endpoint supports one input; use /api/embed for batches",
                ));
            }
            serde_json::json!({"model":self.model,"prompt":texts[0]})
        } else {
            serde_json::json!({"model":self.model,"input":texts,"truncate":false})
        };
        let endpoint = format!(
            "{}://{}{}",
            if self.tls { "https" } else { "http" },
            self.host,
            self.path
        );
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout(std::time::Duration::from_secs(30))
            .build();
        let mut request = agent
            .post(&endpoint)
            .set("Content-Type", "application/json");
        if let Some(key) = &self.api_key {
            request = request.set("Authorization", &format!("Bearer {key}"));
        }
        // Do not return transport errors containing credentials, request bodies or provider responses.
        let response = request
            .send_string(&body.to_string())
            .map_err(|_| reject("Embedding HTTP request failed"))?;
        if response.status() != 200 {
            return Err(reject("Embedding endpoint did not return HTTP 200"));
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(reject("Embedding response exceeds 16 MiB"));
        }
        let json: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| reject("Invalid embedding JSON response"))?;
        let value = if let Some(embeddings) = json.get("embeddings") {
            embeddings.clone()
        } else if let Some(embedding) = json.get("embedding") {
            serde_json::json!([embedding])
        } else if let Some(data) = json.get("data").and_then(|v| v.as_array()) {
            // The compatible endpoint must return unambiguous input order.
            if data
                .iter()
                .enumerate()
                .any(|(i, v)| v.get("index").and_then(|x| x.as_u64()) != Some(i as u64))
            {
                return Err(reject("Embedding response order mismatch"));
            }
            serde_json::Value::Array(
                data.iter()
                    .map(|v| v.get("embedding").cloned().unwrap_or_default())
                    .collect(),
            )
        } else {
            return Err(reject("Missing embeddings in provider response"));
        };
        let vectors: Vec<Vec<f32>> =
            serde_json::from_value(value).map_err(|_| reject("Invalid embedding components"))?;
        validate_embeddings(&vectors, texts.len(), self.dims)?;
        Ok(vectors)
    }
}
impl EmbeddingProvider for HttpEmbeddingEngine {
    fn model_id(&self) -> String {
        self.model.clone()
    }
    fn vector_dimensions(&self) -> usize {
        self.dims
    }
    fn embed_documents(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
        #[cfg(all(feature = "http-embeddings", not(target_arch = "wasm32")))]
        {
            self.request(texts)
        }
        #[cfg(not(all(feature = "http-embeddings", not(target_arch = "wasm32"))))]
        {
            let _ = (texts, self.tls);
            Err(crate::Error::ConstraintViolation("HTTP embeddings require the native http-embeddings feature; supply vectors explicitly on WASM".into()))
        }
    }
}
impl EmbeddingEngine for HttpEmbeddingEngine {
    fn dimensions(&self) -> usize {
        self.dims
    }
    /// Legacy API returns an empty vector on failure. Use try_embed_text to inspect errors.
    fn embed_text(&self, text: &str) -> Vec<f32> {
        self.try_embed_text(text).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_hash_embedder_consistency() {
        let embedder = DeterministicHashEmbedder::new(128);
        assert_eq!(EmbeddingEngine::dimensions(&embedder), 128);

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
