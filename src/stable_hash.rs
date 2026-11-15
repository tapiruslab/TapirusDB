//! Frozen, platform-independent 64-bit hashing for on-disk keys.
//!
//! `std::collections::hash_map::DefaultHasher` is documented as unstable across
//! Rust releases and its `Hash` byte stream depends on the target's endianness
//! and pointer width. Values derived from it must never be persisted.
//!
//! [`StableHasher`] is a self-contained SipHash-1-3 with zero keys. The `key_*`
//! helpers feed it an explicit little-endian byte stream with 64-bit length
//! prefixes. On 64-bit little-endian targets this is bit-identical to the
//! historical `DefaultHasher` output used by TapirusDB `<= 1.1.0`, so existing
//! database files remain readable without migration, while the result is now
//! frozen for every Rust version, endianness and pointer width (including wasm32).

use crate::traits::Value;

/// Self-contained SipHash-1-3 (k0 = k1 = 0) whose output is frozen forever.
#[derive(Debug, Clone)]
pub struct StableHasher {
    v0: u64,
    v1: u64,
    v2: u64,
    v3: u64,
    tail: [u8; 8],
    ntail: usize,
    length: u64,
}

impl Default for StableHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl StableHasher {
    /// Create a hasher with the fixed zero key.
    pub const fn new() -> Self {
        Self {
            v0: 0x736f_6d65_7073_6575,
            v1: 0x646f_7261_6e64_6f6d,
            v2: 0x6c79_6765_6e65_7261,
            v3: 0x7465_6462_7974_6573,
            tail: [0; 8],
            ntail: 0,
            length: 0,
        }
    }

    #[inline(always)]
    fn round(&mut self) {
        self.v0 = self.v0.wrapping_add(self.v1);
        self.v1 = self.v1.rotate_left(13);
        self.v1 ^= self.v0;
        self.v0 = self.v0.rotate_left(32);
        self.v2 = self.v2.wrapping_add(self.v3);
        self.v3 = self.v3.rotate_left(16);
        self.v3 ^= self.v2;
        self.v0 = self.v0.wrapping_add(self.v3);
        self.v3 = self.v3.rotate_left(21);
        self.v3 ^= self.v0;
        self.v2 = self.v2.wrapping_add(self.v1);
        self.v1 = self.v1.rotate_left(17);
        self.v1 ^= self.v2;
        self.v2 = self.v2.rotate_left(32);
    }

    #[inline(always)]
    fn compress_word(&mut self, m: u64) {
        self.v3 ^= m;
        self.round();
        self.v0 ^= m;
    }

    /// Feed raw bytes.
    pub fn write_bytes(&mut self, mut msg: &[u8]) {
        self.length = self.length.wrapping_add(msg.len() as u64);
        if self.ntail != 0 {
            let take = (8 - self.ntail).min(msg.len());
            self.tail[self.ntail..self.ntail + take].copy_from_slice(&msg[..take]);
            self.ntail += take;
            msg = &msg[take..];
            if self.ntail < 8 {
                return;
            }
            let word = u64::from_le_bytes(self.tail);
            self.compress_word(word);
            self.ntail = 0;
        }
        let (words, rest) = msg.as_chunks::<8>();
        for word in words {
            self.compress_word(u64::from_le_bytes(*word));
        }
        self.tail[..rest.len()].copy_from_slice(rest);
        self.ntail = rest.len();
    }

    /// Feed one byte.
    #[inline]
    pub fn write_u8(&mut self, v: u8) {
        self.write_bytes(&[v]);
    }

    /// Feed a `u32` as 4 little-endian bytes.
    #[inline]
    pub fn write_u32(&mut self, v: u32) {
        self.write_bytes(&v.to_le_bytes());
    }

    /// Feed a `u64` as 8 little-endian bytes.
    #[inline]
    pub fn write_u64(&mut self, v: u64) {
        self.write_bytes(&v.to_le_bytes());
    }

    /// Feed a string exactly like `str: Hash` (bytes followed by `0xFF`).
    #[inline]
    pub fn write_str(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
        self.write_u8(0xFF);
    }

    /// Feed a byte slice exactly like `[u8]: Hash` on 64-bit (u64 length prefix + bytes).
    #[inline]
    pub fn write_len_prefixed(&mut self, b: &[u8]) {
        self.write_u64(b.len() as u64);
        self.write_bytes(b);
    }

    /// Finalize the SipHash-1-3 state.
    pub fn finish(&self) -> u64 {
        let mut state = self.clone();
        let mut last = [0u8; 8];
        last[..state.ntail].copy_from_slice(&state.tail[..state.ntail]);
        let b = ((state.length & 0xff) << 56) | u64::from_le_bytes(last);
        state.compress_word(b);
        state.v2 ^= 0xff;
        state.round();
        state.round();
        state.round();
        state.v0 ^ state.v1 ^ state.v2 ^ state.v3
    }
}

