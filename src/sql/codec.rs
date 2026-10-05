//! Binary record codec for serializing and deserializing SQL/Document values.
//!
//! Provides ultra-compact, high-speed binary packing for `Row` and `Value` types.

use crate::btree::{decode_varint, encode_varint};
use crate::error::{Error, Result};
use crate::traits::{Row, Value};

const TAG_NULL: u8 = 0x00;
const TAG_INT_0: u8 = 0x01;
const TAG_INT_1: u8 = 0x02;
const TAG_INT_I8: u8 = 0x03;
const TAG_INT_I16: u8 = 0x04;
const TAG_INT_I32: u8 = 0x05;
const TAG_INT_I64: u8 = 0x06;
const TAG_REAL: u8 = 0x07;
const TAG_TEXT: u8 = 0x08;
const TAG_BLOB: u8 = 0x09;
const TAG_VECTOR: u8 = 0x0A;

/// Encodes a slice of `Value`s into a compact binary tuple payload.
pub fn encode_row(values: &[Value]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(values.len() * 10);
    encode_row_into(values, &mut buf);
    buf
}

/// Encodes a slice of `Value`s into an existing buffer, clearing it first. Reuses the buffer's
/// heap allocation across multiple calls to avoid per-row `Vec` allocations in hot insert loops.
pub fn encode_row_into(values: &[Value], out: &mut Vec<u8>) {
    out.clear();

    // Reserve space for header (col_count varint + one tag byte per column)
    let mut varint_buf = [0u8; 9];
    let n = encode_varint(values.len() as u64, &mut varint_buf);

    // We'll write header inline, then body. Collect tags first.

    out.extend_from_slice(&varint_buf[..n]);
    let tags_start = out.len();
    // Reserve tag bytes (one per column), will fill in below
    out.resize(tags_start + values.len(), 0);


    for (col_idx, val) in values.iter().enumerate() {
        match val {
            Value::Null => {
                out[tags_start + col_idx] = TAG_NULL;
            }
            Value::Integer(0) => {
                out[tags_start + col_idx] = TAG_INT_0;
            }
            Value::Integer(1) => {
                out[tags_start + col_idx] = TAG_INT_1;
            }
            Value::Integer(i) => {
                if let Ok(v) = i8::try_from(*i) {
                    out[tags_start + col_idx] = TAG_INT_I8;
                    out.push(v as u8);
                } else if let Ok(v) = i16::try_from(*i) {
                    out[tags_start + col_idx] = TAG_INT_I16;
                    out.extend_from_slice(&v.to_le_bytes());
                } else if let Ok(v) = i32::try_from(*i) {
                    out[tags_start + col_idx] = TAG_INT_I32;
                    out.extend_from_slice(&v.to_le_bytes());
                } else {
                    out[tags_start + col_idx] = TAG_INT_I64;
                    out.extend_from_slice(&i.to_le_bytes());
                }
            }
            Value::Real(r) => {
                out[tags_start + col_idx] = TAG_REAL;
                out.extend_from_slice(&r.to_le_bytes());
            }
            Value::Text(s) => {
                out[tags_start + col_idx] = TAG_TEXT;
                let bytes = s.as_bytes();
                let n = encode_varint(bytes.len() as u64, &mut varint_buf);
                out.extend_from_slice(&varint_buf[..n]);
                out.extend_from_slice(bytes);
            }
            Value::Blob(b) => {
                out[tags_start + col_idx] = TAG_BLOB;
                let n = encode_varint(b.len() as u64, &mut varint_buf);
                out.extend_from_slice(&varint_buf[..n]);
                out.extend_from_slice(b);
            }
            Value::Vector(v) => {
                out[tags_start + col_idx] = TAG_VECTOR;
                let dims = v.len() as u16;
                out.extend_from_slice(&dims.to_le_bytes());
                for f in v {
                    out.extend_from_slice(&f.to_le_bytes());
                }
            }
        }
    }
}

/// Decodes a binary payload into a list of `Value`s.
/// Decodes a binary payload into a list of `Value`s.
pub fn decode_row_values(bytes: &[u8]) -> Result<Vec<Value>> {
    let mut values = Vec::new();
    decode_row_values_into(bytes, &mut values)?;
    Ok(values)
}

