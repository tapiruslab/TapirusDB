//! Pure-Rust Excel & OpenDocument spreadsheet reader and writer (`.xlsx`, `.xls`, `.xlsb`, `.xlsm`, `.ods`).
//!
//! Uses `calamine` for multi-format spreadsheet reading and `rust_xlsxwriter` for native
//! Excel `.xlsx` exports under the `lakehouse` feature, strictly complying with
//! `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

use crate::Connection;
use crate::error::{Error, Result};
use crate::lakehouse::csv_stream::{ExportStats, IngestStats};
use crate::lakehouse::sanitize_table_stem;
use crate::sql::catalog::DataType;
use crate::traits::{Row, Value};
use calamine::{Data, Reader, open_workbook_auto};
use rust_xlsxwriter::{Format, Workbook};
use std::path::Path;
use std::time::Instant;

fn cell_to_display_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => format!("{dt}"),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#{e:?}"),
    }
}

fn try_parse_vector_str(s: &str) -> Option<Vec<f32>> {
    let t = s.trim();
    if !(t.starts_with('[') && t.ends_with(']')) {
        return None;
    }
    let inner = &t[1..t.len() - 1];
    let mut vals = Vec::new();
    for item in inner.split(',') {
        let p = item.trim();
        if p.is_empty() {
            return None;
        }
        vals.push(p.parse::<f32>().ok()?);
    }
    if vals.is_empty() { None } else { Some(vals) }
}

fn infer_excel_column_types(rows: &[&[Data]], width: usize) -> Vec<DataType> {
    let mut types = Vec::with_capacity(width);
    for col in 0..width {
        let mut saw_non_empty = false;
        let mut all_int = true;
        let mut all_num = true;
        let mut vector_dim: Option<usize> = None;

        for row in rows.iter().take(256) {
            let cell = row.get(col).unwrap_or(&Data::Empty);
            match cell {
                Data::Empty => continue,
                Data::Int(_) => {
                    saw_non_empty = true;
                }
                Data::Bool(_) => {
                    saw_non_empty = true;
                }
                Data::Float(f) => {
                    saw_non_empty = true;
                    if f.fract() != 0.0 || *f < (i64::MIN as f64) || *f > (i64::MAX as f64) {
                        all_int = false;
                    }
                }
                Data::String(s) => {
                    let t = s.trim();
                    if t.is_empty() || t.eq_ignore_ascii_case("null") {
                        continue;
                    }
                    saw_non_empty = true;
                    if let Some(vec) = try_parse_vector_str(t) {
                        vector_dim = Some(vec.len());
                        all_int = false;
                        all_num = false;
                    } else {
                        all_int = false;
                        all_num = false;
                    }
                }
                _ => {
                    saw_non_empty = true;
                    all_int = false;
                    all_num = false;
                }
            }
        }

        if !saw_non_empty {
            types.push(DataType::Text);
        } else if let Some(dim) = vector_dim {
            types.push(DataType::Vector(dim));
        } else if all_int {
            types.push(DataType::Integer);
        } else if all_num {
            types.push(DataType::Real);
        } else {
            types.push(DataType::Text);
        }
    }
    types
}

fn cell_to_value(cell: &Data, expected: &DataType) -> Value {
    match cell {
        Data::Empty => Value::Null,
        Data::Int(i) => match expected {
            DataType::Real => Value::Real(*i as f64),
            DataType::Text => Value::Text(i.to_string()),
            _ => Value::Integer(*i),
        },
        Data::Float(f) => match expected {
            DataType::Integer => Value::Integer(*f as i64),
            DataType::Text => Value::Text(f.to_string()),
            _ => Value::Real(*f),
        },
        Data::Bool(b) => match expected {
            DataType::Text => Value::Text(b.to_string()),
            _ => Value::Integer(i64::from(*b)),
        },
        Data::String(s) => {
            let t = s.trim();
            if t.is_empty() || t.eq_ignore_ascii_case("null") {
                return Value::Null;
            }
            match expected {
                DataType::Vector(_) => try_parse_vector_str(t)
                    .map(Value::Vector)
                    .unwrap_or_else(|| Value::Text(s.clone())),
                DataType::Integer => t
                    .parse::<i64>()
                    .map(Value::Integer)
                    .unwrap_or_else(|_| Value::Text(s.clone())),
                DataType::Real => t
                    .parse::<f64>()
                    .map(Value::Real)
                    .unwrap_or_else(|_| Value::Text(s.clone())),
                _ => Value::Text(s.clone()),
            }
        }
        other => Value::Text(cell_to_display_string(other)),
    }
}

