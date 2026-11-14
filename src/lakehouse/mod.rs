//! Big Data, Data Lakehouse, and Multi-Format Data Conversion Engine for TapirusDB.
//!
//! Provides high-throughput ingestion, SQL external table scans, and zero-copy
//! cross-format conversions across:
//! - Tabular & Lakehouse formats: CSV, TSV, `.csv.gz`, JSON/JSONL, Apache Parquet (`.parquet`)
//! - Hive-partitioned directories & wildcard globs (e.g. `lake/year=2026/*.parquet`)
//! - Other databases: Pure-Rust SQLite3 (`.db`, `.sqlite`, `.sqlite3`) & SQL Dumps (`.sql` from PostgreSQL/MySQL/SQLite/DuckDB)
//! - Unstructured documents: PDF (`.pdf`), Plain Text (`.txt`, `.log`), and Markdown (`.md`)
//! - Native `.tapir` interchange & export back to `.parquet`, `.csv`, `.jsonl`, or `.sql`.
//!
//! Complies strictly with `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

pub mod csv_stream;
#[cfg(feature = "lakehouse")]
pub mod parquet_io;
pub mod pdf_text;
pub mod sql_dump;
pub mod sqlite_reader;

pub use csv_stream::{ExportStats, IngestStats, ingest_csv_fast, ingest_jsonl_as_table};
#[cfg(feature = "lakehouse")]
pub use parquet_io::{ingest_parquet_file, scan_parquet_rows, write_rows_to_parquet};
pub use pdf_text::{
    ExtractedChunk, TextImportMode, extract_pdf_chunks, extract_text_file_chunks,
    ingest_unstructured_file,
};
pub use sql_dump::{import_sql_dump, write_rows_to_sql_dump};
pub use sqlite_reader::{
    SqliteEncoding, SqliteFileReader, SqliteTableInfo, import_sqlite_file, scan_sqlite_table_rows,
};

use crate::Connection;
use crate::error::{Error, Result};
use crate::traits::Row;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Supported external data and database formats for lakehouse scans & conversions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFormat {
    /// Comma-separated values (`.csv`, `.csv.gz`).
    Csv,
    /// Tab-separated values (`.tsv`, `.tsv.gz`).
    Tsv,
    /// Newline-delimited JSON or JSON object array (`.jsonl`, `.ndjson`, `.json`, `.jsonl.gz`).
    Jsonl,
    /// Apache Parquet columnar Data Lake files (`.parquet`).
    Parquet,
    /// SQLite 3 binary database (`.db`, `.sqlite`, `.sqlite3`).
    Sqlite,
    /// Universal SQL dump (`.sql` from PostgreSQL, MySQL, SQLite, or DuckDB).
    SqlDump,
    /// Adobe PDF document (`.pdf`).
    Pdf,
    /// Plain text or log file (`.txt`, `.text`, `.log`).
    Text,
    /// Markdown document (`.md`, `.markdown`).
    Markdown,
    /// Native TapirusDB single-file database (`.tapir`).
    Tapir,
}

