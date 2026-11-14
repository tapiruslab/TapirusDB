//! Universal SQL Dump importer and exporter (PostgreSQL, MySQL, SQLite, DuckDB `.sql`).
//!
//! Parses standard `CREATE TABLE`, multi-row `INSERT INTO ... VALUES (...), (...)`,
//! and PostgreSQL `COPY ... FROM stdin; ... \.` blocks, converting rows directly into
//! TapirusDB batch inserts without invoking full SQL parser overhead per tuple.
//!
//! Complies strictly with `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

use crate::Connection;
use crate::error::Result;
use crate::lakehouse::csv_stream::{ExportStats, IngestStats, parse_field_with_type};
use crate::lakehouse::sqlite_reader::map_sqlite_type_to_tapir;
use crate::sql::catalog::DataType;
use crate::traits::{Row, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::Instant;

type TableBatch = (Option<Vec<String>>, Vec<Vec<Value>>);
type BatchMap = HashMap<String, TableBatch>;
type ParsedInsert = (String, Option<Vec<String>>, Vec<Vec<Value>>);
type ParsedDdlCol = (String, String, bool);
type ParsedDdl = (String, Vec<ParsedDdlCol>);

/// Stream a `.sql` dump file (PostgreSQL `pg_dump`, MySQL `mysqldump`, SQLite `.dump`, DuckDB)
/// into a TapirusDB `Connection`.
pub fn import_sql_dump(
    conn: &Connection,
    sql_path: &Path,
    target_table_override: Option<&str>,
    batch_size: usize,
) -> Result<IngestStats> {
    let start = Instant::now();
    let file = File::open(sql_path)?;
    let reader = BufReader::with_capacity(256 * 1024, file);
    let eff_batch = batch_size.max(1);

    let mut total_rows = 0usize;
    let mut last_target = target_table_override.unwrap_or("imported").to_string();
    let mut last_cols = 0usize;

    // Buffered batches keyed by table name so we flush efficiently.
    let mut batches: BatchMap = HashMap::new();

    // Active PostgreSQL COPY ... FROM stdin state
    let mut active_copy: Option<(String, Option<Vec<String>>, Vec<DataType>)> = None;
    let mut stmt_buf = String::with_capacity(4096);
    let mut in_block_comment = false;

    for line_res in reader.lines() {
        let line = line_res?;

        if let Some((ref copy_table, ref copy_cols, ref col_types)) = active_copy {
            let trimmed_end = line.trim_end_matches(&['\r', '\n'][..]);
            if trimmed_end == "\\." {
                flush_table_batch(conn, copy_table, &mut batches, &mut total_rows)?;
                active_copy = None;
                continue;
            }
            let raw_fields: Vec<&str> = trimmed_end.split('\t').collect();
            let mut vals = Vec::with_capacity(raw_fields.len());
            for (i, raw) in raw_fields.iter().enumerate() {
                if let Some(text) = unescape_pg_copy_field(raw).filter(|s| s != "\\N") {
                    let dt = col_types.get(i).unwrap_or(&DataType::Text);
                    vals.push(parse_field_with_type(&text, dt));
                } else {
                    vals.push(Value::Null);
                }
            }
            last_target = copy_table.clone();
            last_cols = vals.len();
            let entry = batches
                .entry(copy_table.clone())
                .or_insert_with(|| (copy_cols.clone(), Vec::with_capacity(eff_batch)));
            entry.1.push(vals);
            if entry.1.len() >= eff_batch {
                flush_table_batch(conn, copy_table, &mut batches, &mut total_rows)?;
            }
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with('#') {
            continue;
        }
        if in_block_comment {
            if let Some(end_idx) = trimmed.find("*/") {
                in_block_comment = false;
                let rest = trimmed[end_idx + 2..].trim();
                if rest.is_empty() {
                    continue;
                }
                stmt_buf.push_str(rest);
                stmt_buf.push(' ');
            }
            continue;
        }
        if trimmed.starts_with("/*") {
            if !trimmed.contains("*/") {
                in_block_comment = true;
            }
            continue;
        }

        stmt_buf.push_str(trimmed);
        stmt_buf.push(' ');

        if trimmed.ends_with(';') {
            let stmt = std::mem::take(&mut stmt_buf);
            let clean = stmt.trim().trim_end_matches(';').trim();
            if let Some((copy_tbl, copy_cols)) = try_parse_pg_copy_stdin(clean) {
                let target_tbl = target_table_override
                    .map(str::to_string)
                    .unwrap_or(copy_tbl);
                let col_types = resolve_table_col_types(conn, &target_tbl, copy_cols.as_deref());
                active_copy = Some((target_tbl, copy_cols, col_types));
                continue;
            }

            if let Some((tbl, cols, rows)) =
                process_sql_dump_statement(conn, clean, target_table_override)?
            {
                last_target = tbl.clone();
                if let Some(first) = rows.first() {
                    last_cols = first.len();
                }
                let entry = batches
                    .entry(tbl.clone())
                    .or_insert_with(|| (cols, Vec::with_capacity(eff_batch)));
                entry.1.extend(rows);
                if entry.1.len() >= eff_batch {
                    flush_table_batch(conn, &tbl, &mut batches, &mut total_rows)?;
                }
            }
        }
    }

    let remaining_tables: Vec<String> = batches.keys().cloned().collect();
    for tbl in remaining_tables {
        flush_table_batch(conn, &tbl, &mut batches, &mut total_rows)?;
    }

    Ok(IngestStats {
        rows_ingested: total_rows,
        files_processed: 1,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        target: last_target,
        columns: last_cols,
    })
}

fn flush_table_batch(
    conn: &Connection,
    table: &str,
    batches: &mut BatchMap,
    total_rows: &mut usize,
) -> Result<()> {
    if let Some((cols, mut rows)) = batches.remove(table)
        && !rows.is_empty()
    {
        *total_rows += conn.insert_rows_batch(table, cols.as_deref(), &rows)?;
        rows.clear();
    }
    Ok(())
}

fn resolve_table_col_types(
    conn: &Connection,
    table: &str,
    cols: Option<&[String]>,
) -> Vec<DataType> {
    let Some(def) = conn.table(table) else {
        return Vec::new();
    };
    if let Some(c_list) = cols {
        c_list
            .iter()
            .map(|c| {
                def.columns
                    .iter()
                    .find(|td| td.name.eq_ignore_ascii_case(c))
                    .map(|td| td.data_type.clone())
                    .unwrap_or(DataType::Text)
            })
            .collect()
    } else {
        def.columns.iter().map(|c| c.data_type.clone()).collect()
    }
}

fn try_parse_pg_copy_stdin(stmt: &str) -> Option<(String, Option<Vec<String>>)> {
    let upper = stmt.to_ascii_uppercase();
    if !upper.starts_with("COPY ") || !upper.contains("FROM STDIN") {
        return None;
    }
    let after_copy = stmt[5..].trim();
    let from_pos = after_copy.to_ascii_uppercase().find(" FROM ")?;
    let target_part = after_copy[..from_pos].trim();
    if let Some(open_p) = target_part.find('(')
        && let Some(close_p) = target_part.rfind(')')
    {
        let raw_tbl = target_part[..open_p].trim();
        let cols: Vec<String> = target_part[open_p + 1..close_p]
            .split(',')
            .map(normalize_ident)
            .collect();
        Some((normalize_table_name(raw_tbl), Some(cols)))
    } else {
        Some((normalize_table_name(target_part), None))
    }
}

fn unescape_pg_copy_field(raw: &str) -> Option<String> {
    if raw == "\\N" {
        return None;
    }
    if !raw.contains('\\') {
        return Some(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('N') if out.is_empty() && raw.len() == 2 => return None,
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn process_sql_dump_statement(
    conn: &Connection,
    stmt: &str,
    target_table_override: Option<&str>,
) -> Result<Option<ParsedInsert>> {
    let upper = stmt.to_ascii_uppercase();

    // Skip engine-specific pragmas/directives common in pg_dump, mysqldump, sqlite dumps.
    if upper.starts_with("SET ")
        || upper.starts_with("PRAGMA ")
        || upper.starts_with("BEGIN")
        || upper.starts_with("COMMIT")
        || upper.starts_with("LOCK ")
        || upper.starts_with("UNLOCK ")
        || upper.starts_with("SELECT PG_CATALOG")
        || upper.starts_with("ALTER ")
        || upper.starts_with("DROP ")
        || upper.starts_with("COMMENT ")
    {
        return Ok(None);
    }

    if upper.starts_with("CREATE TABLE") {
        let Some((tbl_name, col_defs)) = parse_create_table_ddl(stmt) else {
            return Ok(None);
        };
        let target_tbl = target_table_override.unwrap_or(&tbl_name);
        if !col_defs.is_empty() {
            let defs_sql: Vec<String> = col_defs
                .iter()
                .map(|(name, ty, pk)| {
                    if *pk {
                        format!("{name} {ty} PRIMARY KEY")
                    } else {
                        format!("{name} {ty}")
                    }
                })
                .collect();
            let sql = format!(
                "CREATE TABLE IF NOT EXISTS {target_tbl} ({});",
                defs_sql.join(", ")
            );
            let _ = conn.execute(&sql);
        }
        return Ok(None);
    }

    if upper.starts_with("INSERT INTO") || upper.starts_with("INSERT OR ") {
        if let Some((tbl_name, cols, rows)) = parse_insert_values_dump(stmt) {
            let target_tbl = target_table_override.unwrap_or(&tbl_name).to_string();
            return Ok(Some((target_tbl, cols, rows)));
        }
        return Ok(None);
    }

    Ok(None)
}

fn normalize_ident(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
        .to_string()
}

fn normalize_table_name(raw: &str) -> String {
    let cleaned = raw.trim();
    // Strip schema prefixes like public.users -> users
    let without_schema = cleaned.rsplit('.').next().unwrap_or(cleaned);
    normalize_ident(without_schema)
}

fn parse_create_table_ddl(stmt: &str) -> Option<ParsedDdl> {
    let open = stmt.find('(')?;
    let close = stmt.rfind(')')?;
    if close <= open {
        return None;
    }
    let header = &stmt[..open];
    let raw_tbl = header.split_whitespace().last().map(normalize_table_name)?;

    let body = &stmt[open + 1..close];
    let parts = split_top_level_commas(body);
    let mut cols = Vec::new();
    for item in parts {
        let trimmed = item.trim();
        let up = trimmed.to_ascii_uppercase();
        if up.starts_with("PRIMARY KEY")
            || up.starts_with("FOREIGN KEY")
            || up.starts_with("CONSTRAINT ")
            || up.starts_with("UNIQUE")
            || up.starts_with("KEY ")
            || up.starts_with("INDEX ")
            || up.starts_with("CHECK")
        {
            continue;
        }
        let mut tokens = trimmed.split_whitespace();
        let col_name = normalize_ident(tokens.next()?);
        let raw_type = tokens.next().unwrap_or("TEXT");
        let mapped = map_sqlite_type_to_tapir(raw_type).to_string();
        let is_pk = up.contains("PRIMARY KEY");
        cols.push((col_name, mapped, is_pk));
    }
    Some((raw_tbl, cols))
}

fn parse_insert_values_dump(stmt: &str) -> Option<ParsedInsert> {
    let up = stmt.to_ascii_uppercase();
    let into_pos = up.find("INTO ")? + 5;
    let rest = &stmt[into_pos..];
    let rest_up = &up[into_pos..];
    let val_pos = rest_up.find("VALUES")?;

    let target_spec = rest[..val_pos].trim();
    let (table_name, cols) = if let Some(open) = target_spec.find('(')
        && let Some(close) = target_spec.rfind(')')
    {
        let tbl = normalize_table_name(&target_spec[..open]);
        let col_list = split_top_level_commas(&target_spec[open + 1..close])
            .into_iter()
            .map(|s| normalize_ident(&s))
            .collect();
        (tbl, Some(col_list))
    } else {
        (normalize_table_name(target_spec), None)
    };

    let values_tail = rest[val_pos + 6..].trim();
    let tuples = parse_sql_value_tuples(values_tail);
    if tuples.is_empty() {
        return None;
    }
    Some((table_name, cols, tuples))
}

fn split_top_level_commas(input: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if in_single || in_double => {
                cur.push(c);
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            '\'' if !in_double => {
                cur.push(c);
                if in_single && chars.peek() == Some(&'\'') {
                    cur.push(chars.next().unwrap());
                } else {
                    in_single = !in_single;
                }
            }
            '"' if !in_single => {
                in_double = !in_double;
                cur.push(c);
            }
            '(' if !in_single && !in_double => {
                depth += 1;
                cur.push(c);
            }
            ')' if !in_single && !in_double => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            ',' if depth == 0 && !in_single && !in_double => {
                items.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        items.push(cur);
    }
    items
}

fn parse_sql_value_tuples(tail: &str) -> Vec<Vec<Value>> {
    let mut rows = Vec::new();
    let mut i = 0usize;
    let bytes = tail.as_bytes();

    while i < bytes.len() {
        if bytes[i] == b'(' {
            let start = i + 1;
            let mut j = start;
            let mut depth = 1usize;
            let mut in_single = false;
            while j < tail.len() {
                let slice = &tail[j..];
                let ch = slice.chars().next().unwrap();
                let ch_len = ch.len_utf8();
                if in_single {
                    if ch == '\\' {
                        j += ch_len;
                        if j < tail.len() {
                            j += tail[j..].chars().next().unwrap().len_utf8();
                        }
                        continue;
                    }
                    if ch == '\'' {
                        if tail[j + 1..].starts_with('\'') {
                            j += 2;
                            continue;
                        }
                        in_single = false;
                    }
                } else if ch == '\'' {
                    in_single = true;
                } else if ch == '(' {
                    depth += 1;
                } else if ch == ')' {
                    depth -= 1;
                    if depth == 0 {
                        let inner = &tail[start..j];
                        let parts = split_top_level_commas(inner);
                        let row_vals = parts
                            .into_iter()
                            .map(|p| parse_sql_literal_token(&p))
                            .collect();
                        rows.push(row_vals);
                        i = j + ch_len;
                        break;
                    }
                }
                j += ch_len;
            }
            if depth != 0 {
                break;
            }
        } else {
            i += 1;
        }
    }

    rows
}

fn parse_sql_literal_token(raw: &str) -> Value {
    let t = raw.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("NULL") {
        return Value::Null;
    }
    if t.eq_ignore_ascii_case("TRUE") {
        return Value::Integer(1);
    }
    if t.eq_ignore_ascii_case("FALSE") {
        return Value::Integer(0);
    }
    if t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2 {
        let inner = &t[1..t.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\'' && chars.peek() == Some(&'\'') {
                chars.next();
                out.push('\'');
            } else if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('\\') => out.push('\\'),
                    Some('\'') => out.push('\''),
                    Some(other) => out.push(other),
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        return Value::Text(out);
    }
    if let Ok(i) = t.parse::<i64>() {
        return Value::Integer(i);
    }
    if let Ok(f) = t.parse::<f64>()
        && f.is_finite()
    {
        return Value::Real(f);
    }
    Value::Text(t.to_string())
}

/// Export rows from a table or query into an ANSI `.sql` dump compatible with
/// SQLite, PostgreSQL, MySQL, and DuckDB.
pub fn write_rows_to_sql_dump(table_name: &str, rows: &[Row], dest: &Path) -> Result<ExportStats> {
    let start = Instant::now();
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(dest)?;
    let mut writer = BufWriter::with_capacity(128 * 1024, file);

    writeln!(writer, "-- TapirusDB Universal SQL Dump")?;
    writeln!(writer, "BEGIN TRANSACTION;")?;

    if let Some(first) = rows.first() {
        let cols = first.columns();
        let mut col_defs = Vec::with_capacity(cols.len());
        for (idx, col_name) in cols.iter().enumerate() {
            let inferred = rows
                .iter()
                .take(100)
                .filter_map(|r| r.values().get(idx))
                .find(|v| !v.is_null())
                .map(|v| match v {
                    Value::Integer(_) => "INTEGER",
                    Value::Real(_) => "REAL",
                    Value::Blob(_) => "BLOB",
                    _ => "TEXT",
                })
                .unwrap_or("TEXT");
            col_defs.push(format!("{col_name} {inferred}"));
        }
        writeln!(
            writer,
            "CREATE TABLE IF NOT EXISTS {table_name} ({});",
            col_defs.join(", ")
        )?;

        let col_list = cols.join(", ");
        for chunk in rows.chunks(250) {
            write!(writer, "INSERT INTO {table_name} ({col_list}) VALUES ")?;
            for (r_idx, row) in chunk.iter().enumerate() {
                if r_idx > 0 {
                    write!(writer, ", ")?;
                }
                write!(writer, "(")?;
                for (c_idx, val) in row.values().iter().enumerate() {
                    if c_idx > 0 {
                        write!(writer, ", ")?;
                    }
                    match val {
                        Value::Null => write!(writer, "NULL")?,
                        Value::Integer(n) => write!(writer, "{n}")?,
                        Value::Real(f) => {
                            if f.is_finite() {
                                write!(writer, "{f}")?;
                            } else {
                                write!(writer, "NULL")?;
                            }
                        }
                        Value::Text(s) => {
                            let escaped = s.replace('\'', "''");
                            write!(writer, "'{escaped}'")?;
                        }
                        Value::Blob(b) => {
                            write!(writer, "X'")?;
                            for byte in b {
                                write!(writer, "{byte:02X}")?;
                            }
                            write!(writer, "'")?;
                        }
                        Value::Vector(v) => {
                            let parts: Vec<String> = v.iter().map(|x| x.to_string()).collect();
                            write!(writer, "'[{}]'", parts.join(","))?;
                        }
                    }
                }
                write!(writer, ")")?;
            }
            writeln!(writer, ";")?;
        }
    }

    writeln!(writer, "COMMIT;")?;
    writer.flush()?;

    Ok(ExportStats {
        rows_exported: rows.len(),
        files_written: 1,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}