fn excel_data_type_sql(dt: &DataType) -> String {
    match dt {
        DataType::Integer => "INTEGER".to_string(),
        DataType::Real => "REAL".to_string(),
        DataType::Text => "TEXT".to_string(),
        DataType::Blob => "BLOB".to_string(),
        DataType::Vector(0) => "VECTOR".to_string(),
        DataType::Vector(d) => format!("VECTOR({d})"),
    }
}

fn build_clean_headers(header_cells: &[Data]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(header_cells.len());
    for (idx, cell) in header_cells.iter().enumerate() {
        let raw = cell_to_display_string(cell);
        let mut clean = if raw.trim().is_empty() {
            format!("col_{}", idx + 1)
        } else {
            sanitize_table_stem(raw.trim())
        };
        while !seen.insert(clean.to_ascii_lowercase()) {
            clean.push('_');
        }
        out.push(clean);
    }
    out
}

/// Scan rows directly from an Excel / OpenDocument workbook (`.xlsx`, `.xls`, `.xlsb`, `.xlsm`, `.ods`).
/// Defaults to the first worksheet unless `sheet_name` is provided.
pub fn scan_excel_rows(
    path: &Path,
    sheet_name: Option<&str>,
    extra_partitions: &[(String, String)],
    limit: Option<usize>,
) -> Result<Vec<Row>> {
    let mut workbook = open_workbook_auto(path)
        .map_err(|e| Error::Corrupted(format!("Excel open error for '{}': {e}", path.display())))?;
    let sheets = workbook.sheet_names();
    if sheets.is_empty() {
        return Ok(Vec::new());
    }
    let chosen_sheet = sheet_name
        .and_then(|wanted| {
            sheets
                .iter()
                .find(|s| s.eq_ignore_ascii_case(wanted))
                .cloned()
        })
        .unwrap_or_else(|| sheets[0].clone());

    let range = workbook
        .worksheet_range(&chosen_sheet)
        .map_err(|e| Error::Corrupted(format!("Excel sheet '{chosen_sheet}' error: {e}")))?;
    let mut raw_rows: Vec<&[Data]> = range.rows().collect();
    if raw_rows.is_empty() {
        return Ok(Vec::new());
    }

    let header_cells = raw_rows.remove(0);
    let mut cols = build_clean_headers(header_cells);
    let base_width = cols.len();
    let col_types = infer_excel_column_types(&raw_rows, base_width);

    let mut extra_vals = Vec::with_capacity(extra_partitions.len());
    for (k, v) in extra_partitions {
        if !cols.iter().any(|c| c.eq_ignore_ascii_case(k)) {
            cols.push(k.clone());
            if let Ok(i) = v.parse::<i64>() {
                extra_vals.push(Value::Integer(i));
            } else if let Ok(f) = v.parse::<f64>() {
                extra_vals.push(Value::Real(f));
            } else {
                extra_vals.push(Value::Text(v.clone()));
            }
        }
    }

    let take_n = limit.unwrap_or(raw_rows.len());
    let mut rows = Vec::with_capacity(raw_rows.len().min(take_n));
    for r in raw_rows.into_iter().take(take_n) {
        if r.iter().all(|c| matches!(c, Data::Empty)) {
            continue;
        }
        let mut vals = Vec::with_capacity(cols.len());
        for (idx, dt) in col_types.iter().enumerate() {
            let cell = r.get(idx).unwrap_or(&Data::Empty);
            vals.push(cell_to_value(cell, dt));
        }
        vals.extend(extra_vals.iter().cloned());
        rows.push(Row::new(cols.clone(), vals));
    }
    Ok(rows)
}