/// Decodes a binary payload directly into an existing `Vec<Value>` buffer, reusing memory allocations.
/// If `needed_mask` is provided, columns where `needed_mask[idx] == false` skip heap allocation
/// (e.g. text/blob/vector payloads) and insert `Value::Null` instead.
pub fn decode_row_values_into_projected(
    bytes: &[u8],
    needed_mask: &[bool],
    values: &mut Vec<Value>,
) -> Result<()> {
    values.clear();
    if bytes.is_empty() {
        return Ok(());
    }

    let (col_count_u64, mut cursor) = decode_varint(bytes)?;
    let col_count = col_count_u64 as usize;

    if bytes.len() < cursor + col_count {
        return Err(Error::Corrupted("Record header truncated".into()));
    }

    let tags = &bytes[cursor..cursor + col_count];
    cursor += col_count;

    if values.capacity() < col_count {
        values.reserve(col_count - values.capacity());
    }

    for (col_idx, &tag) in tags.iter().enumerate() {
        let is_needed = needed_mask.get(col_idx).copied().unwrap_or(true);

        if !is_needed {
            // Fast skip without allocating heap buffers
            match tag {
                TAG_NULL | TAG_INT_0 | TAG_INT_1 => values.push(Value::Null),
                TAG_INT_I8 => {
                    cursor += 1;
                    values.push(Value::Null);
                }
                TAG_INT_I16 => {
                    cursor += 2;
                    values.push(Value::Null);
                }
                TAG_INT_I32 => {
                    cursor += 4;
                    values.push(Value::Null);
                }
                TAG_INT_I64 | TAG_REAL => {
                    cursor += 8;
                    values.push(Value::Null);
                }
                TAG_TEXT | TAG_BLOB => {
                    let (len, n) = decode_varint(&bytes[cursor..])?;
                    cursor += n + len as usize;
                    values.push(Value::Null);
                }
                TAG_VECTOR => {
                    if cursor + 2 > bytes.len() {
                        return Err(Error::Corrupted("Truncated Vector dimensions".into()));
                    }
                    let dims = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]) as usize;
                    cursor += 2 + dims * 4;
                    values.push(Value::Null);
                }
                _ => values.push(Value::Null),
            }
            continue;
        }

        match tag {
            TAG_NULL => values.push(Value::Null),
            TAG_INT_0 => values.push(Value::Integer(0)),
            TAG_INT_1 => values.push(Value::Integer(1)),
            TAG_INT_I8 => {
                if cursor >= bytes.len() {
                    return Err(Error::Corrupted("Truncated i8 value".into()));
                }
                let val = bytes[cursor] as i8;
                cursor += 1;
                values.push(Value::Integer(val as i64));
            }
            TAG_INT_I16 => {
                if cursor + 2 > bytes.len() {
                    return Err(Error::Corrupted("Truncated i16 value".into()));
                }
                let val = i16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]);
                cursor += 2;
                values.push(Value::Integer(val as i64));
            }
            TAG_INT_I32 => {
                if cursor + 4 > bytes.len() {
                    return Err(Error::Corrupted("Truncated i32 value".into()));
                }
                let val = i32::from_le_bytes([
                    bytes[cursor],
                    bytes[cursor + 1],
                    bytes[cursor + 2],
                    bytes[cursor + 3],
                ]);
                cursor += 4;
                values.push(Value::Integer(val as i64));
            }
            TAG_INT_I64 => {
                if cursor + 8 > bytes.len() {
                    return Err(Error::Corrupted("Truncated i64 value".into()));
                }
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&bytes[cursor..cursor + 8]);
                let val = i64::from_le_bytes(buf);
                cursor += 8;
                values.push(Value::Integer(val));
            }
            TAG_REAL => {
                if cursor + 8 > bytes.len() {
                    return Err(Error::Corrupted("Truncated Real value".into()));
                }
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&bytes[cursor..cursor + 8]);
                let val = f64::from_le_bytes(buf);
                cursor += 8;
                values.push(Value::Real(val));
            }
            TAG_TEXT => {
                let (len, n) = decode_varint(&bytes[cursor..])?;
                cursor += n;
                let text_len = len as usize;
                if cursor + text_len > bytes.len() {
                    return Err(Error::Corrupted("Truncated Text payload".into()));
                }
                let s = std::str::from_utf8(&bytes[cursor..cursor + text_len])
                    .map_err(|_| Error::Corrupted("Invalid UTF-8 string".into()))?;
                cursor += text_len;
                values.push(Value::Text(s.to_string()));
            }
            TAG_BLOB => {
                let (len, n) = decode_varint(&bytes[cursor..])?;
                cursor += n;
                let blob_len = len as usize;
                if cursor + blob_len > bytes.len() {
                    return Err(Error::Corrupted("Truncated Blob payload".into()));
                }
                let b = bytes[cursor..cursor + blob_len].to_vec();
                cursor += blob_len;
                values.push(Value::Blob(b));
            }
            TAG_VECTOR => {
                if cursor + 2 > bytes.len() {
                    return Err(Error::Corrupted("Truncated Vector dimensions".into()));
                }
                let dims = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]) as usize;
                cursor += 2;
                let needed_bytes = dims * 4;
                if cursor + needed_bytes > bytes.len() {
                    return Err(Error::Corrupted("Truncated Vector float payload".into()));
                }
                let mut vec_data = Vec::with_capacity(dims);
                for _ in 0..dims {
                    let f = f32::from_le_bytes([
                        bytes[cursor],
                        bytes[cursor + 1],
                        bytes[cursor + 2],
                        bytes[cursor + 3],
                    ]);
                    cursor += 4;
                    vec_data.push(f);
                }
                values.push(Value::Vector(vec_data));
            }
            other => {
                return Err(Error::Corrupted(format!(
                    "Unknown type tag in record header: {other:#x}"
                )))
            }
        }
    }

    Ok(())
}

