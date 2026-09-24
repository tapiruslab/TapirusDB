//! Pure Safe-Rust Inverted Index and BM25 Full-Text Search Engine.
//!
//! Provides zero-dependency, ultra-lightweight lexical search optimized for
//! edge microprocessors, IoT devices, and WebAssembly with zero unsafe code.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// BM25 Index parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bm25Params {
    /// Term frequency saturation parameter (default: 1.2)
    pub k1: f32,
    /// Document length normalization parameter (default: 0.75)
    pub b: f32,
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

/// Simple, zero-dependency multilingual-friendly tokenizer
pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|t| !t.is_empty() && t.len() > 1)
        .map(|t| t.to_string())
        .collect()
}

/// Pure Safe-Rust Inverted Index with BM25 Scoring
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Bm25Index {
    /// Mapping of term -> list of (doc_id, term_frequency)
    inverted_index: HashMap<String, Vec<(u64, u32)>>,
    /// Mapping of doc_id -> total token count in document
    doc_lengths: HashMap<u64, usize>,
    /// BM25 tuning parameters
    params: Bm25Params,
}

impl Bm25Index {
    /// Create a new empty BM25 index with default parameters
    pub fn new() -> Self {
        Self {
            inverted_index: HashMap::new(),
            doc_lengths: HashMap::new(),
            params: Bm25Params::default(),
        }
    }

    /// Create a new BM25 index with custom tuning parameters
    pub fn with_params(params: Bm25Params) -> Self {
        Self {
            inverted_index: HashMap::new(),
            doc_lengths: HashMap::new(),
            params,
        }
    }

    /// Index a document with a unique 64-bit document ID
    pub fn index_document(&mut self, doc_id: u64, text: &str) {
        // If document already exists, remove old occurrences first
        if self.doc_lengths.contains_key(&doc_id) {
            self.remove_document(doc_id);
        }

        let tokens = tokenize(text);
        if tokens.is_empty() {
            self.doc_lengths.insert(doc_id, 0);
            return;
        }

        self.doc_lengths.insert(doc_id, tokens.len());

        // Compute term frequencies for this document
        let mut term_counts: HashMap<String, u32> = HashMap::new();
        for token in tokens {
            *term_counts.entry(token).or_insert(0) += 1;
        }

        for (term, count) in term_counts {
            self.inverted_index
                .entry(term)
                .or_default()
                .push((doc_id, count));
        }
    }

    /// Remove a document from the inverted index
    pub fn remove_document(&mut self, doc_id: u64) {
        if self.doc_lengths.remove(&doc_id).is_some() {
            for postings in self.inverted_index.values_mut() {
                postings.retain(|(id, _)| *id != doc_id);
            }
            // Clean up empty term entries
            self.inverted_index.retain(|_, postings| !postings.is_empty());
        }
    }

    /// Total number of indexed documents
    pub fn total_docs(&self) -> usize {
        self.doc_lengths.len()
    }

    /// Average document length across the entire index
    pub fn avg_doc_length(&self) -> f32 {
        if self.doc_lengths.is_empty() {
            return 0.0;
        }
        let sum: usize = self.doc_lengths.values().sum();
        sum as f32 / self.doc_lengths.len() as f32
    }

    /// Search the index using Okapi BM25 scoring, returning top-k matching doc IDs with scores
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(u64, f32)> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() || self.doc_lengths.is_empty() {
            return Vec::new();
        }

        let n_docs = self.total_docs() as f32;
        let avgdl = self.avg_doc_length();
        let k1 = self.params.k1;
        let b = self.params.b;

        let mut scores: HashMap<u64, f32> = HashMap::new();
        let mut unique_query_terms: HashSet<&str> = HashSet::new();

