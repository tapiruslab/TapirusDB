//! Apache Parquet & Arrow columnar Data Lake reader and writer.
//!
//! Reads and writes standard `.parquet` files (SNAPPY/LZ4/GZIP/UNCOMPRESSED),
//! maps Arrow `FixedSizeList<Float32>` vector columns natively into TapirusDB
//! `VECTOR` types, and supports Hive-partitioned Data Lake exports.
//!
//! Complies strictly with `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

use crate::Connection;
use crate::error::{Error, Result};
use crate::lakehouse::csv_stream::{
    ExportStats, IngestStats, parse_field_inferred, sanitize_column_names,
};
use crate::traits::{Row, Value};
use arrow_array::RecordBatch;
use arrow_array::array::{
    Array, BinaryArray, BooleanArray, Date32Array, Date64Array, FixedSizeListArray, Float32Array,
    Float64Array, Int8Array, Int16Array, Int32Array, Int64Array, LargeBinaryArray,
    LargeStringArray, ListArray, StringArray, StringViewArray, TimestampMicrosecondArray,
    TimestampMillisecondArray, TimestampNanosecondArray, TimestampSecondArray, UInt8Array,
    UInt16Array, UInt32Array, UInt64Array,
};
use arrow_array::builder::{
    BinaryBuilder, Float32Builder, Float64Builder, Int64Builder, ListBuilder, StringBuilder,
};
use arrow_schema::{DataType as ArrowDataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

fn arrow_type_to_tapir_sql(dt: &ArrowDataType) -> String {
    match dt {
        ArrowDataType::Boolean
        | ArrowDataType::Int8
        | ArrowDataType::Int16
        | ArrowDataType::Int32
        | ArrowDataType::Int64
        | ArrowDataType::UInt8
        | ArrowDataType::UInt16
        | ArrowDataType::UInt32
        | ArrowDataType::UInt64
        | ArrowDataType::Date32
        | ArrowDataType::Date64
        | ArrowDataType::Timestamp(_, _) => "INTEGER".to_string(),
        ArrowDataType::Float16 | ArrowDataType::Float32 | ArrowDataType::Float64 => {
            "REAL".to_string()
        }
        ArrowDataType::Binary | ArrowDataType::LargeBinary | ArrowDataType::FixedSizeBinary(_) => {
            "BLOB".to_string()
        }
        ArrowDataType::FixedSizeList(field, len)
            if matches!(
                field.data_type(),
                ArrowDataType::Float32 | ArrowDataType::Float64
            ) =>
        {
            format!("VECTOR({len})")
        }
        ArrowDataType::List(field)
            if matches!(
                field.data_type(),
                ArrowDataType::Float32 | ArrowDataType::Float64
            ) =>
        {
            "VECTOR".to_string()
        }
        _ => "TEXT".to_string(),
    }
}

fn extract_arrow_cell(col: &dyn Array, row_idx: usize) -> Value {
    if col.is_null(row_idx) {
        return Value::Null;
    }
    match col.data_type() {
        ArrowDataType::Boolean => {
            let arr = col.as_any().downcast_ref::<BooleanArray>().unwrap();
            Value::Integer(if arr.value(row_idx) { 1 } else { 0 })
        }
        ArrowDataType::Int8 => {
            let arr = col.as_any().downcast_ref::<Int8Array>().unwrap();
            Value::Integer(arr.value(row_idx) as i64)
        }
        ArrowDataType::Int16 => {
            let arr = col.as_any().downcast_ref::<Int16Array>().unwrap();
            Value::Integer(arr.value(row_idx) as i64)
        }
        ArrowDataType::Int32 => {
            let arr = col.as_any().downcast_ref::<Int32Array>().unwrap();
            Value::Integer(arr.value(row_idx) as i64)
        }
        ArrowDataType::Int64 => {
            let arr = col.as_any().downcast_ref::<Int64Array>().unwrap();
            Value::Integer(arr.value(row_idx))
        }
        ArrowDataType::UInt8 => {
            let arr = col.as_any().downcast_ref::<UInt8Array>().unwrap();
            Value::Integer(arr.value(row_idx) as i64)
        }
        ArrowDataType::UInt16 => {
            let arr = col.as_any().downcast_ref::<UInt16Array>().unwrap();
            Value::Integer(arr.value(row_idx) as i64)
        }
        ArrowDataType::UInt32 => {
            let arr = col.as_any().downcast_ref::<UInt32Array>().unwrap();
            Value::Integer(arr.value(row_idx) as i64)
        }
        ArrowDataType::UInt64 => {
            let arr = col.as_any().downcast_ref::<UInt64Array>().unwrap();
            Value::Integer(arr.value(row_idx) as i64)
        }
        ArrowDataType::Date32 => {
            let arr = col.as_any().downcast_ref::<Date32Array>().unwrap();
            Value::Integer(arr.value(row_idx) as i64)
        }
        ArrowDataType::Date64 => {
            let arr = col.as_any().downcast_ref::<Date64Array>().unwrap();
            Value::Integer(arr.value(row_idx))
        }
        ArrowDataType::Timestamp(unit, _) => {
            use arrow_schema::TimeUnit;
            match unit {
                TimeUnit::Second => Value::Integer(
                    col.as_any()
                        .downcast_ref::<TimestampSecondArray>()
                        .unwrap()
                        .value(row_idx),
                ),
                TimeUnit::Millisecond => Value::Integer(
                    col.as_any()
                        .downcast_ref::<TimestampMillisecondArray>()
                        .unwrap()
                        .value(row_idx),
                ),
                TimeUnit::Microsecond => Value::Integer(
                    col.as_any()
                        .downcast_ref::<TimestampMicrosecondArray>()
                        .unwrap()
                        .value(row_idx),
                ),
                TimeUnit::Nanosecond => Value::Integer(
                    col.as_any()
                        .downcast_ref::<TimestampNanosecondArray>()
                        .unwrap()
                        .value(row_idx),
                ),
            }
        }
        ArrowDataType::Float32 => {
            let arr = col.as_any().downcast_ref::<Float32Array>().unwrap();
            Value::Real(arr.value(row_idx) as f64)
        }
        ArrowDataType::Float64 => {
            let arr = col.as_any().downcast_ref::<Float64Array>().unwrap();
            Value::Real(arr.value(row_idx))
        }
        ArrowDataType::Utf8 => {
            let arr = col.as_any().downcast_ref::<StringArray>().unwrap();
            Value::Text(arr.value(row_idx).to_string())
        }
        ArrowDataType::LargeUtf8 => {
            let arr = col.as_any().downcast_ref::<LargeStringArray>().unwrap();
            Value::Text(arr.value(row_idx).to_string())
        }
        ArrowDataType::Utf8View => {
            let arr = col.as_any().downcast_ref::<StringViewArray>().unwrap();
            Value::Text(arr.value(row_idx).to_string())
        }
        ArrowDataType::Binary => {
            let arr = col.as_any().downcast_ref::<BinaryArray>().unwrap();
            Value::Blob(arr.value(row_idx).to_vec())
        }
        ArrowDataType::LargeBinary => {
            let arr = col.as_any().downcast_ref::<LargeBinaryArray>().unwrap();
            Value::Blob(arr.value(row_idx).to_vec())
        }
        ArrowDataType::FixedSizeList(_, _) => {
            let arr = col.as_any().downcast_ref::<FixedSizeListArray>().unwrap();
            let slice = arr.value(row_idx);
            if let Some(f32_arr) = slice.as_any().downcast_ref::<Float32Array>() {
                Value::Vector(f32_arr.values().to_vec())
            } else if let Some(f64_arr) = slice.as_any().downcast_ref::<Float64Array>() {
                Value::Vector(f64_arr.values().iter().map(|&v| v as f32).collect())
            } else {
                Value::Text(format!("[list len={}]", slice.len()))
            }
        }
        ArrowDataType::List(_) => {
            let arr = col.as_any().downcast_ref::<ListArray>().unwrap();
            let slice = arr.value(row_idx);
            if let Some(f32_arr) = slice.as_any().downcast_ref::<Float32Array>() {
                Value::Vector(f32_arr.values().to_vec())
            } else if let Some(f64_arr) = slice.as_any().downcast_ref::<Float64Array>() {
                Value::Vector(f64_arr.values().iter().map(|&v| v as f32).collect())
            } else {
                Value::Text(format!("[list len={}]", slice.len()))
            }
        }
        other => Value::Text(format!("{other:?}")),
    }
}

/// Stream an Apache Parquet file directly into a TapirusDB SQL table via batch inserts.
pub fn ingest_parquet_file(
    conn: &Connection,
    path: &Path,
    table_name: &str,
    batch_size: usize,
    extra_partitions: &[(String, String)],
) -> Result<IngestStats> {
    let start = Instant::now();
    let file = File::open(path)?;
    let eff_batch = batch_size.max(1024);

    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| Error::Corrupted(format!("Parquet open error: {e}")))?
        .with_batch_size(eff_batch);

    let arrow_schema = builder.schema().clone();
    let raw_names: Vec<String> = arrow_schema
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    let mut clean_cols = sanitize_column_names(&raw_names);

    let mut extra_vals = Vec::new();
    let mut extra_defs = Vec::new();
    for (p_col, p_val) in extra_partitions {
        if !clean_cols.iter().any(|c| c.eq_ignore_ascii_case(p_col)) {
            clean_cols.push(p_col.clone());
            let parsed = parse_field_inferred(p_val);
            let sql_ty = match &parsed {
                Value::Integer(_) => "INTEGER",
                Value::Real(_) => "REAL",
                _ => "TEXT",
            };
            extra_defs.push(format!("{p_col} {sql_ty}"));
            extra_vals.push(parsed);
        }
    }

    if conn.table(table_name).is_none() {
        let mut defs: Vec<String> = clean_cols[..raw_names.len()]
            .iter()
            .zip(arrow_schema.fields().iter())
            .map(|(name, field)| format!("{name} {}", arrow_type_to_tapir_sql(field.data_type())))
            .collect();
        defs.extend(extra_defs);
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {} ({});",
            table_name,
            defs.join(", ")
        );
        conn.execute(&sql)?;
    }

    let reader = builder
        .build()
        .map_err(|e| Error::Corrupted(format!("Parquet reader build error: {e}")))?;

    let mut total_rows = 0usize;
    for batch_res in reader {
        let batch = batch_res
            .map_err(|e| Error::Corrupted(format!("Parquet record batch read error: {e}")))?;
        let num_rows = batch.num_rows();
        let num_cols = batch.num_columns();
        let mut rows_vals = Vec::with_capacity(num_rows);
        for r in 0..num_rows {
            let mut vals = Vec::with_capacity(clean_cols.len());
            for c in 0..num_cols {
                vals.push(extract_arrow_cell(batch.column(c).as_ref(), r));
            }
            vals.extend(extra_vals.iter().cloned());
            rows_vals.push(vals);
        }
        total_rows += conn.insert_rows_batch(table_name, Some(&clean_cols), &rows_vals)?;
    }

    Ok(IngestStats {
        rows_ingested: total_rows,
        files_processed: 1,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        target: table_name.to_string(),
        columns: clean_cols.len(),
    })
}

