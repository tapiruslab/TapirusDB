//! Pure Safe Rust LZ4 Block Compression Engine.
//!
//! Provides transparent, high-throughput lossless compression for 4KB/8KB slotted
//! pages in TapirusDB without any external dependencies or unsafe code.
//!
//! Complies strictly with `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

use crate::error::{Error, Result};

const MIN_MATCH: usize = 4;
const HASH_TABLE_SIZE: usize = 4096;
const MAX_DISTANCE: usize = 65535;

/// Magic tag identifying a TapirusDB compressed page payload
pub const COMPRESSED_PAGE_MAGIC: u8 = 0x54; // 'T'
/// Flag indicating that page payload bytes are uncompressed
pub const UNCOMPRESSED_PAGE_FLAG: u8 = 0x00;
/// Flag indicating that page payload bytes are compressed using LZ4
pub const LZ4_COMPRESSED_PAGE_FLAG: u8 = 0x01;

/// Hash a 4-byte sequence for hash table lookup
#[inline(always)]
fn hash_4(bytes: &[u8]) -> usize {
    let val = (bytes[0] as usize)
        | ((bytes[1] as usize) << 8)
        | ((bytes[2] as usize) << 16)
        | ((bytes[3] as usize) << 24);
    // Knuth multiplicative hash
    (val.wrapping_mul(2654435761) >> 20) % HASH_TABLE_SIZE
}

/// Compress an arbitrary byte slice using LZ4 block encoding in pure safe Rust.
pub fn compress_lz4_block(src: &[u8]) -> Vec<u8> {
    if src.len() < MIN_MATCH {
        let mut out = Vec::with_capacity(src.len() + 2);
        let lit_len = src.len();
        out.push((lit_len.min(15) << 4) as u8);
        if lit_len >= 15 {
            let mut rem = lit_len - 15;
            while rem >= 255 {
                out.push(255);
                rem -= 255;
            }
            out.push(rem as u8);
        }
        out.extend_from_slice(src);
        return out;
    }

    let mut hash_table = [0usize; HASH_TABLE_SIZE];
    let mut out = Vec::with_capacity(src.len());

    let mut anchor = 0;
    let mut ip = 0;
    let end = src.len();
    let mflimit = if end > 12 { end - 12 } else { 0 };

    while ip < mflimit {
        let h = hash_4(&src[ip..ip + 4]);
        let match_pos = hash_table[h];
        hash_table[h] = ip;

        // Verify if match exists within MAX_DISTANCE and has at least 4 identical bytes
        if match_pos < ip
            && ip - match_pos <= MAX_DISTANCE
            && src[match_pos..match_pos + 4] == src[ip..ip + 4]
        {
            // Found a match!
            let lit_len = ip - anchor;

            // Determine length of match
            let mut match_len = 4;
            while (ip + match_len < end) && (src[match_pos + match_len] == src[ip + match_len]) {
                match_len += 1;
            }

            let token_lit = lit_len.min(15);
            let token_match = (match_len - 4).min(15);
            let token = ((token_lit << 4) | token_match) as u8;
            out.push(token);

            // Encode literal length overflow
            if lit_len >= 15 {
                let mut rem = lit_len - 15;
                while rem >= 255 {
                    out.push(255);
                    rem -= 255;
                }
                out.push(rem as u8);
            }

            // Write literals
            out.extend_from_slice(&src[anchor..ip]);

            // Write 2-byte little-endian offset
            let offset = (ip - match_pos) as u16;
            out.extend_from_slice(&offset.to_le_bytes());

            // Encode match length overflow
            if match_len - 4 >= 15 {
                let mut rem = (match_len - 4) - 15;
                while rem >= 255 {
                    out.push(255);
                    rem -= 255;
                }
                out.push(rem as u8);
            }

            ip += match_len;
            anchor = ip;

            // Update hash for boundary
            if ip < mflimit {
                let h_next = hash_4(&src[ip - 2..ip + 2]);
                hash_table[h_next] = ip - 2;
            }
        } else {
            ip += 1;
        }
    }

    // Write trailing literals
    let last_lit_len = end - anchor;
    let token = ((last_lit_len.min(15)) << 4) as u8;
    out.push(token);
    if last_lit_len >= 15 {
        let mut rem = last_lit_len - 15;
        while rem >= 255 {
            out.push(255);
            rem -= 255;
        }
        out.push(rem as u8);
    }
    out.extend_from_slice(&src[anchor..end]);

    out
}

