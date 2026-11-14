//! Ultra-fast streaming CSV, TSV, and JSONL ingestion and export engine.
//!
//! Bypasses per-row SQL string formatting and parser overhead by streaming parsed
//! batches directly into TapirusDB's native batch B+Tree storage engine.
//!
//! Complies strictly with `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

use crate::Connection;
use crate::error::{Error, Result};
use crate::sql::catalog::DataType;
use crate::traits::{Row, Value};
use serde_json::Value as JsonValue;
use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::time::Instant;

/// Streaming ingestion statistics for CSV/JSONL/Parquet/SQLite/SQL/Text imports.
#[derive(Debug, Clone, Default)]
pub struct IngestStats {
    /// Total rows or chunks ingested into TapirusDB.
    pub rows_ingested: usize,
    /// Number of files processed (supports globs & Hive partitions).
    pub files_processed: usize,
    /// Elapsed wall-clock time in milliseconds.
    pub elapsed_ms: f64,
    /// Target table, collection, or namespace name.
    pub target: String,
    /// Number of columns or fields in target table (when tabular).
    pub columns: usize,
}

/// Streaming export statistics.
#[derive(Debug, Clone, Default)]
pub struct ExportStats {
    /// Total rows written to the output target.
    pub rows_exported: usize,
    /// Output files written (more than 1 when partitioned).
    pub files_written: usize,
    /// Elapsed wall-clock time in milliseconds.
    pub elapsed_ms: f64,
}

fn open_buffered_reader(path: &Path) -> Result<Box<dyn BufRead>> {
    let file = File::open(path)?;
    let lower = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    #[cfg(feature = "lakehouse")]
    if lower.ends_with(".gz") {
        let decoder = flate2::read::GzDecoder::new(file);
        return Ok(Box::new(BufReader::with_capacity(256 * 1024, decoder)));
    }
    #[cfg(not(feature = "lakehouse"))]
    if lower.ends_with(".gz") {
        return Err(Error::Corrupted(
            "Gzip (.gz) decompression requires the 'lakehouse' feature".into(),
        ));
    }

    Ok(Box::new(BufReader::with_capacity(256 * 1024, file)))
}

/// Detect the most likely delimiter from a header line.
pub fn detect_delimiter(header: &str, default_delim: char) -> char {
    if default_delim == '\t' {
        return '\t';
    }
    let candidates = [',', '\t', ';', '|'];
    let mut best = default_delim;
    let mut best_count = 0usize;
    for cand in candidates {
        let count = count_unquoted_chars(header, cand);
        if count > best_count {
            best_count = count;
            best = cand;
        }
    }
    best
}

fn count_unquoted_chars(line: &str, target: char) -> usize {
    let mut in_quotes = false;
    let mut count = 0usize;
    for ch in line.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == target && !in_quotes {
            count += 1;
        }
    }
    count
}

/// Sanitize raw header strings into valid SQL column identifiers, deduplicating collisions.
pub fn sanitize_column_names(raw: &[String]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::with_capacity(raw.len());
    for (idx, item) in raw.iter().enumerate() {
        let trimmed = item.trim().trim_start_matches('\u{feff}');
        let mut clean: String = trimmed
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if clean.is_empty() || clean.chars().all(|c| c == '_') {
            clean = format!("col_{idx}");
        }
        if clean.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            clean.insert_str(0, "c_");
        }
        let lower = clean.to_ascii_lowercase();
        let entry = seen.entry(lower).or_insert(0);
        if *entry > 0 {
            clean = format!("{clean}_{}", *entry);
        }
        *entry += 1;
        out.push(clean);
    }
    out
}