impl DataFormat {
    /// Infer format from a file extension or explicit format token.
    pub fn parse_format_or_path(explicit: Option<&str>, path: &str) -> Result<Self> {
        if let Some(fmt) = explicit {
            return match fmt.trim().to_ascii_lowercase().as_str() {
                "csv" => Ok(Self::Csv),
                "tsv" => Ok(Self::Tsv),
                "jsonl" | "ndjson" | "json" => Ok(Self::Jsonl),
                "parquet" | "pq" => Ok(Self::Parquet),
                "sqlite" | "sqlite3" | "db" => Ok(Self::Sqlite),
                "sql" | "dump" => Ok(Self::SqlDump),
                "pdf" => Ok(Self::Pdf),
                "txt" | "text" | "log" => Ok(Self::Text),
                "md" | "markdown" => Ok(Self::Markdown),
                "tapir" | "tapirus" => Ok(Self::Tapir),
                other => Err(Error::SqlSyntax(format!(
                    "Unsupported data format: '{other}'"
                ))),
            };
        }

        let lower = path.trim().to_ascii_lowercase();
        let trimmed = lower.strip_suffix(".gz").unwrap_or(&lower);
        if trimmed.ends_with(".csv") {
            Ok(Self::Csv)
        } else if trimmed.ends_with(".tsv") {
            Ok(Self::Tsv)
        } else if trimmed.ends_with(".jsonl")
            || trimmed.ends_with(".ndjson")
            || trimmed.ends_with(".json")
        {
            Ok(Self::Jsonl)
        } else if trimmed.ends_with(".parquet") {
            Ok(Self::Parquet)
        } else if trimmed.ends_with(".sqlite")
            || trimmed.ends_with(".sqlite3")
            || trimmed.ends_with(".db")
        {
            Ok(Self::Sqlite)
        } else if trimmed.ends_with(".sql") {
            Ok(Self::SqlDump)
        } else if trimmed.ends_with(".pdf") {
            Ok(Self::Pdf)
        } else if trimmed.ends_with(".md") || trimmed.ends_with(".markdown") {
            Ok(Self::Markdown)
        } else if trimmed.ends_with(".txt")
            || trimmed.ends_with(".text")
            || trimmed.ends_with(".log")
        {
            Ok(Self::Text)
        } else if trimmed.ends_with(".tapir") {
            Ok(Self::Tapir)
        } else {
            Err(Error::SqlSyntax(format!(
                "Cannot infer data format from path '{path}'. Pass an explicit --format."
            )))
        }
    }
}

/// Options controlling ingestion, Data Lake scanning, and format conversions.
#[derive(Debug, Clone)]
pub struct LakeIngestOptions {
    /// Target SQL table, collection, or namespace name (defaults to file stem).
    pub target_name: Option<String>,
    /// Source table name inside multi-table inputs (`.sqlite`, `.tapir`).
    pub source_table: Option<String>,
    /// Batch commit size for high-throughput table streaming (default: 2048).
    pub batch_size: usize,
    /// Optional explicit delimiter override for CSV/TSV.
    pub delimiter: Option<char>,
    /// Destination mode for `.pdf`, `.txt`, and `.md` inputs.
    pub text_mode: TextImportMode,
    /// Chunk size (characters) when splitting `.pdf` or `.txt` documents.
    pub chunk_size: usize,
    /// Overlap size (characters) between consecutive document chunks.
    pub chunk_overlap: usize,
    /// Optional tags attached to AI Memory chunks.
    pub tags: Vec<String>,
}

impl Default for LakeIngestOptions {
    fn default() -> Self {
        Self {
            target_name: None,
            source_table: None,
            batch_size: 2048,
            delimiter: None,
            text_mode: TextImportMode::Table,
            chunk_size: 1200,
            chunk_overlap: 150,
            tags: Vec::new(),
        }
    }
}

/// Resolved external file path together with any Hive partition `(col, value)` pairs.
#[derive(Debug, Clone)]
pub struct ResolvedLakeFile {
    /// Concrete local file path.
    pub path: PathBuf,
    /// Extracted Hive partition columns from ancestor directories (e.g. `year=2026`).
    pub partitions: Vec<(String, String)>,
}