/// Ingest one or all worksheets from an Excel / OpenDocument file into TapirusDB SQL table(s).
pub fn ingest_excel_file(
    conn: &Connection,
    path: &Path,
    target_table_override: Option<&str>,
    source_sheet: Option<&str>,
    batch_size: usize,
    extra_partitions: &[(String, String)],
) -> Result<IngestStats> {
    let start = Instant::now();
    let mut workbook = open_workbook_auto(path)
        .map_err(|e| Error::Corrupted(format!("Excel open error for '{}': {e}", path.display())))?;
    let all_sheets = workbook.sheet_names();
    if all_sheets.is_empty() {
        return Ok(IngestStats {
            rows_ingested: 0,
            files_processed: 1,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            target: target_table_override.unwrap_or("sheet").to_string(),
            columns: 0,
        });
    }

    let selected_sheets: Vec<String> = if let Some(wanted) = source_sheet {
        let matched = all_sheets
            .iter()
            .find(|s| s.eq_ignore_ascii_case(wanted))
            .cloned()
            .ok_or_else(|| {
                Error::TableNotFound(format!(
                    "Worksheet '{wanted}' not found in '{}'",
                    path.display()
                ))
            })?;
        vec![matched]
    } else if target_table_override.is_some() && all_sheets.len() == 1 {
        vec![all_sheets[0].clone()]
    } else {
        all_sheets
    };

    let eff_batch = batch_size.max(512);
    let mut total_rows = 0usize;
    let mut last_target = String::new();
    let mut max_cols = 0usize;

    for sheet in &selected_sheets {
        let range = workbook
            .worksheet_range(sheet)
            .map_err(|e| Error::Corrupted(format!("Excel sheet '{sheet}' error: {e}")))?;
        let mut raw_rows: Vec<&[Data]> = range.rows().collect();
        if raw_rows.is_empty() {
            continue;
        }

        let header_cells = raw_rows.remove(0);
        let mut clean_cols = build_clean_headers(header_cells);
        let base_width = clean_cols.len();
        let col_types = infer_excel_column_types(&raw_rows, base_width);

        let target_tbl = if selected_sheets.len() == 1 {
            target_table_override
                .map(sanitize_table_stem)
                .unwrap_or_else(|| sanitize_table_stem(sheet))
        } else {
            sanitize_table_stem(sheet)
        };

        let mut extra_defs = Vec::new();
        let mut extra_vals = Vec::new();
        for (k, v) in extra_partitions {
            if !clean_cols.iter().any(|c| c.eq_ignore_ascii_case(k)) {
                clean_cols.push(k.clone());
                if let Ok(i) = v.parse::<i64>() {
                    extra_defs.push(format!("{k} INTEGER"));
                    extra_vals.push(Value::Integer(i));
                } else if let Ok(f) = v.parse::<f64>() {
                    extra_defs.push(format!("{k} REAL"));
                    extra_vals.push(Value::Real(f));
                } else {
                    extra_defs.push(format!("{k} TEXT"));
                    extra_vals.push(Value::Text(v.clone()));
                }
            }
        }

        if conn.table(&target_tbl).is_none() {
            let mut col_defs: Vec<String> = clean_cols[..base_width]
                .iter()
                .zip(col_types.iter())
                .map(|(n, dt)| format!("{n} {}", excel_data_type_sql(dt)))
                .collect();
            col_defs.extend(extra_defs);
            let ddl = format!(
                "CREATE TABLE IF NOT EXISTS {target_tbl} ({});",
                col_defs.join(", ")
            );
            conn.execute(&ddl)?;
        }

        let mut batch = Vec::with_capacity(eff_batch);
        for r in raw_rows {
            if r.iter().all(|c| matches!(c, Data::Empty)) {
                continue;
            }
            let mut vals = Vec::with_capacity(clean_cols.len());
            for (idx, dt) in col_types.iter().enumerate() {
                let cell = r.get(idx).unwrap_or(&Data::Empty);
                vals.push(cell_to_value(cell, dt));
            }
            vals.extend(extra_vals.iter().cloned());
            batch.push(vals);
            if batch.len() >= eff_batch {
                total_rows += conn.insert_rows_batch(&target_tbl, Some(&clean_cols), &batch)?;
                batch.clear();
            }
        }
        if !batch.is_empty() {
            total_rows += conn.insert_rows_batch(&target_tbl, Some(&clean_cols), &batch)?;
        }

        last_target = target_tbl;
        max_cols = max_cols.max(clean_cols.len());
    }

    Ok(IngestStats {
        rows_ingested: total_rows,
        files_processed: 1,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        target: last_target,
        columns: max_cols,
    })
}