/// Parse one RFC-4180 CSV/TSV record into string fields using a reusable buffer,
/// keeping bracketed vector literals (`[0.1, 0.2, ...]`) intact even if unquoted.
pub fn parse_delimited_line(line: &str, delimiter: char, out: &mut Vec<String>) {
    out.clear();
    let line = line.trim_end_matches(&['\r', '\n'][..]);
    let mut current = String::with_capacity(32);
    let mut in_quotes = false;
    let mut bracket_depth = 0usize;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '"' {
            if in_quotes {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    current.push('"');
                } else {
                    in_quotes = false;
                }
            } else if current.is_empty() {
                in_quotes = true;
            } else {
                current.push('"');
            }
        } else if c == '[' && !in_quotes {
            bracket_depth += 1;
            current.push(c);
        } else if c == ']' && !in_quotes {
            bracket_depth = bracket_depth.saturating_sub(1);
            current.push(c);
        } else if c == delimiter && !in_quotes && bracket_depth == 0 {
            out.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    out.push(current);
}

/// Read logical CSV records supporting embedded newlines inside quoted fields.
fn read_csv_record<R: BufRead + ?Sized>(reader: &mut R, buf: &mut String) -> std::io::Result<bool> {
    buf.clear();
    let n = reader.read_line(buf)?;
    if n == 0 {
        return Ok(false);
    }
    let mut quote_count = buf.bytes().filter(|&b| b == b'"').count();
    while quote_count % 2 == 1 {
        let prev_len = buf.len();
        let more = reader.read_line(buf)?;
        if more == 0 {
            break;
        }
        quote_count += buf[prev_len..].bytes().filter(|&b| b == b'"').count();
    }
    Ok(true)
}

/// Infer Tapirus SQL `DataType`s from sample string rows.
pub fn infer_column_types(num_cols: usize, sample_rows: &[Vec<String>]) -> Vec<DataType> {
    let mut col_types = Vec::with_capacity(num_cols);
    for col_idx in 0..num_cols {
        let mut all_int = true;
        let mut all_float = true;
        let mut vector_dim: Option<usize> = None;
        let mut has_non_empty = false;

        for row in sample_rows {
            let Some(raw) = row.get(col_idx) else {
                continue;
            };
            let val = raw.trim();
            if val.is_empty() || val.eq_ignore_ascii_case("null") || val == "\\N" {
                continue;
            }
            has_non_empty = true;

            if val.starts_with('[') && val.ends_with(']') && val.len() > 2 {
                let inner = &val[1..val.len() - 1];
                let parts: Vec<&str> = inner.split(',').collect();
                if !parts.is_empty() && parts.iter().all(|p| p.trim().parse::<f32>().is_ok()) {
                    vector_dim = Some(parts.len());
                    all_int = false;
                    all_float = false;
                    continue;
                }
            }
            if all_int && val.parse::<i64>().is_err() {
                all_int = false;
            }
            if all_float && val.parse::<f64>().is_err() {
                all_float = false;
            }
        }

        if !has_non_empty {
            col_types.push(DataType::Text);
        } else if let Some(dim) = vector_dim {
            col_types.push(DataType::Vector(dim));
        } else if all_int {
            col_types.push(DataType::Integer);
        } else if all_float {
            col_types.push(DataType::Real);
        } else {
            col_types.push(DataType::Text);
        }
    }
    col_types
}

/// Parse a raw CSV text token into a typed `Value` using the target column `DataType`.
#[inline]
pub fn parse_field_with_type(raw: &str, col_type: &DataType) -> Value {
    let val = raw.trim();
    if val.is_empty() || val.eq_ignore_ascii_case("null") || val == "\\N" {
        return Value::Null;
    }
    match col_type {
        DataType::Integer => {
            if let Ok(i) = val.parse::<i64>() {
                Value::Integer(i)
            } else if val.eq_ignore_ascii_case("true") {
                Value::Integer(1)
            } else if val.eq_ignore_ascii_case("false") {
                Value::Integer(0)
            } else {
                Value::Text(raw.to_string())
            }
        }
        DataType::Real => {
            if let Ok(f) = val.parse::<f64>() {
                Value::Real(f)
            } else {
                Value::Text(raw.to_string())
            }
        }
        DataType::Vector(_) => {
            if val.starts_with('[') && val.ends_with(']') {
                let inner = &val[1..val.len() - 1];
                let mut vec = Vec::new();
                let mut ok = true;
                for part in inner.split(',') {
                    if let Ok(f) = part.trim().parse::<f32>() {
                        vec.push(f);
                    } else {
                        ok = false;
                        break;
                    }
                }
                if ok && !vec.is_empty() {
                    return Value::Vector(vec);
                }
            }
            Value::Text(raw.to_string())
        }
        DataType::Blob => Value::Blob(raw.as_bytes().to_vec()),
        DataType::Text => Value::Text(raw.to_string()),
    }
}

