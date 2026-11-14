//! Pure Safe Rust SQLite 3 binary database reader (`.db`, `.sqlite`, `.sqlite3`).
//!
//! Reads SQLite3 B-Tree pages, overflow chains, varints, and typed records directly
//! without linking `libsqlite3-sys` C code, eliminating symbol conflicts and native C CVEs.
//!
//! Complies strictly with `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

use crate::Connection;
use crate::error::{Error, Result};
use crate::lakehouse::csv_stream::IngestStats;
use crate::traits::{Row, Value};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Instant;

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// Text encoding reported by the SQLite database header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteEncoding {
    /// UTF-8 (header value 1).
    Utf8,
    /// UTF-16 little-endian (header value 2).
    Utf16Le,
    /// UTF-16 big-endian (header value 3).
    Utf16Be,
}

/// Metadata describing one user table inside a `.sqlite` / `.db` file.
#[derive(Debug, Clone)]
pub struct SqliteTableInfo {
    /// Table name.
    pub name: String,
    /// 1-based root page index in the SQLite B-Tree.
    pub root_page: u32,
    /// Column names extracted from `CREATE TABLE` DDL.
    pub columns: Vec<String>,
    /// Column SQL types extracted from `CREATE TABLE` DDL (mapped to Tapirus types).
    pub column_types: Vec<String>,
    /// Optional 0-based index of the `INTEGER PRIMARY KEY` column (alias for `rowid`).
    pub integer_pk_index: Option<usize>,
}

/// Pure safe Rust reader for SQLite 3 binary database files.
pub struct SqliteFileReader {
    data: Vec<u8>,
    page_size: usize,
    usable_size: usize,
    encoding: SqliteEncoding,
}