/// Export TapirusDB query/table rows to a native Excel `.xlsx` workbook.
pub fn write_rows_to_excel(rows: &[Row], sheet_name: &str, dest: &Path) -> Result<ExportStats> {
    let start = Instant::now();
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();
    let clean_sheet: String = sheet_name
        .chars()
        .map(|c| {
            if matches!(c, '[' | ']' | ':' | '*' | '?' | '/' | '\\') {
                '_'
            } else {
                c
            }
        })
        .take(31)
        .collect();
    let safe_sheet = if clean_sheet.is_empty() {
        "Sheet1".to_string()
    } else {
        clean_sheet
    };
    worksheet
        .set_name(&safe_sheet)
        .map_err(|e| Error::Corrupted(format!("Excel worksheet name error: {e}")))?;

    if !rows.is_empty() {
        let bold = Format::new().set_bold();
        for (c_idx, col_name) in rows[0].columns().iter().enumerate() {
            worksheet
                .write_string_with_format(0, c_idx as u16, col_name, &bold)
                .map_err(|e| Error::Corrupted(format!("Excel header write error: {e}")))?;
        }

        for (r_idx, row) in rows.iter().enumerate() {
            let excel_row = (r_idx + 1) as u32;
            for (c_idx, val) in row.values().iter().enumerate() {
                let col = c_idx as u16;
                match val {
                    Value::Null => {}
                    Value::Integer(i) => {
                        worksheet
                            .write_number(excel_row, col, *i as f64)
                            .map_err(|e| Error::Corrupted(format!("Excel write int error: {e}")))?;
                    }
                    Value::Real(f) => {
                        worksheet.write_number(excel_row, col, *f).map_err(|e| {
                            Error::Corrupted(format!("Excel write float error: {e}"))
                        })?;
                    }
                    Value::Text(s) => {
                        worksheet.write_string(excel_row, col, s).map_err(|e| {
                            Error::Corrupted(format!("Excel write text error: {e}"))
                        })?;
                    }
                    Value::Vector(v) => {
                        let s = serde_json::to_string(v).unwrap_or_default();
                        worksheet.write_string(excel_row, col, &s).map_err(|e| {
                            Error::Corrupted(format!("Excel write vector error: {e}"))
                        })?;
                    }
                    Value::Blob(b) => {
                        let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
                        worksheet
                            .write_string(excel_row, col, format!("0x{hex}"))
                            .map_err(|e| {
                                Error::Corrupted(format!("Excel write blob error: {e}"))
                            })?;
                    }
                }
            }
        }
    }

    workbook
        .save(dest)
        .map_err(|e| Error::Corrupted(format!("Excel save error for '{}': {e}", dest.display())))?;

    Ok(ExportStats {
        rows_exported: rows.len(),
        files_written: 1,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}
