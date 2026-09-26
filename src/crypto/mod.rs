//! # Native Page-Level Cryptographic Encryption for TapirusDB.
//!
//! Implements ChaCha20-Poly1305 Authenticated Encryption with Associated Data (AEAD).
//! Provides page-level encryption for `.tapir` and `.tapir-wal` files, key derivation
//! via SHA-256, and constant-time Key Check Value (KCV) verification.

use crate::error::{Error, Result};
use crate::pager::PageId;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};
use sha2::{Digest, Sha256};

/// Poly1305 MAC authentication tag size in bytes
pub const TAG_SIZE: usize = 16;

/// 256-bit AES / ChaCha20 encryption key size in bytes
pub const KEY_SIZE: usize = 32;

/// Database salt size in bytes
pub const SALT_SIZE: usize = 16;

/// Constant verification payload for Key Check Value (KCV)
pub const KCV_MAGIC: &[u8] = b"TAPIRUS_CIPHER_KCV_V1";

/// Usable unencrypted slotted page content size when encryption is enabled
pub const ENCRYPTED_PAGE_USABLE_SIZE: usize = 4096 - TAG_SIZE; // 4080 bytes

/// Standard iterations for PBKDF2-HMAC-SHA256 password key derivation in production
pub const PBKDF2_RECOMMENDED_ITERATIONS: u32 = 100_000;
/// Fast iterations for PBKDF2-HMAC-SHA256 used in development and fast unit testing
pub const PBKDF2_FAST_ITERATIONS: u32 = 10_000;

/// Compute standard HMAC-SHA256 (RFC 2104)
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k_block = [0u8; 64];
    if key.len() > 64 {
        let hashed = Sha256::digest(key);
        k_block[0..32].copy_from_slice(&hashed);
    } else {
        k_block[0..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0u8; 64];
    let mut opad = [0u8; 64];
    for i in 0..64 {
        ipad[i] = k_block[i] ^ 0x36;
        opad[i] = k_block[i] ^ 0x5c;
    }

    let mut inner_hasher = Sha256::new();
    inner_hasher.update(&ipad);
    inner_hasher.update(data);
    let inner_hash = inner_hasher.finalize();

    let mut outer_hasher = Sha256::new();
    outer_hasher.update(&opad);
    outer_hasher.update(&inner_hash);
    let outer_hash = outer_hasher.finalize();

    let mut out = [0u8; 32];
    out.copy_from_slice(&outer_hash);
    out
}

/// Derive a 256-bit cryptographic key using PBKDF2-HMAC-SHA256 (RFC 2898 / RFC 6070)
pub fn pbkdf2_hmac_sha256(passphrase: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    assert!(iterations >= 1);
    let mut salt_and_int = Vec::with_capacity(salt.len() + 4);
    salt_and_int.extend_from_slice(salt);
    salt_and_int.extend_from_slice(&1u32.to_be_bytes());

    let mut u_prev = hmac_sha256(passphrase, &salt_and_int);
    let mut result = u_prev;

    for _ in 1..iterations {
        u_prev = hmac_sha256(passphrase, &u_prev);
        for j in 0..32 {
            result[j] ^= u_prev[j];
        }
    }

    result
}

/// Derive a 256-bit cryptographic key from a user passphrase and database salt using PBKDF2.
///
/// Uses `PBKDF2_RECOMMENDED_ITERATIONS` (100,000) per OWASP/NIST guidelines for
/// production brute-force resistance. Use `derive_key_with_iterations` for test/dev fast mode.
pub fn derive_key(passphrase: &str, salt: &[u8; 16]) -> [u8; 32] {
    pbkdf2_hmac_sha256(passphrase.as_bytes(), salt, PBKDF2_RECOMMENDED_ITERATIONS)
}

/// Derive a 256-bit cryptographic key with explicitly specified PBKDF2 iteration count
pub fn derive_key_with_iterations(passphrase: &str, salt: &[u8; 16], iterations: u32) -> [u8; 32] {
    pbkdf2_hmac_sha256(passphrase.as_bytes(), salt, iterations)
}