/// Parse a raw field heuristically when no table schema exists yet (for direct file queries).
#[inline]
pub fn parse_field_inferred(raw: &str) -> Value {
    let val = raw.trim();
    if val.is_empty() || val.eq_ignore_ascii_case("null") || val == "\\N" {
        return Value::Null;
    }
    if let Ok(i) = val.parse::<i64>() {
        return Value::Integer(i);
    }
    if let Ok(f) = val.parse::<f64>()
        && f.is_finite()
    {
        return Value::Real(f);
    }
    Value::Text(raw.to_string())
}

fn data_type_sql(dt: &DataType) -> String {
    match dt {
        DataType::Integer => "INTEGER".to_string(),
        DataType::Real => "REAL".to_string(),
        DataType::Text => "TEXT".to_string(),
        DataType::Blob => "BLOB".to_string(),
        DataType::Vector(0) => "VECTOR".to_string(),
        DataType::Vector(d) => format!("VECTOR({d})"),
    }
}

/// Stream a CSV or TSV (optionally `.gz`) file directly into a TapirusDB SQL table
/// without per-row SQL parsing overhead.
pub fn ingest_csv_fast(
    conn: &Connection,
    path: &Path,
    table_name: &str,
    delimiter_hint: Option<char>,
    batch_size: usize,
    extra_partitions: &[(String, String)],
) -> Result<IngestStats> {
    let start = Instant::now();
    let mut reader = open_buffered_reader(path)?;
    let mut line_buf = String::with_capacity(4096);

    if !read_csv_record(&mut *reader, &mut line_buf)? {
        return Err(Error::Corrupted(format!(
            "CSV file '{}' is empty or unreadable",
            path.display()
        )));
    }

    let default_delim = if path.to_string_lossy().to_ascii_lowercase().contains(".tsv") {
        '\t'
    } else {
        ','
    };
    let delim = delimiter_hint.unwrap_or_else(|| detect_delimiter(&line_buf, default_delim));

    let mut raw_headers = Vec::new();
    parse_delimited_line(&line_buf, delim, &mut raw_headers);
    if raw_headers.is_empty() {
        return Err(Error::Corrupted("CSV file has no columns in header".into()));
    }

    let mut clean_headers = sanitize_column_names(&raw_headers);
    let csv_col_count = clean_headers.len();
    for (p_col, _) in extra_partitions {
        if !clean_headers.iter().any(|c| c.eq_ignore_ascii_case(p_col)) {
            clean_headers.push(p_col.clone());
        }
    }

    let eff_batch = batch_size.max(1);
    let mut sample_rows: Vec<Vec<String>> = Vec::with_capacity(256);
    let mut pending_str_rows: Vec<Vec<String>> = Vec::with_capacity(eff_batch);

    // Ensure table exists, inferring schema from the first sample chunk if new.
    let col_types: Vec<DataType> = if let Some(existing_def) = conn.table(table_name) {
        let mut types = Vec::with_capacity(clean_headers.len());
        for h in &clean_headers {
            let dt = existing_def
                .columns
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(h))
                .map(|c| c.data_type.clone())
                .unwrap_or(DataType::Text);
            types.push(dt);
        }
        types
    } else {
        while pending_str_rows.len() < eff_batch && read_csv_record(&mut *reader, &mut line_buf)? {
            if line_buf.trim().is_empty() {
                continue;
            }
            let mut fields = Vec::with_capacity(csv_col_count);
            parse_delimited_line(&line_buf, delim, &mut fields);
            if fields.len() == csv_col_count {
                if sample_rows.len() < 256 {
                    sample_rows.push(fields.clone());
                }
                pending_str_rows.push(fields);
            }
        }
        let mut inferred = infer_column_types(csv_col_count, &sample_rows);
        for (p_col, p_val) in extra_partitions {
            if !clean_headers[..csv_col_count]
                .iter()
                .any(|c| c.eq_ignore_ascii_case(p_col))
            {
                if p_val.parse::<i64>().is_ok() {
                    inferred.push(DataType::Integer);
                } else {
                    inferred.push(DataType::Text);
                }
            }
        }
        let col_defs: Vec<String> = clean_headers
            .iter()
            .zip(inferred.iter())
            .map(|(n, dt)| format!("{n} {}", data_type_sql(dt)))
            .collect();
        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS {table_name} ({});",
            col_defs.join(", ")
        );
        conn.execute(&create_sql)?;
        inferred
    };

    let part_values: Vec<Value> = extra_partitions
        .iter()
        .enumerate()
        .filter(|(_, (p_col, _))| {
            !clean_headers[..csv_col_count]
                .iter()
                .any(|c| c.eq_ignore_ascii_case(p_col))
        })
        .map(|(idx, (_, p_val))| {
            let dt = col_types
                .get(csv_col_count + idx)
                .unwrap_or(&DataType::Text);
            parse_field_with_type(p_val, dt)
        })
        .collect();

    let mut total_rows = 0usize;
    let mut batch_values: Vec<Vec<Value>> = Vec::with_capacity(eff_batch);

    if !pending_str_rows.is_empty() {
        for str_row in pending_str_rows.drain(..) {
            let mut vals = Vec::with_capacity(clean_headers.len());
            for (idx, field) in str_row.iter().enumerate() {
                vals.push(parse_field_with_type(
                    field,
                    col_types.get(idx).unwrap_or(&DataType::Text),
                ));
            }
            vals.extend(part_values.iter().cloned());
            batch_values.push(vals);
        }
        total_rows += conn.insert_rows_batch(table_name, Some(&clean_headers), &batch_values)?;
        batch_values.clear();
    }

    let mut parsed_fields = Vec::with_capacity(csv_col_count);
    while read_csv_record(&mut *reader, &mut line_buf)? {
        if line_buf.trim().is_empty() {
            continue;
        }
        parse_delimited_line(&line_buf, delim, &mut parsed_fields);
        if parsed_fields.len() != csv_col_count {
            continue;
        }
        let mut vals = Vec::with_capacity(clean_headers.len());
        for (idx, field) in parsed_fields.iter().enumerate() {
            vals.push(parse_field_with_type(
                field,
                col_types.get(idx).unwrap_or(&DataType::Text),
            ));
        }
        vals.extend(part_values.iter().cloned());
        batch_values.push(vals);

        if batch_values.len() >= eff_batch {
            total_rows +=
                conn.insert_rows_batch(table_name, Some(&clean_headers), &batch_values)?;
            batch_values.clear();
        }
    }

    if !batch_values.is_empty() {
        total_rows += conn.insert_rows_batch(table_name, Some(&clean_headers), &batch_values)?;
    }

    Ok(IngestStats {
        rows_ingested: total_rows,
        files_processed: 1,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        target: table_name.to_string(),
        columns: clean_headers.len(),
    })
}

