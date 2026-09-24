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
    let mut header = Vec::new();
    let mut body = Vec::new();

    // 1. Column count as varint
    let mut varint_buf = [0u8; 9];
    let n = encode_varint(values.len() as u64, &mut varint_buf);
    header.extend_from_slice(&varint_buf[..n]);

    for val in values {
        match val {
            Value::Null => {
                header.push(TAG_NULL);
            }
            Value::Integer(0) => {
                header.push(TAG_INT_0);
            }
            Value::Integer(1) => {
                header.push(TAG_INT_1);
            }
            Value::Integer(i) => {
                if let Ok(v) = i8::try_from(*i) {
                    header.push(TAG_INT_I8);
                    body.push(v as u8);
                } else if let Ok(v) = i16::try_from(*i) {
                    header.push(TAG_INT_I16);
                    body.extend_from_slice(&v.to_le_bytes());
                } else if let Ok(v) = i32::try_from(*i) {
                    header.push(TAG_INT_I32);
                    body.extend_from_slice(&v.to_le_bytes());
                } else {
                    header.push(TAG_INT_I64);
                    body.extend_from_slice(&i.to_le_bytes());
                }
            }
            Value::Real(r) => {
                header.push(TAG_REAL);
                body.extend_from_slice(&r.to_le_bytes());
            }
            Value::Text(s) => {
                header.push(TAG_TEXT);
                let bytes = s.as_bytes();
                let n = encode_varint(bytes.len() as u64, &mut varint_buf);
                body.extend_from_slice(&varint_buf[..n]);
                body.extend_from_slice(bytes);
            }
            Value::Blob(b) => {
                header.push(TAG_BLOB);
                let n = encode_varint(b.len() as u64, &mut varint_buf);
                body.extend_from_slice(&varint_buf[..n]);
                body.extend_from_slice(b);
            }
            Value::Vector(v) => {
                header.push(TAG_VECTOR);
                let dims = v.len() as u16;
                body.extend_from_slice(&dims.to_le_bytes());
                for f in v {
                    body.extend_from_slice(&f.to_le_bytes());
                }
            }
        }
    }

    // Combine header and body
    let mut result = Vec::with_capacity(header.len() + body.len());
    result.extend_from_slice(&header);
    result.extend_from_slice(&body);
    result
}

/// Decodes a binary payload into a list of `Value`s.
pub fn decode_row_values(bytes: &[u8]) -> Result<Vec<Value>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }

    let (col_count_u64, mut cursor) = decode_varint(bytes)?;
    let col_count = col_count_u64 as usize;

    if bytes.len() < cursor + col_count {
        return Err(Error::Corrupted("Record header truncated".into()));
    }

    let tags = &bytes[cursor..cursor + col_count];
    cursor += col_count;

    let mut values = Vec::with_capacity(col_count);

    for &tag in tags {
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

    Ok(values)
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