/// Read rows from an Apache Parquet file directly for SQL `read_parquet(...)` scans.
pub fn scan_parquet_rows(
    path: &Path,
    extra_partitions: &[(String, String)],
    limit: Option<usize>,
) -> Result<Vec<Row>> {
    let file = File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| Error::Corrupted(format!("Parquet open error: {e}")))?
        .with_batch_size(4096);

    let arrow_schema = builder.schema().clone();
    let raw_names: Vec<String> = arrow_schema
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    let mut clean_cols = sanitize_column_names(&raw_names);
    let mut extra_vals = Vec::new();
    for (p_col, p_val) in extra_partitions {
        if !clean_cols.iter().any(|c| c.eq_ignore_ascii_case(p_col)) {
            clean_cols.push(p_col.clone());
            extra_vals.push(parse_field_inferred(p_val));
        }
    }

    let reader = builder
        .build()
        .map_err(|e| Error::Corrupted(format!("Parquet reader build error: {e}")))?;

    let mut rows = Vec::new();
    'outer: for batch_res in reader {
        let batch =
            batch_res.map_err(|e| Error::Corrupted(format!("Parquet batch read error: {e}")))?;
        for r in 0..batch.num_rows() {
            if let Some(lim) = limit
                && rows.len() >= lim
            {
                break 'outer;
            }
            let mut vals = Vec::with_capacity(clean_cols.len());
            for c in 0..batch.num_columns() {
                vals.push(extract_arrow_cell(batch.column(c).as_ref(), r));
            }
            vals.extend(extra_vals.iter().cloned());
            rows.push(Row::new(clean_cols.clone(), vals));
        }
    }
    Ok(rows)
}