        for token in &query_tokens {
            if !unique_query_terms.insert(token.as_str()) {
                continue; // Process each unique query term once for IDF
            }

            if let Some(postings) = self.inverted_index.get(token.as_str()) {
                let doc_freq = postings.len() as f32;
                // Standard smoothed IDF: ln(1 + (N - df + 0.5) / (df + 0.5))
                let idf = ((n_docs - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0).ln().max(0.1);

                for &(doc_id, tf) in postings {
                    let doc_len = *self.doc_lengths.get(&doc_id).unwrap_or(&0) as f32;
                    let tf_val = tf as f32;
                    
                    let denom = tf_val + k1 * (1.0 - b + b * (doc_len / avgdl.max(1.0)));
                    let term_score = idf * (tf_val * (k1 + 1.0)) / denom.max(0.0001);

                    *scores.entry(doc_id).or_insert(0.0) += term_score;
                }
            }
        }

        let mut result: Vec<(u64, f32)> = scores.into_iter().collect();
        result.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        result.truncate(top_k);

        // Normalize scores to [0.0, 1.0] if top score > 0
        if let Some(&(_, max_score)) = result.first() {
            if max_score > 0.0 {
                for (_, score) in result.iter_mut() {
                    *score /= max_score;
                }
            }
        }

        result
    }
}

/// Combine multiple ranked result lists into a single unified ranking using Reciprocal Rank Fusion (RRF).
///
/// Formula: `score(d) = \sum_{m} \frac{weight_m}{k + rank_m(d)}`
/// where `k` is the RRF smoothing constant (typically 60.0).
pub fn reciprocal_rank_fusion(
    ranked_lists: &[(&[u64], f32)],
    k: f32,
    top_n: usize,
) -> Vec<(u64, f32)> {
    let mut scores: HashMap<u64, f32> = HashMap::new();

    for &(doc_ids, weight) in ranked_lists {
        for (rank_idx, &doc_id) in doc_ids.iter().enumerate() {
            let rank = (rank_idx + 1) as f32;
            let rrf_score = weight / (k + rank);
            *scores.entry(doc_id).or_insert(0.0) += rrf_score;
        }
    }

    let mut fused: Vec<(u64, f32)> = scores.into_iter().collect();
    fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    fused.truncate(top_n);
    fused
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bm25_tokenization() {
        let tokens = tokenize("Hello, World! This is TapirusDB-2026.");
        assert_eq!(tokens, vec!["hello", "world", "this", "is", "tapirusdb-2026"]);
    }

    #[test]
    fn test_bm25_search_scoring() {
        let mut index = Bm25Index::new();
        index.index_document(1, "The quick brown fox jumps over the lazy dog");
        index.index_document(2, "Ahmad Faiz built TapirusDB for embedded edge AI memory");
        index.index_document(3, "TapirusDB is ultra fast and safe in Pure Safe Rust");

        let results = index.search("TapirusDB Safe Rust", 5);
        assert!(!results.is_empty());
        // Doc 3 has both TapirusDB and Safe and Rust, so it should rank first
        assert_eq!(results[0].0, 3);
        assert_eq!(results[0].1, 1.0); // Normalized to 1.0
    }

    #[test]
    fn test_bm25_remove_document() {
        let mut index = Bm25Index::new();
        index.index_document(1, "Artificial intelligence agent memory");
        index.index_document(2, "Database engine persistence");

        assert_eq!(index.total_docs(), 2);
        index.remove_document(1);
        assert_eq!(index.total_docs(), 1);

        let res = index.search("intelligence", 5);
        assert!(res.is_empty());
    }

    #[test]
    fn test_reciprocal_rank_fusion() {
        let list_a: Vec<u64> = vec![10, 20, 30];
        let list_b: Vec<u64> = vec![20, 10, 40];

        let fused = reciprocal_rank_fusion(&[(&list_a, 1.0), (&list_b, 1.0)], 60.0, 3);
        assert!(!fused.is_empty());

        // Doc 20 ranked #2 in A and #1 in B: 1/(60+2) + 1/(60+1) = 1/62 + 1/61 = 0.016129 + 0.016393 = 0.03252
        // Doc 10 ranked #1 in A and #2 in B: 1/(60+1) + 1/(60+2) = same score!
        assert!(fused[0].0 == 10 || fused[0].0 == 20);
        assert!(fused[0].1 > 0.03);
    }
}
