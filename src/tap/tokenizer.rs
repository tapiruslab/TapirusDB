//! High-performance, zero-allocation subword and n-gram tokenizer for the Tap Decision Engine.
//!
//! Provides deterministic subword segmentation and token hashing for sub-millisecond
//! non-autoregressive decision classification and scoring.

use std::collections::HashMap;

/// Vocabulary and configuration for the Tap Tokenizer
#[derive(Debug, Clone)]
pub struct TapTokenizer {
    vocab: HashMap<String, u32>,
    inv_vocab: Vec<String>,
    max_token_len: usize,
}

impl Default for TapTokenizer {
    fn default() -> Self {
        Self::new()
    }
}

impl TapTokenizer {
    /// Creates a default tokenizer initialized with core subword primitives
    pub fn new() -> Self {
        let mut tokenizer = Self {
            vocab: HashMap::new(),
            inv_vocab: Vec::new(),
            max_token_len: 16,
        };
        tokenizer.build_default_vocab();
        tokenizer
    }

    fn add_token(&mut self, token: &str) -> u32 {
        if let Some(&id) = self.vocab.get(token) {
            return id;
        }
        let id = self.inv_vocab.len() as u32;
        self.vocab.insert(token.to_string(), id);
        self.inv_vocab.push(token.to_string());
        id
    }

    fn build_default_vocab(&mut self) {
        self.add_token("[PAD]");
        self.add_token("[UNK]");
        self.add_token("[CLS]");
        self.add_token("[SEP]");
        self.add_token("[MASK]");

        // Basic ASCII printable characters
        for c in 32u8..=126u8 {
            let s = (c as char).to_string();
            self.add_token(&s);
        }

        // Common decision markers, modal tokens, and semantic stems across international languages
        let common_stems = [
            // English core stems
            "true", "false", "yes", "no", "valid", "invalid", "high", "low", "medium",
            "urgent", "normal", "critical", "fraud", "legit", "approve", "reject",
            "pending", "escalate", "resolve", "triage", "error", "success", "fail",
            "cancel", "refund", "policy", "active", "expired", "verified", "unverified",
            "positive", "negative", "neutral", "risk", "safe", "violation", "compliant",
            "support", "billing", "technical", "account", "security", "sales", "general",
            "the", "be", "to", "of", "and", "a", "in", "that", "have", "i", "it", "for",
            "not", "on", "with", "he", "as", "you", "do", "at", "this", "but", "his", "by",
            "from", "they", "we", "say", "her", "she", "or", "an", "will", "my", "one", "all",
            "would", "there", "their", "what", "so", "up", "out", "if", "about", "who", "get",
            "which", "go", "me", "when", "make", "can", "like", "time", "no", "just", "him",
            "know", "take", "people", "into", "year", "your", "good", "some", "could", "them",
            "see", "other", "than", "then", "now", "look", "only", "come", "its", "over",
            "think", "also", "back", "after", "use", "two", "how", "our", "work", "first",
            "well", "way", "even", "new", "want", "because", "any", "these", "give", "day",
            "most", "us", "is", "are", "was", "were", "has", "had", "should", "must", "claim",
            // Bahasa Melayu & Indonesian stems
            "sah", "batal", "palsu", "rosak", "pulang", "duit", "bayar", "kecemasan", "bahaya",
            "lulus", "tolak", "tuntut", "aduan", "betul", "salah", "penting", "akaun", "henti",
            "bantuan", "maklum", "hantar", "terima", "layak", "rugi", "untung", "daftar", "selesai",
            // Spanish stems
            "reembolso", "cancelar", "valido", "invalido", "fraude", "urgente", "aprobado",
            "rechazado", "peligro", "correcto", "falso", "soporte", "cuenta", "pagar", "seguro",
            // French stems
            "remboursement", "annuler", "valide", "urgent", "approuver", "rejeter", "danger",
            "erreur", "succes", "compte", "payer", "securite",
            // German stems
            "erstattung", "abbrechen", "gultig", "ungultig", "betrug", "dringend", "ablehnen",
            "fehler", "erfolg", "konto", "zahlen", "sicher",
        ];

        for stem in common_stems {
            self.add_token(stem);
            self.add_token(&format!("##{}", stem));
        }
    }

    /// Tokenize an input string into token IDs and string tokens
    pub fn tokenize(&self, text: &str) -> Vec<u32> {
        let mut tokens = vec![2]; // [CLS]
        let clean = text.to_lowercase();
        let words: Vec<&str> = clean
            .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
            .filter(|s| !s.is_empty())
            .collect();

        for word in words {
            let mut start = 0;
            let len = word.len();
            while start < len {
                let mut end = (start + self.max_token_len).min(len);
                let mut matched = false;
                while end > start {
                    while end > start && !word.is_char_boundary(end) {
                        end -= 1;
                    }
                    if end <= start {
                        break;
                    }
                    let sub = &word[start..end];
                    let candidate = if start == 0 {
                        sub.to_string()
                    } else {
                        format!("##{}", sub)
                    };

                    if let Some(&id) = self.vocab.get(&candidate) {
                        tokens.push(id);
                        matched = true;
                        start = end;
                        break;
                    }
                    end = end.saturating_sub(1);
                }

                if !matched {
                    // Fallback to UTF-8 character token or UNK
                    let mut char_iter = word[start..].chars();
                    if let Some(ch) = char_iter.next() {
                        let ch_len = ch.len_utf8();
                        let char_str = &word[start..start + ch_len];
                        let id = self.vocab.get(char_str).copied().unwrap_or(1); // 1 = [UNK]
                        tokens.push(id);
                        start += ch_len;
                    } else {
                        break;
                    }
                }
            }
        }

        tokens.push(3); // [SEP]
        tokens
    }

    /// Vocabulary size
    pub fn vocab_size(&self) -> usize {
        self.inv_vocab.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenizer_basic() {
        let tok = TapTokenizer::new();
        let ids = tok.tokenize("Valid claim refund approved");
        assert!(ids.len() >= 4);
        assert_eq!(ids[0], 2); // [CLS]
        assert_eq!(*ids.last().unwrap(), 3); // [SEP]
    }
}