#[derive(Clone, Copy)]
enum ExportColKind {
    Integer,
    Real,
    Blob,
    Vector,
    Text,
}

fn infer_arrow_export_schema(rows: &[Row]) -> (Arc<Schema>, Vec<ExportColKind>) {
    let Some(first) = rows.first() else {
        return (Arc::new(Schema::empty()), Vec::new());
    };
    let cols = first.columns();
    let mut fields = Vec::with_capacity(cols.len());
    let mut kinds = Vec::with_capacity(cols.len());

    for (idx, col_name) in cols.iter().enumerate() {
        let mut saw_int = false;
        let mut saw_real = false;
        let mut saw_blob = false;
        let mut saw_vec = false;
        let mut saw_text = false;

        for r in rows.iter().take(512) {
            if let Some(v) = r.values().get(idx) {
                match v {
                    Value::Null => {}
                    Value::Integer(_) => saw_int = true,
                    Value::Real(_) => saw_real = true,
                    Value::Blob(_) => saw_blob = true,
                    Value::Vector(_) => saw_vec = true,
                    Value::Text(_) => saw_text = true,
                }
            }
        }

        let (kind, arrow_dt) = if saw_text {
            (ExportColKind::Text, ArrowDataType::Utf8)
        } else if saw_vec && !saw_int && !saw_real && !saw_blob {
            (
                ExportColKind::Vector,
                ArrowDataType::List(Arc::new(Field::new("item", ArrowDataType::Float32, true))),
            )
        } else if saw_blob && !saw_int && !saw_real {
            (ExportColKind::Blob, ArrowDataType::Binary)
        } else if saw_real {
            (ExportColKind::Real, ArrowDataType::Float64)
        } else if saw_int {
            (ExportColKind::Integer, ArrowDataType::Int64)
        } else {
            (ExportColKind::Text, ArrowDataType::Utf8)
        };

        fields.push(Field::new(col_name, arrow_dt, true));
        kinds.push(kind);
    }

    (Arc::new(Schema::new(fields)), kinds)
}