/// Decompress an LZ4 block byte slice into a target output buffer in pure safe Rust.
pub fn decompress_lz4_block(src: &[u8], target_len: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(target_len);
    let mut ip = 0;

    while ip < src.len() && out.len() < target_len {
        let token = src[ip];
        ip += 1;

        // 1. Literal length
        let mut lit_len = (token >> 4) as usize;
        if lit_len == 15 {
            while ip < src.len() {
                let extra = src[ip] as usize;
                ip += 1;
                lit_len += extra;
                if extra != 255 {
                    break;
                }
            }
        }

        // Copy literals
        if ip + lit_len > src.len() {
            return Err(Error::Corrupted("LZ4 literal run out of bounds".into()));
        }
        out.extend_from_slice(&src[ip..ip + lit_len]);
        ip += lit_len;

        if out.len() >= target_len || ip >= src.len() {
            break;
        }

        // 2. Match offset (2 bytes LE)
        if ip + 2 > src.len() {
            return Err(Error::Corrupted("LZ4 match offset truncated".into()));
        }
        let offset = u16::from_le_bytes([src[ip], src[ip + 1]]) as usize;
        ip += 2;

        if offset == 0 || offset > out.len() {
            return Err(Error::Corrupted(format!(
                "LZ4 invalid back-reference offset: {offset} (current out len: {})",
                out.len()
            )));
        }

        // 3. Match length
        let mut match_len = ((token & 0x0F) as usize) + 4;
        if (token & 0x0F) == 15 {
            while ip < src.len() {
                let extra = src[ip] as usize;
                ip += 1;
                match_len += extra;
                if extra != 255 {
                    break;
                }
            }
        }

        // Copy match from previous output buffer bytes
        let match_start = out.len() - offset;
        for i in 0..match_len {
            let byte = out[match_start + i];
            out.push(byte);
        }
    }

    Ok(out)
}

/// Compress a database page frame. If compressed size is smaller than original, returns
/// tagged compressed payload. Otherwise, returns tagged uncompressed payload.
pub fn compress_page_frame(raw_page: &[u8]) -> Vec<u8> {
    let compressed = compress_lz4_block(raw_page);
    let original_len = raw_page.len() as u16;

    // We need 4 bytes header: [COMPRESSED_PAGE_MAGIC, FLAG, LEN_LO, LEN_HI]
    if compressed.len() + 4 < raw_page.len() {
        let mut out = Vec::with_capacity(4 + compressed.len());
        out.push(COMPRESSED_PAGE_MAGIC);
        out.push(LZ4_COMPRESSED_PAGE_FLAG);
        out.extend_from_slice(&original_len.to_le_bytes());
        out.extend_from_slice(&compressed);
        out
    } else {
        let mut out = Vec::with_capacity(4 + raw_page.len());
        out.push(COMPRESSED_PAGE_MAGIC);
        out.push(UNCOMPRESSED_PAGE_FLAG);
        out.extend_from_slice(&original_len.to_le_bytes());
        out.extend_from_slice(raw_page);
        out
    }
}

/// Decompress a page frame back to its full original page size.
pub fn decompress_page_frame(frame: &[u8], default_page_size: usize) -> Result<Vec<u8>> {
    if frame.len() < 4 || frame[0] != COMPRESSED_PAGE_MAGIC {
        // Plain uncompressed page image
        return Ok(frame.to_vec());
    }

    let flag = frame[1];
    let original_len = u16::from_le_bytes([frame[2], frame[3]]) as usize;
    let expected_len = if original_len > 0 { original_len } else { default_page_size };

    match flag {
        UNCOMPRESSED_PAGE_FLAG => Ok(frame[4..].to_vec()),
        LZ4_COMPRESSED_PAGE_FLAG => decompress_lz4_block(&frame[4..], expected_len),
        _ => Ok(frame.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lz4_roundtrip_slotted_page() {
        // Create synthetic 4096-byte page with repeated JSON and text
        let mut page = vec![0u8; 4096];
        let sample = b"{\"id\": 42, \"name\": \"TapirusDB Industrial Supremacy\", \"active\": true, \"scores\": [1.0, 2.5]}";
        for chunk in page.chunks_mut(sample.len()) {
            let len = chunk.len().min(sample.len());
            chunk[..len].copy_from_slice(&sample[..len]);
        }

        let compressed_frame = compress_page_frame(&page);
        assert!(compressed_frame.len() < page.len() / 2, "LZ4 should compress repetitive page by >50%");

        let decompressed = decompress_page_frame(&compressed_frame, 4096).expect("Decompression should succeed");
        assert_eq!(decompressed.len(), 4096);
        assert_eq!(decompressed, page);
    }

    #[test]
    fn test_lz4_incompressible_fallback() {
        // High entropy pseudo-random sequence
        let mut page = vec![0u8; 512];
        for (i, byte) in page.iter_mut().enumerate() {
            *byte = ((i * 199 + 37) % 256) as u8;
        }

        let compressed_frame = compress_page_frame(&page);
        let decompressed = decompress_page_frame(&compressed_frame, 512).expect("Decompression fallback succeeds");
        assert_eq!(decompressed, page);
    }
}