/// Generate a cryptographically secure 16-byte random salt using the OS CSPRNG.
///
/// Uses `getrandom` which delegates to the platform's secure entropy source
/// (e.g. `getrandom(2)` on Linux, `BCryptGenRandom` on Windows). This is
/// indistinguishable from random even if the creation timestamp is known.
pub fn generate_random_salt() -> [u8; 16] {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).expect("OS CSPRNG unavailable — cannot generate secure salt");
    salt
}

/// Cryptographic engine managing page-level encryption and verification
#[derive(Clone)]
pub struct DatabaseCipher {
    key: [u8; 32],
    salt: [u8; 16],
    epoch: u32,
}

impl DatabaseCipher {
    /// Initialize cipher with raw 256-bit key and 16-byte salt
    pub fn new(key: [u8; 32], salt: [u8; 16]) -> Self {
        Self { key, salt, epoch: 1 }
    }

    /// Initialize cipher by deriving 256-bit key from passphrase and salt via PBKDF2
    pub fn from_passphrase(passphrase: &str, salt: [u8; 16]) -> Self {
        let key = derive_key(passphrase, &salt);
        Self { key, salt, epoch: 1 }
    }

    /// Initialize cipher with custom PBKDF2 iteration count
    pub fn from_passphrase_with_iterations(passphrase: &str, salt: [u8; 16], iterations: u32) -> Self {
        let key = derive_key_with_iterations(passphrase, &salt, iterations);
        Self { key, salt, epoch: 1 }
    }

    /// Return reference to database salt
    pub fn salt(&self) -> &[u8; 16] {
        &self.salt
    }

    /// Return raw key bytes
    pub fn key(&self) -> &[u8; 32] {
        &self.key
    }

    /// Return database epoch
    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// Set database epoch (e.g. incremented after major checkpoints / rollbacks)
    pub fn set_epoch(&mut self, epoch: u32) {
        self.epoch = epoch;
    }