impl SqliteFileReader {
    /// Open and validate a SQLite3 binary file from disk.
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        if len < 100 {
            return Err(Error::Corrupted(format!(
                "File '{}' is too small to be a SQLite database",
                path.display()
            )));
        }
        let mut data = Vec::with_capacity(len as usize);
        file.seek(SeekFrom::Start(0))?;
        file.read_to_end(&mut data)?;
        Self::from_bytes(data)
    }

    /// Construct a reader from an in-memory SQLite3 file byte buffer.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        if data.len() < 100 || &data[0..16] != SQLITE_MAGIC {
            return Err(Error::Corrupted(
                "Invalid SQLite database header (expected 'SQLite format 3')".into(),
            ));
        }
        let raw_ps = u16::from_be_bytes([data[16], data[17]]);
        let page_size = if raw_ps == 1 {
            65536usize
        } else {
            raw_ps as usize
        };
        if !(512..=65536).contains(&page_size) || !page_size.is_power_of_two() {
            return Err(Error::Corrupted(format!(
                "Invalid SQLite page size: {page_size}"
            )));
        }
        let reserved = data[20] as usize;
        if reserved >= page_size {
            return Err(Error::Corrupted(
                "Invalid SQLite reserved page bytes".into(),
            ));
        }
        let usable_size = page_size - reserved;
        let enc_raw = u32::from_be_bytes([data[56], data[57], data[58], data[59]]);
        let encoding = match enc_raw {
            2 => SqliteEncoding::Utf16Le,
            3 => SqliteEncoding::Utf16Be,
            _ => SqliteEncoding::Utf8,
        };

        Ok(Self {
            data,
            page_size,
            usable_size,
            encoding,
        })
    }

    fn page_slice(&self, page_no: u32) -> Result<&[u8]> {
        if page_no == 0 {
            return Err(Error::Corrupted("SQLite page number cannot be 0".into()));
        }
        let start = ((page_no as usize) - 1) * self.page_size;
        let end = start + self.page_size;
        if end > self.data.len() {
            return Err(Error::Corrupted(format!(
                "SQLite page {page_no} out of bounds (file len {})",
                self.data.len()
            )));
        }
        Ok(&self.data[start..end])
    }

    /// List all user tables discovered in `sqlite_schema` (root page 1).
    pub fn list_tables(&self) -> Result<Vec<SqliteTableInfo>> {
        let records = self.read_btree_table(1, None)?;
        let mut tables = Vec::new();

        for (_rowid, values) in records {
            if values.len() < 5 {
                continue;
            }
            let obj_type = match &values[0] {
                Value::Text(s) => s.as_str(),
                _ => continue,
            };
            if !obj_type.eq_ignore_ascii_case("table") {
                continue;
            }
            let name = match &values[1] {
                Value::Text(s) => s.clone(),
                _ => continue,
            };
            if name.starts_with("sqlite_") {
                continue;
            }
            let root_page = match &values[3] {
                Value::Integer(p) if *p > 0 => *p as u32,
                _ => continue,
            };
            let sql = match &values[4] {
                Value::Text(s) => s.clone(),
                _ => String::new(),
            };
            let (columns, column_types, integer_pk_index) = parse_sqlite_create_table(&sql);
            tables.push(SqliteTableInfo {
                name,
                root_page,
                columns,
                column_types,
                integer_pk_index,
            });
        }

        Ok(tables)
    }

    /// Read all rows for the specified table metadata.
    pub fn read_table_rows(&self, table: &SqliteTableInfo) -> Result<Vec<Row>> {
        let records = self.read_btree_table(table.root_page, table.integer_pk_index)?;
        let mut rows = Vec::with_capacity(records.len());

        let fallback_col_count = records.first().map(|(_, v)| v.len()).unwrap_or(0);
        let col_names: Vec<String> = if !table.columns.is_empty() {
            table.columns.clone()
        } else {
            (0..fallback_col_count)
                .map(|i| format!("col_{i}"))
                .collect()
        };

        for (_rowid, mut values) in records {
            if values.len() < col_names.len() {
                values.resize(col_names.len(), Value::Null);
            } else if values.len() > col_names.len() {
                values.truncate(col_names.len());
            }
            rows.push(Row::new(col_names.clone(), values));
        }

        Ok(rows)
    }

    fn read_btree_table(
        &self,
        root_page: u32,
        integer_pk_index: Option<usize>,
    ) -> Result<Vec<(i64, Vec<Value>)>> {
        let mut out = Vec::new();
        let mut visited = HashSet::new();
        self.traverse_table_page(root_page, integer_pk_index, &mut visited, &mut out)?;
        Ok(out)
    }

    fn traverse_table_page(
        &self,
        page_no: u32,
        integer_pk_index: Option<usize>,
        visited: &mut HashSet<u32>,
        out: &mut Vec<(i64, Vec<Value>)>,
    ) -> Result<()> {
        if !visited.insert(page_no) {
            return Err(Error::Corrupted(format!(
                "Cycle detected in SQLite B-Tree at page {page_no}"
            )));
        }
        let page = self.page_slice(page_no)?;
        let hdr_off = if page_no == 1 { 100 } else { 0 };
        if hdr_off + 8 > page.len() {
            return Err(Error::Corrupted(
                "Truncated SQLite B-Tree page header".into(),
            ));
        }

        let page_type = page[hdr_off];
        let cell_count = u16::from_be_bytes([page[hdr_off + 3], page[hdr_off + 4]]) as usize;

        match page_type {
            0x05 => {
                // Interior Table B-Tree page (12-byte header)
                if hdr_off + 12 > page.len() {
                    return Err(Error::Corrupted("Truncated SQLite interior header".into()));
                }
                let right_child = u32::from_be_bytes([
                    page[hdr_off + 8],
                    page[hdr_off + 9],
                    page[hdr_off + 10],
                    page[hdr_off + 11],
                ]);
                let ptr_start = hdr_off + 12;
                for idx in 0..cell_count {
                    let p_off = ptr_start + idx * 2;
                    if p_off + 2 > page.len() {
                        break;
                    }
                    let cell_off = u16::from_be_bytes([page[p_off], page[p_off + 1]]) as usize;
                    if cell_off + 4 > page.len() {
                        continue;
                    }
                    let child = u32::from_be_bytes([
                        page[cell_off],
                        page[cell_off + 1],
                        page[cell_off + 2],
                        page[cell_off + 3],
                    ]);
                    if child != 0 {
                        self.traverse_table_page(child, integer_pk_index, visited, out)?;
                    }
                }
                if right_child != 0 {
                    self.traverse_table_page(right_child, integer_pk_index, visited, out)?;
                }
            }
            0x0d => {
                // Leaf Table B-Tree page (8-byte header)
                let ptr_start = hdr_off + 8;
                let u = self.usable_size;
                let x = u.saturating_sub(35);
                let m = ((u.saturating_sub(12)) * 32 / 255).saturating_sub(23);

                for idx in 0..cell_count {
                    let p_off = ptr_start + idx * 2;
                    if p_off + 2 > page.len() {
                        break;
                    }
                    let mut pos = u16::from_be_bytes([page[p_off], page[p_off + 1]]) as usize;
                    if pos >= page.len() {
                        continue;
                    }

                    let (payload_len_u64, n1) = read_sqlite_varint(&page[pos..])?;
                    pos += n1;
                    let (rowid_u64, n2) = read_sqlite_varint(&page[pos..])?;
                    pos += n2;
                    let rowid = rowid_u64 as i64;
                    let total_payload = payload_len_u64 as usize;

                    let inline_len = if total_payload <= x {
                        total_payload
                    } else {
                        let k = m + ((total_payload - m) % (u.saturating_sub(4).max(1)));
                        if k <= x { k } else { m }
                    };

                    if pos + inline_len > page.len() {
                        return Err(Error::Corrupted(format!(
                            "SQLite cell payload overflows page {page_no}"
                        )));
                    }

                    let mut payload = Vec::with_capacity(total_payload);
                    payload.extend_from_slice(&page[pos..pos + inline_len]);
                    pos += inline_len;

                    if inline_len < total_payload {
                        if pos + 4 > page.len() {
                            return Err(Error::Corrupted(
                                "Missing SQLite overflow page pointer".into(),
                            ));
                        }
                        let mut ovf_page = u32::from_be_bytes([
                            page[pos],
                            page[pos + 1],
                            page[pos + 2],
                            page[pos + 3],
                        ]);
                        let mut ovf_seen = 0usize;
                        while ovf_page != 0 && payload.len() < total_payload {
                            ovf_seen += 1;
                            if ovf_seen > 1_000_000 {
                                return Err(Error::Corrupted(
                                    "SQLite overflow chain exceeded safety limit".into(),
                                ));
                            }
                            let op = self.page_slice(ovf_page)?;
                            let next_ovf = u32::from_be_bytes([op[0], op[1], op[2], op[3]]);
                            let avail = (u.saturating_sub(4)).min(op.len().saturating_sub(4));
                            let needed = (total_payload - payload.len()).min(avail);
                            payload.extend_from_slice(&op[4..4 + needed]);
                            ovf_page = next_ovf;
                        }
                    }

                    let mut values = decode_sqlite_record(&payload, self.encoding)?;
                    if let Some(pk_idx) = integer_pk_index
                        && pk_idx < values.len()
                        && values[pk_idx].is_null()
                    {
                        values[pk_idx] = Value::Integer(rowid);
                    }
                    out.push((rowid, values));
                }
            }
            _ => {
                return Err(Error::Corrupted(format!(
                    "Unexpected SQLite B-Tree page type 0x{page_type:02X} on page {page_no}"
                )));
            }
        }
        Ok(())
    }
}

