//! Variable-length integer (Varint) encoding and decoding.
//!
//! TapirusDB uses SQLite-compatible 1-to-9 byte variable-length integers to
//! compress integer keys and lengths compactly inside B+Tree cells.
//!
//! In bytes 1..8, the high-order bit indicates continuation (1 = more bytes, 0 = last byte).
//! Byte 9 uses all 8 bits for data (supporting full 64-bit unsigned integers).

use crate::error::{Error, Result};

/// Encode a 64-bit unsigned integer into a variable-length byte slice.
/// Returns the number of bytes written (between 1 and 9).
pub fn encode_varint(mut value: u64, buf: &mut [u8]) -> usize {
    if value <= 0x7F {
        buf[0] = value as u8;
        return 1;
    }

    if value > 0x00FF_FFFF_FFFF_FFFF {
        // Needs all 9 bytes
        buf[8] = value as u8;
        value >>= 8;
        for i in (0..8).rev() {
            buf[i] = ((value & 0x7F) as u8) | 0x80;
            value >>= 7;
        }
        return 9;
    }

    let mut temp = [0u8; 10];
    let mut len = 0;
    while value > 0 {
        temp[len] = (value & 0x7F) as u8;
        value >>= 7;
        len += 1;
    }

    for i in 0..len {
        let byte = temp[len - 1 - i];
        buf[i] = if i + 1 < len { byte | 0x80 } else { byte };
    }

    len
}

/// Decode a 64-bit unsigned integer from a variable-length byte slice.
/// Returns `(value, bytes_read)`.
pub fn decode_varint(buf: &[u8]) -> Result<(u64, usize)> {
    if buf.is_empty() {
        return Err(Error::Corrupted("Unexpected EOF decoding varint".into()));
    }

    let mut value: u64 = 0;
    for (i, &byte) in buf.iter().enumerate().take(9) {
        if i == 8 {
            // 9th byte uses all 8 bits
            value = (value << 8) | (byte as u64);
            return Ok((value, 9));
        }

        value = (value << 7) | ((byte & 0x7F) as u64);
        if (byte & 0x80) == 0 {
            return Ok((value, i + 1));
        }
    }

    Err(Error::Corrupted("Unterminated varint sequence".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_single_byte() {
        let mut buf = [0u8; 9];
        let len = encode_varint(42, &mut buf);
        assert_eq!(len, 1);
        assert_eq!(buf[0], 42);

        let (val, read_len) = decode_varint(&buf).expect("Decode failed");
        assert_eq!(val, 42);
        assert_eq!(read_len, 1);
    }

    #[test]
    fn test_varint_multi_byte_roundtrip() {
        let test_cases: [u64; 7] = [
            0,
            127,
            128,
            16383,
            16384,
            1_000_000,
            u64::MAX,
        ];

        let mut buf = [0u8; 9];
        for &tc in &test_cases {
            let written = encode_varint(tc, &mut buf);
            assert!((1..=9).contains(&written));
            let (decoded, read_len) = decode_varint(&buf[..written]).expect("Decode error");
            assert_eq!(decoded, tc, "Mismatch for value: {tc}");
            assert_eq!(read_len, written);
        }
    }
}