/// Read rows from a CSV/TSV file (optionally `.gz`) directly into in-memory `Row`s for external queries.
pub fn scan_csv_rows(
    path: &Path,
    delimiter_hint: Option<char>,
    extra_partitions: &[(String, String)],
    limit: Option<usize>,
) -> Result<Vec<Row>> {
    let mut reader = open_buffered_reader(path)?;
    let mut line_buf = String::with_capacity(4096);
    if !read_csv_record(&mut *reader, &mut line_buf)? {
        return Ok(Vec::new());
    }

    let default_delim = if path.to_string_lossy().to_ascii_lowercase().contains(".tsv") {
        '\t'
    } else {
        ','
    };
    let delim = delimiter_hint.unwrap_or_else(|| detect_delimiter(&line_buf, default_delim));

    let mut raw_headers = Vec::new();
    parse_delimited_line(&line_buf, delim, &mut raw_headers);
    if raw_headers.is_empty() {
        return Ok(Vec::new());
    }
    let mut headers = sanitize_column_names(&raw_headers);
    let csv_col_count = headers.len();
    let mut extra_vals = Vec::new();
    for (p_col, p_val) in extra_partitions {
        if !headers.iter().any(|c| c.eq_ignore_ascii_case(p_col)) {
            headers.push(p_col.clone());
            extra_vals.push(parse_field_inferred(p_val));
        }
    }

    let mut rows = Vec::new();
    let mut fields = Vec::with_capacity(csv_col_count);
    while read_csv_record(&mut *reader, &mut line_buf)? {
        if let Some(lim) = limit
            && rows.len() >= lim
        {
            break;
        }
        if line_buf.trim().is_empty() {
            continue;
        }
        parse_delimited_line(&line_buf, delim, &mut fields);
        if fields.len() != csv_col_count {
            continue;
        }
        let mut vals: Vec<Value> = fields.iter().map(|f| parse_field_inferred(f)).collect();
        vals.extend(extra_vals.iter().cloned());
        rows.push(Row::new(headers.clone(), vals));
    }
    Ok(rows)
}