fn build_record_batch(
    schema: Arc<Schema>,
    kinds: &[ExportColKind],
    rows: &[Row],
) -> Result<RecordBatch> {
    let mut arrays: Vec<Arc<dyn Array>> = Vec::with_capacity(kinds.len());

    for (col_idx, kind) in kinds.iter().enumerate() {
        let array: Arc<dyn Array> = match kind {
            ExportColKind::Integer => {
                let mut b = Int64Builder::with_capacity(rows.len());
                for r in rows {
                    match r.values().get(col_idx) {
                        Some(Value::Integer(n)) => b.append_value(*n),
                        Some(Value::Real(f)) => b.append_value(*f as i64),
                        _ => b.append_null(),
                    }
                }
                Arc::new(b.finish())
            }
            ExportColKind::Real => {
                let mut b = Float64Builder::with_capacity(rows.len());
                for r in rows {
                    match r.values().get(col_idx) {
                        Some(Value::Real(f)) => b.append_value(*f),
                        Some(Value::Integer(n)) => b.append_value(*n as f64),
                        _ => b.append_null(),
                    }
                }
                Arc::new(b.finish())
            }
            ExportColKind::Blob => {
                let mut b = BinaryBuilder::new();
                for r in rows {
                    match r.values().get(col_idx) {
                        Some(Value::Blob(bytes)) => b.append_value(bytes),
                        _ => b.append_null(),
                    }
                }
                Arc::new(b.finish())
            }
            ExportColKind::Vector => {
                let mut b = ListBuilder::new(Float32Builder::new());
                for r in rows {
                    match r.values().get(col_idx) {
                        Some(Value::Vector(vec)) => {
                            for &x in vec {
                                b.values().append_value(x);
                            }
                            b.append(true);
                        }
                        _ => b.append(false),
                    }
                }
                Arc::new(b.finish())
            }
            ExportColKind::Text => {
                let mut b = StringBuilder::new();
                for r in rows {
                    match r.values().get(col_idx) {
                        Some(Value::Null) | None => b.append_null(),
                        Some(Value::Text(s)) => b.append_value(s),
                        Some(other) => b.append_value(other.to_string()),
                    }
                }
                Arc::new(b.finish())
            }
        };
        arrays.push(array);
    }

    RecordBatch::try_new(schema, arrays)
        .map_err(|e| Error::Corrupted(format!("Arrow RecordBatch build error: {e}")))
}