/// Import one table (or all user tables when `source_table` is `None`) from a SQLite `.db` file
/// directly into a TapirusDB `Connection`.
pub fn import_sqlite_file(
    conn: &Connection,
    sqlite_path: &Path,
    source_table: Option<&str>,
    dest_table: Option<&str>,
    batch_size: usize,
) -> Result<Vec<IngestStats>> {
    let reader = SqliteFileReader::open(sqlite_path)?;
    let tables = reader.list_tables()?;
    if tables.is_empty() {
        return Err(Error::Corrupted(format!(
            "No user tables found in SQLite database '{}'",
            sqlite_path.display()
        )));
    }

    let selected: Vec<&SqliteTableInfo> = if let Some(req) = source_table {
        let found = tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(req))
            .ok_or_else(|| Error::TableNotFound(req.to_string()))?;
        vec![found]
    } else {
        tables.iter().collect()
    };

    let mut all_stats = Vec::with_capacity(selected.len());
    let eff_batch = batch_size.max(1);

    for t in selected {
        let start = Instant::now();
        let target_name = if let Some(custom) = dest_table
            && source_table.is_some()
        {
            custom.to_string()
        } else {
            t.name.clone()
        };

        let col_names = if t.columns.is_empty() {
            Vec::new()
        } else {
            t.columns.clone()
        };

        let rows = reader.read_table_rows(t)?;
        let actual_cols = if !col_names.is_empty() {
            col_names
        } else if let Some(first) = rows.first() {
            first.columns().to_vec()
        } else {
            continue;
        };

        if conn.table(&target_name).is_none() {
            let mut defs = Vec::with_capacity(actual_cols.len());
            for (idx, col) in actual_cols.iter().enumerate() {
                let raw_type = t
                    .column_types
                    .get(idx)
                    .map(|s| s.as_str())
                    .unwrap_or("TEXT");
                let mapped = map_sqlite_type_to_tapir(raw_type);
                if t.integer_pk_index == Some(idx) {
                    defs.push(format!("{col} INTEGER PRIMARY KEY"));
                } else {
                    defs.push(format!("{col} {mapped}"));
                }
            }
            let create_sql = format!(
                "CREATE TABLE IF NOT EXISTS {} ({});",
                target_name,
                defs.join(", ")
            );
            conn.execute(&create_sql)?;
        }

        let mut inserted = 0usize;
        for chunk in rows.chunks(eff_batch) {
            let batch: Vec<Vec<Value>> = chunk.iter().map(|r| r.values().to_vec()).collect();
            inserted += conn.insert_rows_batch(&target_name, Some(&actual_cols), &batch)?;
        }

        all_stats.push(IngestStats {
            rows_ingested: inserted,
            files_processed: 1,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            target: target_name,
            columns: actual_cols.len(),
        });
    }

    Ok(all_stats)
}