#[inline]
fn json_to_tapir_value(v: &JsonValue) -> Value {
    match v {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Integer(if *b { 1 } else { 0 }),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                Value::Real(f)
            } else {
                Value::Text(n.to_string())
            }
        }
        JsonValue::String(s) => Value::Text(s.clone()),
        JsonValue::Array(arr) if !arr.is_empty() && arr.iter().all(|x| x.as_f64().is_some()) => {
            Value::Vector(
                arr.iter()
                    .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                    .collect(),
            )
        }
        other => Value::Text(other.to_string()),
    }
}

/// Convert JSON or JSONL objects directly into a relational TapirusDB table.
pub fn ingest_jsonl_as_table(
    conn: &Connection,
    path: &Path,
    table_name: &str,
    batch_size: usize,
    extra_partitions: &[(String, String)],
) -> Result<IngestStats> {
    let start = Instant::now();
    let objects = read_json_objects(path)?;
    if objects.is_empty() {
        return Ok(IngestStats {
            rows_ingested: 0,
            files_processed: 1,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            target: table_name.to_string(),
            columns: 0,
        });
    }

    let mut key_set = BTreeSet::new();
    for obj in objects.iter().take(256) {
        if let Some(map) = obj.as_object() {
            for k in map.keys() {
                key_set.insert(k.clone());
            }
        }
    }
    let raw_keys: Vec<String> = key_set.into_iter().collect();
    if raw_keys.is_empty() {
        return Err(Error::Corrupted(
            "JSON/JSONL contains no object properties to map into table columns".into(),
        ));
    }

    let mut clean_cols = sanitize_column_names(&raw_keys);
    for (p_col, _) in extra_partitions {
        if !clean_cols.iter().any(|c| c.eq_ignore_ascii_case(p_col)) {
            clean_cols.push(p_col.clone());
        }
    }

    if conn.table(table_name).is_none() {
        let mut col_defs = Vec::with_capacity(clean_cols.len());
        for (i, raw_key) in raw_keys.iter().enumerate() {
            let mut has_int = false;
            let mut has_real = false;
            let mut has_vec: Option<usize> = None;
            let mut has_text = false;
            for obj in objects.iter().take(256) {
                if let Some(v) = obj.as_object().and_then(|m| m.get(raw_key)) {
                    match json_to_tapir_value(v) {
                        Value::Null => {}
                        Value::Integer(_) => has_int = true,
                        Value::Real(_) => has_real = true,
                        Value::Vector(vec) => has_vec = Some(vec.len()),
                        _ => has_text = true,
                    }
                }
            }
            let dt = if has_text {
                "TEXT".to_string()
            } else if let Some(d) = has_vec {
                format!("VECTOR({d})")
            } else if has_real {
                "REAL".to_string()
            } else if has_int {
                "INTEGER".to_string()
            } else {
                "TEXT".to_string()
            };
            col_defs.push(format!("{} {dt}", clean_cols[i]));
        }
        for col in clean_cols.iter().skip(raw_keys.len()) {
            col_defs.push(format!("{col} TEXT"));
        }
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {table_name} ({});",
            col_defs.join(", ")
        );
        conn.execute(&sql)?;
    }

    let extra_vals: Vec<Value> = extra_partitions
        .iter()
        .filter(|(p_col, _)| !raw_keys.iter().any(|k| k.eq_ignore_ascii_case(p_col)))
        .map(|(_, v)| parse_field_inferred(v))
        .collect();

    let eff_batch = batch_size.max(1);
    let mut total = 0usize;
    for chunk in objects.chunks(eff_batch) {
        let mut rows_vals = Vec::with_capacity(chunk.len());
        for obj in chunk {
            let mut vals = Vec::with_capacity(clean_cols.len());
            let map = obj.as_object();
            for raw_k in &raw_keys {
                if let Some(v) = map.and_then(|m| m.get(raw_k)) {
                    vals.push(json_to_tapir_value(v));
                } else {
                    vals.push(Value::Null);
                }
            }
            vals.extend(extra_vals.iter().cloned());
            rows_vals.push(vals);
        }
        total += conn.insert_rows_batch(table_name, Some(&clean_cols), &rows_vals)?;
    }

    Ok(IngestStats {
        rows_ingested: total,
        files_processed: 1,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        target: table_name.to_string(),
        columns: clean_cols.len(),
    })
}