/// Discover concrete files matching a path, directory, or wildcard glob (`*`, `?`, `**`).
/// Extracts Hive-style `key=value` directory segments automatically.
pub fn discover_lake_files(
    pattern: &str,
    expected_ext: Option<&str>,
) -> Result<Vec<ResolvedLakeFile>> {
    let clean = pattern.trim().trim_matches(|c| c == '\'' || c == '"');
    let p = Path::new(clean);

    // Direct single file fast-path
    if !clean.contains('*') && !clean.contains('?') {
        if p.is_file() {
            return Ok(vec![ResolvedLakeFile {
                path: p.to_path_buf(),
                partitions: extract_hive_partitions(p),
            }]);
        }
        if p.is_dir() {
            let mut found = Vec::new();
            collect_dir_files(p, expected_ext, &mut found)?;
            found.sort_by(|a, b| a.path.cmp(&b.path));
            return Ok(found);
        }
    }

    // Wildcard / glob matching relative to the static non-glob directory prefix
    let norm = clean.replace('\\', "/");
    let first_wild = norm.find(['*', '?']).unwrap_or(norm.len());
    let base_prefix = if let Some(slash) = norm[..first_wild].rfind('/') {
        let dir = &norm[..slash];
        if dir.is_empty() { "." } else { dir }
    } else {
        "."
    };

    let mut all_files = Vec::new();
    let base_path = Path::new(base_prefix);
    if base_path.exists() {
        collect_dir_files(base_path, None, &mut all_files)?;
    }

    let mut matched = Vec::new();
    for item in all_files {
        let candidate_norm = item.path.to_string_lossy().replace('\\', "/");
        if glob_match(&norm, &candidate_norm)
            || (norm.starts_with("./") && glob_match(&norm[2..], &candidate_norm))
        {
            matched.push(item);
        }
    }
    matched.sort_by(|a, b| a.path.cmp(&b.path));
    if matched.is_empty() {
        return Err(Error::Corrupted(format!(
            "No files matched Data Lake path or pattern '{clean}'"
        )));
    }
    Ok(matched)
}

fn collect_dir_files(
    dir: &Path,
    expected_ext: Option<&str>,
    out: &mut Vec<ResolvedLakeFile>,
) -> Result<()> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_dir_files(&path, expected_ext, out)?;
        } else if path.is_file() {
            if let Some(ext) = expected_ext {
                let name_lower = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !name_lower.ends_with(&ext.to_ascii_lowercase()) {
                    continue;
                }
            }
            out.push(ResolvedLakeFile {
                partitions: extract_hive_partitions(&path),
                path,
            });
        }
    }
    Ok(())
}

fn extract_hive_partitions(path: &Path) -> Vec<(String, String)> {
    let mut parts = Vec::new();
    for comp in path.components() {
        let s = comp.as_os_str().to_string_lossy();
        if let Some((k, v)) = s.split_once('=')
            && !k.is_empty()
            && !v.is_empty()
            && !k.contains(':')
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            parts.push((k.to_string(), v.to_string()));
        }
    }
    parts
}

/// Simple glob matcher supporting `*`, `?`, and `**`.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let t_parts: Vec<&str> = text.split('/').filter(|s| !s.is_empty()).collect();
    glob_segments(&p_parts, &t_parts)
}

fn glob_segments(pat: &[&str], txt: &[&str]) -> bool {
    if pat.is_empty() {
        return txt.is_empty();
    }
    if pat[0] == "**" {
        if glob_segments(&pat[1..], txt) {
            return true;
        }
        return !txt.is_empty() && glob_segments(pat, &txt[1..]);
    }
    if txt.is_empty() {
        return false;
    }
    segment_match(pat[0], txt[0]) && glob_segments(&pat[1..], &txt[1..])
}