/// Export rows to an Apache Parquet `.parquet` file (or Hive-partitioned directory when `partition_by` is set).
pub fn write_rows_to_parquet(
    rows: &[Row],
    dest: &Path,
    partition_by: Option<&str>,
) -> Result<ExportStats> {
    let start = Instant::now();
    if rows.is_empty() {
        let empty_schema = Arc::new(Schema::empty());
        if let Some(parent) = dest.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let file = File::create(dest)?;
        let props = WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .build();
        let writer = ArrowWriter::try_new(file, empty_schema, Some(props))
            .map_err(|e| Error::Corrupted(format!("Parquet writer init error: {e}")))?;
        writer
            .close()
            .map_err(|e| Error::Corrupted(format!("Parquet writer close error: {e}")))?;
        return Ok(ExportStats {
            rows_exported: 0,
            files_written: 1,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        });
    }

    if let Some(part_col) = partition_by {
        let first_cols = rows[0].columns();
        let p_idx = first_cols
            .iter()
            .position(|c| c.eq_ignore_ascii_case(part_col))
            .ok_or_else(|| {
                Error::Corrupted(format!(
                    "Partition column '{part_col}' does not exist in result columns"
                ))
            })?;

        let mut grouped: BTreeMap<String, Vec<Row>> = BTreeMap::new();
        for row in rows {
            let p_val = match row.values().get(p_idx) {
                Some(Value::Text(s)) => s.clone(),
                Some(Value::Integer(n)) => n.to_string(),
                Some(Value::Real(f)) => f.to_string(),
                _ => "null".to_string(),
            };
            let safe_val: String = p_val
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            grouped.entry(safe_val).or_default().push(row.clone());
        }

        let mut files_written = 0usize;
        for (p_val, part_rows) in &grouped {
            let part_dir = dest.join(format!("{part_col}={p_val}"));
            std::fs::create_dir_all(&part_dir)?;
            let out_file = part_dir.join("part-0.parquet");
            write_single_parquet(&out_file, part_rows)?;
            files_written += 1;
        }

        return Ok(ExportStats {
            rows_exported: rows.len(),
            files_written,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        });
    }

    write_single_parquet(dest, rows)?;
    Ok(ExportStats {
        rows_exported: rows.len(),
        files_written: 1,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

fn write_single_parquet(dest: &Path, rows: &[Row]) -> Result<()> {
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let (schema, kinds) = infer_arrow_export_schema(rows);
    let file = File::create(dest)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props))
        .map_err(|e| Error::Corrupted(format!("Parquet writer init error: {e}")))?;

    for chunk in rows.chunks(8192) {
        let batch = build_record_batch(schema.clone(), &kinds, chunk)?;
        writer
            .write(&batch)
            .map_err(|e| Error::Corrupted(format!("Parquet write batch error: {e}")))?;
    }
    writer
        .close()
        .map_err(|e| Error::Corrupted(format!("Parquet writer close error: {e}")))?;
    Ok(())
}