/// Read JSON / JSONL / `.jsonl.gz` into `Vec<Row>` for direct `read_jsonl(...)` SQL scans.
pub fn scan_jsonl_rows(
    path: &Path,
    extra_partitions: &[(String, String)],
    limit: Option<usize>,
) -> Result<Vec<Row>> {
    let objects = read_json_objects(path)?;
    if objects.is_empty() {
        return Ok(Vec::new());
    }
    let mut key_set = BTreeSet::new();
    for obj in objects.iter().take(256) {
        if let Some(map) = obj.as_object() {
            for k in map.keys() {
                key_set.insert(k.clone());
            }
        }
    }
    let raw_keys: Vec<String> = key_set.into_iter().collect();
    let mut clean_cols = sanitize_column_names(&raw_keys);
    let mut extra_vals = Vec::new();
    for (p_col, p_val) in extra_partitions {
        if !clean_cols.iter().any(|c| c.eq_ignore_ascii_case(p_col)) {
            clean_cols.push(p_col.clone());
            extra_vals.push(parse_field_inferred(p_val));
        }
    }

    let max_rows = limit.unwrap_or(objects.len());
    let mut rows = Vec::with_capacity(objects.len().min(max_rows));
    for obj in objects.into_iter().take(max_rows) {
        let map = obj.as_object();
        let mut vals = Vec::with_capacity(clean_cols.len());
        for raw_k in &raw_keys {
            if let Some(v) = map.and_then(|m| m.get(raw_k)) {
                vals.push(json_to_tapir_value(v));
            } else {
                vals.push(Value::Null);
            }
        }
        vals.extend(extra_vals.iter().cloned());
        rows.push(Row::new(clean_cols.clone(), vals));
    }
    Ok(rows)
}