fn segment_match(pat: &str, txt: &str) -> bool {
    let pb: Vec<char> = pat.chars().collect();
    let tb: Vec<char> = txt.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_idx, mut match_idx) = (None, 0usize);

    while ti < tb.len() {
        if pi < pb.len() && (pb[pi] == '?' || pb[pi] == tb[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pb.len() && pb[pi] == '*' {
            star_idx = Some(pi);
            match_idx = ti;
            pi += 1;
        } else if let Some(si) = star_idx {
            pi = si + 1;
            match_idx += 1;
            ti = match_idx;
        } else {
            return false;
        }
    }
    while pi < pb.len() && pb[pi] == '*' {
        pi += 1;
    }
    pi == pb.len()
}

/// Scan rows directly from external files (`read_csv`, `read_parquet`, `read_jsonl`, `read_sqlite`, `read_pdf`, `read_text`).
pub fn scan_external_rows(
    func_or_format: &str,
    source_path: &str,
    sub_arg: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<Row>> {
    let norm_fn = func_or_format.trim().to_ascii_lowercase();
    let format = match norm_fn.as_str() {
        "read_csv" | "csv" => DataFormat::Csv,
        "read_tsv" | "tsv" => DataFormat::Tsv,
        "read_jsonl" | "read_ndjson" | "read_json" | "jsonl" | "json" => DataFormat::Jsonl,
        "read_parquet" | "parquet" => DataFormat::Parquet,
        "read_sqlite" | "sqlite" => DataFormat::Sqlite,
        "read_pdf" | "pdf" => DataFormat::Pdf,
        "read_text" | "read_md" | "text" | "txt" | "md" => DataFormat::Text,
        other => DataFormat::parse_format_or_path(Some(other), source_path)?,
    };

    let files = discover_lake_files(source_path, None)?;
    let mut all_rows = Vec::new();
    for file in files {
        let remaining = limit.map(|l| l.saturating_sub(all_rows.len()));
        if remaining == Some(0) {
            break;
        }
        let chunk = match format {
            DataFormat::Csv => {
                csv_stream::scan_csv_rows(&file.path, None, &file.partitions, remaining)?
            }
            DataFormat::Tsv => {
                csv_stream::scan_csv_rows(&file.path, Some('\t'), &file.partitions, remaining)?
            }
            DataFormat::Jsonl => {
                csv_stream::scan_jsonl_rows(&file.path, &file.partitions, remaining)?
            }
            DataFormat::Parquet => {
                #[cfg(feature = "lakehouse")]
                {
                    parquet_io::scan_parquet_rows(&file.path, &file.partitions, remaining)?
                }
                #[cfg(not(feature = "lakehouse"))]
                {
                    return Err(Error::Corrupted(
                        "Parquet reading requires the 'lakehouse' feature".into(),
                    ));
                }
            }
            DataFormat::Sqlite => {
                let tbl = sub_arg.ok_or_else(|| {
                    Error::SqlSyntax(
                        "read_sqlite('file.db', 'table_name') requires a target table argument"
                            .into(),
                    )
                })?;
                sqlite_reader::scan_sqlite_table_rows(&file.path, tbl, remaining)?
            }
            DataFormat::Pdf => {
                let chunks = pdf_text::extract_pdf_chunks(&file.path, 1200, 150)?;
                let mut rows = pdf_text::chunks_to_rows(&chunks);
                if let Some(lim) = remaining {
                    rows.truncate(lim);
                }
                rows
            }
            DataFormat::Text | DataFormat::Markdown => {
                let chunks = pdf_text::extract_text_file_chunks(&file.path, 1200, 150)?;
                let mut rows = pdf_text::chunks_to_rows(&chunks);
                if let Some(lim) = remaining {
                    rows.truncate(lim);
                }
                rows
            }
            DataFormat::SqlDump | DataFormat::Tapir => {
                return Err(Error::SqlSyntax(format!(
                    "Direct SQL scan is not supported for {format:?}; use `tapirus convert` or `COPY` instead"
                )));
            }
        };
        all_rows.extend(chunk);
    }

    Ok(all_rows)
}

/// Ingest one or more files (supports globs & Hive partitions) into an active TapirusDB `Connection`.
pub fn ingest_into_connection(
    conn: &Connection,
    source_pattern: &str,
    explicit_format: Option<&str>,
    options: &LakeIngestOptions,
) -> Result<IngestStats> {
    let start = Instant::now();
    let format = DataFormat::parse_format_or_path(explicit_format, source_pattern)?;
    let files = discover_lake_files(source_pattern, None)?;

    let first_stem = files[0]
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("data")
        .trim_end_matches(".csv")
        .trim_end_matches(".tsv")
        .trim_end_matches(".jsonl")
        .to_string();
    let default_target = options
        .target_name
        .clone()
        .unwrap_or_else(|| sanitize_table_stem(&first_stem));

    let mut total_rows = 0usize;
    let mut max_cols = 0usize;
    let file_count = files.len();

    for file in &files {
        let stats = match format {
            DataFormat::Csv => csv_stream::ingest_csv_fast(
                conn,
                &file.path,
                &default_target,
                options.delimiter,
                options.batch_size,
                &file.partitions,
            )?,
            DataFormat::Tsv => csv_stream::ingest_csv_fast(
                conn,
                &file.path,
                &default_target,
                Some(options.delimiter.unwrap_or('\t')),
                options.batch_size,
                &file.partitions,
            )?,
            DataFormat::Jsonl => csv_stream::ingest_jsonl_as_table(
                conn,
                &file.path,
                &default_target,
                options.batch_size,
                &file.partitions,
            )?,
            DataFormat::Parquet => {
                #[cfg(feature = "lakehouse")]
                {
                    parquet_io::ingest_parquet_file(
                        conn,
                        &file.path,
                        &default_target,
                        options.batch_size,
                        &file.partitions,
                    )?
                }
                #[cfg(not(feature = "lakehouse"))]
                {
                    return Err(Error::Corrupted(
                        "Parquet ingestion requires the 'lakehouse' feature".into(),
                    ));
                }
            }
            DataFormat::Sqlite => {
                let per_table = sqlite_reader::import_sqlite_file(
                    conn,
                    &file.path,
                    options.source_table.as_deref(),
                    options.target_name.as_deref(),
                    options.batch_size,
                )?;
                let sum_rows: usize = per_table.iter().map(|s| s.rows_ingested).sum();
                let cols = per_table.first().map(|s| s.columns).unwrap_or(0);
                let target = per_table
                    .first()
                    .map(|s| s.target.clone())
                    .unwrap_or_else(|| default_target.clone());
                IngestStats {
                    rows_ingested: sum_rows,
                    files_processed: 1,
                    elapsed_ms: 0.0,
                    target,
                    columns: cols,
                }
            }
            DataFormat::SqlDump => sql_dump::import_sql_dump(
                conn,
                &file.path,
                options.target_name.as_deref(),
                options.batch_size,
            )?,
            DataFormat::Pdf | DataFormat::Text | DataFormat::Markdown => {
                let tag_refs: Vec<&str> = options.tags.iter().map(|s| s.as_str()).collect();
                pdf_text::ingest_unstructured_file(
                    conn,
                    &file.path,
                    &default_target,
                    options.text_mode,
                    options.chunk_size,
                    options.chunk_overlap,
                    &tag_refs,
                )?
            }
            DataFormat::Tapir => {
                return Err(Error::SqlSyntax(
                    "Importing a .tapir file into another .tapir should use `tapirus backup` or table copy".into(),
                ));
            }
        };
        total_rows += stats.rows_ingested;
        max_cols = max_cols.max(stats.columns);
    }

    Ok(IngestStats {
        rows_ingested: total_rows,
        files_processed: file_count,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        target: default_target,
        columns: max_cols,
    })
}

/// Export rows from a query/table to CSV, TSV, JSONL, Parquet, or universal SQL dump.
pub fn export_rows_to_path(
    rows: &[Row],
    table_name: &str,
    dest: &Path,
    explicit_format: Option<&str>,
    partition_by: Option<&str>,
) -> Result<ExportStats> {
    let dest_str = dest.to_string_lossy();
    let format = DataFormat::parse_format_or_path(explicit_format, &dest_str)?;

    match format {
        DataFormat::Csv => csv_stream::write_rows_to_csv(rows, dest, ','),
        DataFormat::Tsv => csv_stream::write_rows_to_csv(rows, dest, '\t'),
        DataFormat::Jsonl => csv_stream::write_rows_to_jsonl(rows, dest),
        DataFormat::SqlDump => sql_dump::write_rows_to_sql_dump(table_name, rows, dest),
        DataFormat::Parquet => {
            #[cfg(feature = "lakehouse")]
            {
                parquet_io::write_rows_to_parquet(rows, dest, partition_by)
            }
            #[cfg(not(feature = "lakehouse"))]
            {
                let _ = partition_by;
                Err(Error::Corrupted(
                    "Parquet export requires the 'lakehouse' feature".into(),
                ))
            }
        }
        other => Err(Error::SqlSyntax(format!(
            "Unsupported export destination format: {other:?}"
        ))),
    }
}

/// Universal converter between any supported input format (`.csv`, `.tsv`, `.jsonl`,
/// `.parquet`, `.sqlite/.db`, `.sql`, `.pdf`, `.txt`, `.md`, `.tapir`) and any supported
/// output format (`.tapir`, `.parquet`, `.csv`, `.tsv`, `.jsonl`, `.sql`).
pub fn convert_dataset(
    src_pattern: &str,
    dest_path: &Path,
    src_format: Option<&str>,
    dest_format: Option<&str>,
    passphrase: Option<&str>,
    options: &LakeIngestOptions,
    partition_by: Option<&str>,
) -> Result<(IngestStats, Option<ExportStats>)> {
    let dest_str = dest_path.to_string_lossy();
    let out_fmt = DataFormat::parse_format_or_path(dest_format, &dest_str)?;
    let in_fmt = DataFormat::parse_format_or_path(src_format, src_pattern)?;

    if out_fmt == DataFormat::Tapir {
        let conn = if let Some(pass) = passphrase {
            Connection::open_encrypted(dest_path, pass)?
        } else {
            Connection::open(dest_path)?
        };
        let stats = ingest_into_connection(&conn, src_pattern, src_format, options)?;
        conn.checkpoint()?;
        return Ok((stats, None));
    }

    if in_fmt == DataFormat::Tapir {
        let src_path = Path::new(src_pattern);
        let conn = if let Some(pass) = passphrase {
            Connection::open_encrypted(src_path, pass)?
        } else {
            Connection::open_read_only(src_path)?
        };
        let table = if let Some(ref tbl) = options.source_table {
            tbl.clone()
        } else {
            conn.tables()
                .into_iter()
                .map(|t| t.name)
                .find(|t| !t.starts_with("__"))
                .ok_or_else(|| Error::TableNotFound("No user tables found in .tapir".into()))?
        };
        let rows = conn.query(&format!("SELECT * FROM {table};"))?;
        let export_stats =
            export_rows_to_path(&rows, &table, dest_path, dest_format, partition_by)?;
        let ingest_stats = IngestStats {
            rows_ingested: rows.len(),
            files_processed: 1,
            elapsed_ms: export_stats.elapsed_ms,
            target: table,
            columns: rows.first().map(|r| r.columns().len()).unwrap_or(0),
        };
        return Ok((ingest_stats, Some(export_stats)));
    }

    // Cross-format conversion (e.g., CSV -> Parquet, SQLite -> Parquet, PDF -> CSV/JSONL, SQL -> Parquet)
    let conn = Connection::open_in_memory()?;
    let ingest_stats = ingest_into_connection(&conn, src_pattern, src_format, options)?;
    let table = if !ingest_stats.target.is_empty() && conn.table(&ingest_stats.target).is_some() {
        ingest_stats.target.clone()
    } else {
        conn.tables()
            .into_iter()
            .map(|t| t.name)
            .find(|t| !t.starts_with("__"))
            .ok_or_else(|| Error::TableNotFound("No converted table found".into()))?
    };
    let rows = conn.query(&format!("SELECT * FROM {table};"))?;
    let export_stats = export_rows_to_path(&rows, &table, dest_path, dest_format, partition_by)?;
    Ok((ingest_stats, Some(export_stats)))
}

fn sanitize_table_stem(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() || out.chars().all(|c| c == '_') {
        out = "dataset".to_string();
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert_str(0, "t_");
    }
    out
}