/// Decodes a binary payload directly into an existing `Vec<Value>` buffer, reusing memory allocations
pub fn decode_row_values_into(bytes: &[u8], values: &mut Vec<Value>) -> Result<()> {
    decode_row_values_into_projected(bytes, &[], values)
}

/// Decodes binary bytes into a full `Row` with shared column names Arc
pub fn decode_row_shared(bytes: &[u8], column_names: &std::sync::Arc<Vec<String>>) -> Result<Row> {
    let mut values = decode_row_values(bytes)?;
    while values.len() < column_names.len() {
        values.push(Value::Null);
    }
    Ok(Row::with_shared_columns(column_names.clone(), values))
}

/// Decodes binary bytes into a full `Row` with shared column names Arc, skipping unneeded columns via `needed_mask`
pub fn decode_row_shared_projected(
    bytes: &[u8],
    column_names: &std::sync::Arc<Vec<String>>,
    needed_mask: &[bool],
) -> Result<Row> {
    let mut values = Vec::with_capacity(column_names.len());
    decode_row_values_into_projected(bytes, needed_mask, &mut values)?;
    while values.len() < column_names.len() {
        values.push(Value::Null);
    }
    Ok(Row::with_shared_columns(column_names.clone(), values))
}

/// Decodes binary bytes into a full `Row` with column names.
///
/// Supports schema evolution (`ALTER TABLE ADD COLUMN`): if the binary tuple
/// contains fewer values than the current schema columns, missing values
/// are automatically padded with `Value::Null`.
pub fn decode_row(bytes: &[u8], column_names: &[String]) -> Result<Row> {
    let mut values = decode_row_values(bytes)?;
    while values.len() < column_names.len() {
        values.push(Value::Null);
    }
    Ok(Row::new(column_names.to_vec(), values))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_codec_roundtrip() {
        let original_values = vec![
            Value::Integer(0),
            Value::Integer(1),
            Value::Integer(120),
            Value::Integer(-32000),
            Value::Integer(1_000_000),
            Value::Integer(9_000_000_000_000),
            Value::Real(std::f64::consts::PI),
            Value::Text("Safe Rust Database 🦛".to_string()),
            Value::Blob(vec![0xDE, 0xAD, 0xBE, 0xEF]),
            Value::Vector(vec![0.1, 0.2, 0.3, 0.4, 0.5]),
            Value::Null,
        ];

        let encoded = encode_row(&original_values);
        let decoded = decode_row_values(&encoded).expect("Decoding failed");

        assert_eq!(original_values.len(), decoded.len());
        for (idx, (orig, dec)) in original_values.iter().zip(decoded.iter()).enumerate() {
            match (orig, dec) {
                (Value::Real(r1), Value::Real(r2)) => {
                    assert!((r1 - r2).abs() < 1e-6, "Real mismatch at index {idx}");
                }
                _ => assert_eq!(orig, dec, "Mismatch at index {idx}"),
            }
        }
    }
}