/// Read rows from one named table inside a SQLite `.db` file (for `SELECT * FROM read_sqlite(...)`).
pub fn scan_sqlite_table_rows(
    sqlite_path: &Path,
    table_name: &str,
    limit: Option<usize>,
) -> Result<Vec<Row>> {
    let reader = SqliteFileReader::open(sqlite_path)?;
    let tables = reader.list_tables()?;
    let target = tables
        .into_iter()
        .find(|t| t.name.eq_ignore_ascii_case(table_name))
        .ok_or_else(|| Error::TableNotFound(table_name.to_string()))?;
    let mut rows = reader.read_table_rows(&target)?;
    if let Some(lim) = limit {
        rows.truncate(lim);
    }
    Ok(rows)
}

/// Decode SQLite big-endian 1-to-9 byte varint.
#[inline]
fn read_sqlite_varint(buf: &[u8]) -> Result<(u64, usize)> {
    if buf.is_empty() {
        return Err(Error::Corrupted(
            "Unexpected EOF reading SQLite varint".into(),
        ));
    }
    let mut result = 0u64;
    for (i, &b) in buf.iter().take(8).enumerate() {
        if (b & 0x80) == 0 {
            result = (result << 7) | (b as u64);
            return Ok((result, i + 1));
        }
        result = (result << 7) | ((b & 0x7f) as u64);
    }
    if buf.len() < 9 {
        return Err(Error::Corrupted("Truncated 9-byte SQLite varint".into()));
    }
    result = (result << 8) | (buf[8] as u64);
    Ok((result, 9))
}

fn decode_sqlite_record(payload: &[u8], encoding: SqliteEncoding) -> Result<Vec<Value>> {
    let (hdr_len_u64, first_len) = read_sqlite_varint(payload)?;
    let hdr_len = hdr_len_u64 as usize;
    if hdr_len > payload.len() || hdr_len < first_len {
        return Err(Error::Corrupted(
            "Invalid SQLite record header length".into(),
        ));
    }

    let mut serial_types = Vec::new();
    let mut hdr_pos = first_len;
    while hdr_pos < hdr_len {
        let (st, used) = read_sqlite_varint(&payload[hdr_pos..hdr_len])?;
        serial_types.push(st);
        hdr_pos += used;
    }

    let mut data_pos = hdr_len;
    let mut values = Vec::with_capacity(serial_types.len());

    for st in serial_types {
        let val = match st {
            0 => Value::Null,
            1 => {
                if data_pos + 1 > payload.len() {
                    return Err(Error::Corrupted("Truncated i8 in SQLite record".into()));
                }
                let v = (payload[data_pos] as i8) as i64;
                data_pos += 1;
                Value::Integer(v)
            }
            2 => {
                if data_pos + 2 > payload.len() {
                    return Err(Error::Corrupted("Truncated i16 in SQLite record".into()));
                }
                let v = i16::from_be_bytes([payload[data_pos], payload[data_pos + 1]]) as i64;
                data_pos += 2;
                Value::Integer(v)
            }
            3 => {
                if data_pos + 3 > payload.len() {
                    return Err(Error::Corrupted("Truncated i24 in SQLite record".into()));
                }
                let raw = ((payload[data_pos] as i32) << 16)
                    | ((payload[data_pos + 1] as i32) << 8)
                    | (payload[data_pos + 2] as i32);
                let signed = (raw << 8) >> 8;
                data_pos += 3;
                Value::Integer(signed as i64)
            }
            4 => {
                if data_pos + 4 > payload.len() {
                    return Err(Error::Corrupted("Truncated i32 in SQLite record".into()));
                }
                let v = i32::from_be_bytes([
                    payload[data_pos],
                    payload[data_pos + 1],
                    payload[data_pos + 2],
                    payload[data_pos + 3],
                ]) as i64;
                data_pos += 4;
                Value::Integer(v)
            }
            5 => {
                if data_pos + 6 > payload.len() {
                    return Err(Error::Corrupted("Truncated i48 in SQLite record".into()));
                }
                let mut raw = 0i64;
                for &b in &payload[data_pos..data_pos + 6] {
                    raw = (raw << 8) | (b as i64);
                }
                let signed = (raw << 16) >> 16;
                data_pos += 6;
                Value::Integer(signed)
            }
            6 => {
                if data_pos + 8 > payload.len() {
                    return Err(Error::Corrupted("Truncated i64 in SQLite record".into()));
                }
                let v = i64::from_be_bytes(payload[data_pos..data_pos + 8].try_into().unwrap());
                data_pos += 8;
                Value::Integer(v)
            }
            7 => {
                if data_pos + 8 > payload.len() {
                    return Err(Error::Corrupted("Truncated f64 in SQLite record".into()));
                }
                let v = f64::from_be_bytes(payload[data_pos..data_pos + 8].try_into().unwrap());
                data_pos += 8;
                Value::Real(v)
            }
            8 => Value::Integer(0),
            9 => Value::Integer(1),
            10 | 11 => Value::Null,
            n if n >= 12 && n % 2 == 0 => {
                let len = ((n - 12) / 2) as usize;
                if data_pos + len > payload.len() {
                    return Err(Error::Corrupted("Truncated BLOB in SQLite record".into()));
                }
                let bytes = payload[data_pos..data_pos + len].to_vec();
                data_pos += len;
                Value::Blob(bytes)
            }
            n => {
                let len = ((n - 13) / 2) as usize;
                if data_pos + len > payload.len() {
                    return Err(Error::Corrupted("Truncated TEXT in SQLite record".into()));
                }
                let slice = &payload[data_pos..data_pos + len];
                data_pos += len;
                let s = decode_sqlite_text(slice, encoding);
                Value::Text(s)
            }
        };
        values.push(val);
    }

    Ok(values)
}