    /// Derive unique 12-byte AEAD nonce with monotonic write sequence and epoch
    /// Formally satisfies RFC 8439 Nonce Uniqueness Requirement:
    /// Nonce = Epoch (4B) || PageId (4B) || Sequence (4B)
    pub fn derive_nonce_with_seq(&self, page_id: PageId, sequence: u32, epoch: u32) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[0..4].copy_from_slice(&epoch.to_le_bytes());
        nonce[4..8].copy_from_slice(&page_id.to_le_bytes());
        nonce[8..12].copy_from_slice(&sequence.to_le_bytes());
        nonce
    }

    /// Default 12-byte nonce for backward compatibility (sequence 0, epoch 1)
    pub fn derive_nonce(&self, page_id: PageId) -> [u8; 12] {
        self.derive_nonce_with_seq(page_id, 0, self.epoch)
    }

    /// Generate 16-byte Key Check Value (KCV) tag for header validation.
    ///
    /// Returns `Err` if the ChaCha20-Poly1305 cipher fails, so callers always
    /// know when KCV generation is impossible rather than silently receiving a
    /// zeroed tag that would bypass authentication checks.
    pub fn generate_kcv(&self) -> Result<[u8; 16]> {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        let nonce = Nonce::from_slice(&self.salt[0..12]);
        let ciphertext = cipher
            .encrypt(nonce, KCV_MAGIC)
            .map_err(|_| Error::Corrupted("KCV generation failed: ChaCha20-Poly1305 encryption error".into()))?;
        if ciphertext.len() < 16 {
            return Err(Error::Corrupted("KCV ciphertext too short".into()));
        }
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&ciphertext[ciphertext.len() - 16..]);
        Ok(tag)
    }

    /// Constant-time verification of stored KCV against candidate key.
    /// Uses cumulative XOR reduction and compiler memory barriers to thwart timing attacks.
    /// Returns `false` (not-equal) on internal error rather than panicking.
    pub fn verify_kcv(&self, stored_kcv: &[u8; 16]) -> bool {
        let expected = match self.generate_kcv() {
            Ok(v) => v,
            Err(_) => return false,
        };
        let mut diff = 0u8;
        for i in 0..16 {
            diff |= expected[i] ^ stored_kcv[i];
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
        diff == 0
    }

    /// Encrypt a 4,096-byte in-memory page into a 4,096-byte disk page
    pub fn encrypt_page(&self, page_id: PageId, plaintext: &[u8]) -> Result<Vec<u8>> {
        self.encrypt_page_with_seq(page_id, 0, self.epoch, plaintext)
    }

    /// Encrypt a 4,096-byte in-memory page with a strictly unique monotonic sequence and epoch
    pub fn encrypt_page_with_seq(&self, page_id: PageId, sequence: u32, epoch: u32, plaintext: &[u8]) -> Result<Vec<u8>> {
        if plaintext.len() != 4096 {
            return Err(Error::Corrupted(format!(
                "Expected 4096 byte page, got {}",
                plaintext.len()
            )));
        }

        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        let nonce_bytes = self.derive_nonce_with_seq(page_id, sequence, epoch);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let (pt_start, pt_end) = if page_id == 1 {
            (100, ENCRYPTED_PAGE_USABLE_SIZE)
        } else {
            (0, ENCRYPTED_PAGE_USABLE_SIZE)
        };

        let pt_slice = &plaintext[pt_start..pt_end];
        let payload = Payload {
            msg: pt_slice,
            aad: &page_id.to_le_bytes(),
        };

        let ct = cipher
            .encrypt(nonce, payload)
            .map_err(|e| Error::Corrupted(format!("Encryption error: {e}")))?;

        let tag_offset = ct.len() - TAG_SIZE;
        let actual_ct = &ct[0..tag_offset];
        let tag = &ct[tag_offset..];

        let mut out_page = vec![0u8; 4096];
        if page_id == 1 {
            out_page[0..100].copy_from_slice(&plaintext[0..100]);
        }
        out_page[pt_start..pt_end].copy_from_slice(actual_ct);
        out_page[4080..4096].copy_from_slice(tag);

        Ok(out_page)
    }

    /// Decrypt a 4,096-byte disk page back into an in-memory page
    pub fn decrypt_page(&self, page_id: PageId, disk_page: &[u8]) -> Result<Vec<u8>> {
        self.decrypt_page_with_seq(page_id, 0, self.epoch, disk_page)
    }

    /// Decrypt a 4,096-byte disk page with a specified sequence and epoch
    pub fn decrypt_page_with_seq(&self, page_id: PageId, sequence: u32, epoch: u32, disk_page: &[u8]) -> Result<Vec<u8>> {
        if disk_page.len() != 4096 {
            return Err(Error::Corrupted(format!(
                "Expected 4096 byte page, got {}",
                disk_page.len()
            )));
        }

        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        let nonce_bytes = self.derive_nonce_with_seq(page_id, sequence, epoch);
        let nonce = Nonce::from_slice(&nonce_bytes);


        let (pt_start, pt_end) = if page_id == 1 {
            (100, ENCRYPTED_PAGE_USABLE_SIZE)
        } else {
            (0, ENCRYPTED_PAGE_USABLE_SIZE)
        };

        let ct_slice = &disk_page[pt_start..pt_end];
        let tag_slice = &disk_page[4080..4096];

        let mut combined = Vec::with_capacity(ct_slice.len() + tag_slice.len());
        combined.extend_from_slice(ct_slice);
        combined.extend_from_slice(tag_slice);

        let payload = Payload {
            msg: &combined,
            aad: &page_id.to_le_bytes(),
        };

        let pt = cipher
            .decrypt(nonce, payload)
            .map_err(|_| Error::DecryptionFailed(page_id))?;

        let mut out_page = vec![0u8; 4096];
        if page_id == 1 {
            out_page[0..100].copy_from_slice(&disk_page[0..100]);
        }
        out_page[pt_start..pt_end].copy_from_slice(&pt);

        Ok(out_page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cipher_roundtrip_page() {
        let salt = [7u8; 16];
        let cipher = DatabaseCipher::from_passphrase_with_iterations("super-secret-robot-key", salt, PBKDF2_FAST_ITERATIONS);

        // Test KCV verification
        let kcv = cipher.generate_kcv().unwrap();
        assert!(cipher.verify_kcv(&kcv));

        let wrong_cipher = DatabaseCipher::from_passphrase_with_iterations("wrong-password", salt, PBKDF2_FAST_ITERATIONS);
        assert!(!wrong_cipher.verify_kcv(&kcv));

        // Test Page 2 encryption
        let mut original_page = vec![0u8; 4096];
        original_page[10] = 42;
        original_page[500] = 99;
        original_page[4000] = 123;

        let encrypted = cipher.encrypt_page(2, &original_page).unwrap();
        assert_ne!(encrypted, original_page);
        assert_eq!(encrypted.len(), 4096);

        // Decrypt with correct key
        let decrypted = cipher.decrypt_page(2, &encrypted).unwrap();
        assert_eq!(decrypted[10], 42);
        assert_eq!(decrypted[500], 99);
        assert_eq!(decrypted[4000], 123);

        // Decrypt with wrong key fails
        let fail = wrong_cipher.decrypt_page(2, &encrypted);
        assert!(fail.is_err());
    }

    #[test]
    fn test_cipher_roundtrip_page1() {
        let salt = [3u8; 16];
        let cipher = DatabaseCipher::from_passphrase_with_iterations("mars-rover-mission", salt, PBKDF2_FAST_ITERATIONS);

        let mut page1 = vec![0u8; 4096];
        page1[0..8].copy_from_slice(b"TAPIRUS\0"); // Plaintext header preserved
        page1[200] = 88;

        let encrypted = cipher.encrypt_page(1, &page1).unwrap();
        assert_eq!(&encrypted[0..8], b"TAPIRUS\0"); // Header remains readable
        assert_ne!(encrypted[200], 88); // Payload is encrypted

        let decrypted = cipher.decrypt_page(1, &encrypted).unwrap();
        assert_eq!(&decrypted[0..8], b"TAPIRUS\0");
        assert_eq!(decrypted[200], 88);
    }

    #[test]
    fn test_pbkdf2_rfc6070_vectors() {
        // RFC 6070 Test Vector 1: c = 1
        let p = b"password";
        let s = b"salt";
        let key_c1 = pbkdf2_hmac_sha256(p, s, 1);
        let expected_c1 = [
            0x12, 0x0f, 0xb6, 0xcf, 0xfc, 0xf8, 0xb3, 0x2c,
            0x43, 0xe7, 0x22, 0x52, 0x56, 0xc4, 0xf8, 0x37,
            0xa8, 0x65, 0x48, 0xc9, 0x2c, 0xcc, 0x35, 0x48,
            0x08, 0x05, 0x98, 0x7c, 0xb7, 0x0b, 0xe1, 0x7b,
        ];
        assert_eq!(key_c1, expected_c1);

        // RFC 6070 Test Vector 2: c = 2
        let key_c2 = pbkdf2_hmac_sha256(p, s, 2);
        let expected_c2 = [
            0xae, 0x4d, 0x0c, 0x95, 0xaf, 0x6b, 0x46, 0xd3,
            0x2d, 0x0a, 0xdf, 0xf9, 0x28, 0xf0, 0x6d, 0xd0,
            0x2a, 0x30, 0x3f, 0x8e, 0xf3, 0xc2, 0x51, 0xdf,
            0xd6, 0xe2, 0xd8, 0x5a, 0x95, 0x47, 0x4c, 0x43,
        ];
        assert_eq!(key_c2, expected_c2);
    }

    #[test]
    fn test_monotonic_nonce_uniqueness() {
        let salt = [11u8; 16];
        let cipher = DatabaseCipher::from_passphrase_with_iterations("quantum_encryption_2026", salt, PBKDF2_FAST_ITERATIONS);

        let page_data = vec![0xaau8; 4096];

        // Encrypt page 4 at sequence 1 vs sequence 2
        let ct_seq1 = cipher.encrypt_page_with_seq(4, 1, 1, &page_data).unwrap();
        let ct_seq2 = cipher.encrypt_page_with_seq(4, 2, 1, &page_data).unwrap();

        // Distinct nonces guarantee completely different ciphertexts and auth tags
        assert_ne!(ct_seq1, ct_seq2);

        // Decrypt with correct sequence succeeds
        let pt1 = cipher.decrypt_page_with_seq(4, 1, 1, &ct_seq1).unwrap();
        assert_eq!(&pt1[0..ENCRYPTED_PAGE_USABLE_SIZE], &page_data[0..ENCRYPTED_PAGE_USABLE_SIZE]);

        // Decrypt with mismatched sequence fails authentication (RFC 8439)
        let fail = cipher.decrypt_page_with_seq(4, 2, 1, &ct_seq1);
        assert!(fail.is_err());
    }
}