fn read_json_objects(path: &Path) -> Result<Vec<JsonValue>> {
    let mut reader = open_buffered_reader(path)?;
    let mut content = String::new();
    reader.read_to_string(&mut content)?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[')
        && let Ok(JsonValue::Array(arr)) = serde_json::from_str::<JsonValue>(trimmed)
    {
        return Ok(arr);
    }
    let mut out = Vec::new();
    for line in trimmed.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        let val: JsonValue = serde_json::from_str(l)
            .map_err(|e| Error::Corrupted(format!("Invalid JSONL record: {e}")))?;
        out.push(val);
    }
    Ok(out)
}

/// Export rows to an RFC-4180 CSV (or TSV) file.
pub fn write_rows_to_csv(rows: &[Row], dest: &Path, delimiter: char) -> Result<ExportStats> {
    let start = Instant::now();
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(dest)?;
    let mut writer = BufWriter::with_capacity(128 * 1024, file);

    if let Some(first) = rows.first() {
        let cols = first.columns();
        for (i, c) in cols.iter().enumerate() {
            if i > 0 {
                write!(writer, "{delimiter}")?;
            }
            write_csv_escaped(&mut writer, c, delimiter)?;
        }
        writeln!(writer)?;

        for row in rows {
            for (i, val) in row.values().iter().enumerate() {
                if i > 0 {
                    write!(writer, "{delimiter}")?;
                }
                match val {
                    Value::Null => {}
                    Value::Integer(n) => write!(writer, "{n}")?,
                    Value::Real(f) => write!(writer, "{f}")?,
                    Value::Text(s) => write_csv_escaped(&mut writer, s, delimiter)?,
                    Value::Blob(b) => write!(writer, "0x{}", hex_bytes(b))?,
                    Value::Vector(v) => {
                        let s: Vec<String> = v.iter().map(|x| x.to_string()).collect();
                        let joined = format!("[{}]", s.join(","));
                        write_csv_escaped(&mut writer, &joined, delimiter)?;
                    }
                }
            }
            writeln!(writer)?;
        }
    }
    writer.flush()?;

    Ok(ExportStats {
        rows_exported: rows.len(),
        files_written: 1,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

fn write_csv_escaped<W: Write>(w: &mut W, s: &str, delimiter: char) -> std::io::Result<()> {
    if s.contains(delimiter) || s.contains('"') || s.contains('\n') || s.contains('\r') {
        w.write_all(b"\"")?;
        for b in s.as_bytes() {
            if *b == b'"' {
                w.write_all(b"\"\"")?;
            } else {
                w.write_all(&[*b])?;
            }
        }
        w.write_all(b"\"")
    } else {
        w.write_all(s.as_bytes())
    }
}

/// Export rows to a JSONL (newline-delimited JSON) file.
pub fn write_rows_to_jsonl(rows: &[Row], dest: &Path) -> Result<ExportStats> {
    let start = Instant::now();
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(dest)?;
    let mut writer = BufWriter::with_capacity(128 * 1024, file);

    for row in rows {
        let mut map = serde_json::Map::with_capacity(row.columns().len());
        for (col, val) in row.columns().iter().zip(row.values().iter()) {
            let jv = match val {
                Value::Null => JsonValue::Null,
                Value::Integer(n) => JsonValue::from(*n),
                Value::Real(f) => serde_json::Number::from_f64(*f)
                    .map(JsonValue::Number)
                    .unwrap_or(JsonValue::Null),
                Value::Text(s) => JsonValue::String(s.clone()),
                Value::Blob(b) => JsonValue::String(format!("0x{}", hex_bytes(b))),
                Value::Vector(v) => {
                    JsonValue::Array(v.iter().map(|x| JsonValue::from(*x as f64)).collect())
                }
            };
            map.insert(col.clone(), jv);
        }
        serde_json::to_writer(&mut writer, &JsonValue::Object(map))
            .map_err(|e| Error::Corrupted(format!("JSONL serialize error: {e}")))?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;

    Ok(ExportStats {
        rows_exported: rows.len(),
        files_written: 1,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}