/// Never return 0: some B+Tree callers reserve key 0 as "absent".
#[inline]
fn non_zero(h: u64) -> u64 {
    if h == 0 { 1 } else { h }
}

/// Stable 64-bit B+Tree key for a secondary-index value.
pub fn key_for_value(val: &Value) -> u64 {
    let mut h = StableHasher::new();
    match val {
        Value::Null => h.write_u8(0),
        Value::Integer(i) => {
            h.write_u8(1);
            h.write_u64(*i as u64);
        }
        Value::Real(f) => {
            h.write_u8(2);
            h.write_u64(f.to_bits());
        }
        Value::Text(s) => {
            h.write_u8(3);
            h.write_str(s);
        }
        Value::Blob(b) => {
            h.write_u8(4);
            h.write_len_prefixed(b);
        }
        Value::Vector(v) => {
            h.write_u8(5);
            for x in v {
                h.write_u32(x.to_bits());
            }
        }
    }
    non_zero(h.finish())
}

/// Stable 64-bit key for a table/collection name (case-insensitive).
pub fn key_for_name(name: &str) -> u64 {
    let mut h = StableHasher::new();
    h.write_str(&name.to_lowercase());
    non_zero(h.finish())
}

/// Stable 64-bit identifier derived from free text (e.g. graph projection IDs).
pub fn hash_text(s: &str) -> u64 {
    let mut h = StableHasher::new();
    h.write_str(s);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_siphash13_vectors_are_frozen() {
        // Pinned outputs: changing these breaks every persisted secondary index.
        assert_eq!(StableHasher::new().finish(), 0xd1fb_a762_150c_532c);
        assert_ne!(key_for_value(&Value::Integer(42)), 0);
        assert_eq!(key_for_name("Users"), key_for_name("users"));
    }

    #[test]
    fn chunked_writes_equal_single_write() {
        let data: Vec<u8> = (0u8..=200).collect();
        let mut a = StableHasher::new();
        a.write_bytes(&data);
        for split in [1usize, 3, 7, 8, 9, 63, 64, 65, 150] {
            let mut b = StableHasher::new();
            for chunk in data.chunks(split) {
                b.write_bytes(chunk);
            }
            assert_eq!(a.finish(), b.finish(), "split {split}");
        }
    }

    /// On 64-bit little-endian targets the frozen hasher must reproduce the
    /// historical `DefaultHasher` keys so existing `.tapir` files keep working.
    #[cfg(all(target_pointer_width = "64", target_endian = "little"))]
    #[test]
    #[allow(deprecated)]
    fn matches_legacy_default_hasher_keys() {
        use std::hash::{Hash, Hasher};

        fn legacy_value_key(val: &Value) -> u64 {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            match val {
                Value::Null => 0u8.hash(&mut hasher),
                Value::Integer(i) => {
                    1u8.hash(&mut hasher);
                    i.hash(&mut hasher);
                }
                Value::Real(f) => {
                    2u8.hash(&mut hasher);
                    f.to_bits().hash(&mut hasher);
                }
                Value::Text(s) => {
                    3u8.hash(&mut hasher);
                    s.hash(&mut hasher);
                }
                Value::Blob(b) => {
                    4u8.hash(&mut hasher);
                    b.hash(&mut hasher);
                }
                Value::Vector(v) => {
                    5u8.hash(&mut hasher);
                    for x in v {
                        x.to_bits().hash(&mut hasher);
                    }
                }
            }
            let h = hasher.finish();
            if h == 0 { 1 } else { h }
        }

        let samples = vec![
            Value::Null,
            Value::Integer(0),
            Value::Integer(-1),
            Value::Integer(i64::MAX),
            Value::Real(3.25),
            Value::Real(-0.0),
            Value::Text(String::new()),
            Value::Text("alice".into()),
            Value::Text("unicode ✓ tapir — ﷺ".into()),
            Value::Text("x".repeat(1000)),
            Value::Blob(vec![]),
            Value::Blob((0u8..=255).collect()),
            Value::Vector(vec![0.1, -0.2, 0.3]),
        ];
        for v in &samples {
            assert_eq!(key_for_value(v), legacy_value_key(v), "value {v:?}");
        }

        for name in ["users", "Orders", "__sys_vec", "Ünïcødé"] {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            name.to_lowercase().hash(&mut hasher);
            let h = hasher.finish();
            let legacy = if h == 0 { 1 } else { h };
            assert_eq!(key_for_name(name), legacy, "name {name}");

            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            name.hash(&mut hasher);
            assert_eq!(hash_text(name), hasher.finish(), "text {name}");
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        hasher.write(&[]);
        assert_eq!(StableHasher::new().finish(), hasher.finish());
    }
}