fn decode_sqlite_text(bytes: &[u8], encoding: SqliteEncoding) -> String {
    match encoding {
        SqliteEncoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        SqliteEncoding::Utf16Le => {
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_le_bytes(c))
                .collect();
            String::from_utf16_lossy(&units)
        }
        SqliteEncoding::Utf16Be => {
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_be_bytes(c))
                .collect();
            String::from_utf16_lossy(&units)
        }
    }
}

fn parse_sqlite_create_table(sql: &str) -> (Vec<String>, Vec<String>, Option<usize>) {
    let mut cols = Vec::new();
    let mut types = Vec::new();
    let mut pk_idx = None;

    let Some(open) = sql.find('(') else {
        return (cols, types, pk_idx);
    };
    let Some(close) = sql.rfind(')') else {
        return (cols, types, pk_idx);
    };
    if close <= open {
        return (cols, types, pk_idx);
    }
    let body = &sql[open + 1..close];

    // Split top-level comma items (respecting parentheses like DECIMAL(10, 2) and quotes).
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    for ch in body.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                cur.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                cur.push(ch);
            }
            '(' if !in_single && !in_double => {
                depth += 1;
                cur.push(ch);
            }
            ')' if !in_single && !in_double => {
                depth = depth.saturating_sub(1);
                cur.push(ch);
            }
            ',' if depth == 0 && !in_single && !in_double => {
                items.push(std::mem::take(&mut cur));
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        items.push(cur);
    }

    for item in items {
        let trimmed = item.trim();
        let upper = trimmed.to_ascii_uppercase();
        if upper.starts_with("PRIMARY KEY")
            || upper.starts_with("FOREIGN KEY")
            || upper.starts_with("UNIQUE")
            || upper.starts_with("CHECK")
            || upper.starts_with("CONSTRAINT")
        {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(raw_name) = parts.next() else {
            continue;
        };
        let col_name = raw_name
            .trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
            .to_string();
        let type_token = parts.next().unwrap_or("TEXT").to_ascii_uppercase();
        if upper.contains("INTEGER PRIMARY KEY") || upper.contains("INT PRIMARY KEY") {
            pk_idx = Some(cols.len());
        }
        cols.push(col_name);
        types.push(type_token);
    }

    (cols, types, pk_idx)
}

/// Map SQLite / PostgreSQL / MySQL / DuckDB column type strings to Tapirus SQL types.
pub fn map_sqlite_type_to_tapir(raw: &str) -> &'static str {
    let u = raw.to_ascii_uppercase();
    if u.contains("INT") || u.contains("SERIAL") || u.contains("BOOL") {
        "INTEGER"
    } else if u.contains("REAL")
        || u.contains("FLOA")
        || u.contains("DOUB")
        || u.contains("NUM")
        || u.contains("DEC")
    {
        "REAL"
    } else if u.contains("BLOB") || u.contains("BYTEA") || u.contains("BIN") {
        "BLOB"
    } else {
        "TEXT"
    }
}
