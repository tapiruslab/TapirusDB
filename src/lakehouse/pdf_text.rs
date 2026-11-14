//! PDF and plain-text document extraction, smart chunking, and multi-model ingestion.
//!
//! Extracts text page-by-page from `.pdf` files (under the `lakehouse` feature via pure-Rust `lopdf`)
//! and reads `.txt`, `.log`, and `.md` files into either:
//! - Structured SQL tables (`Table`),
//! - JSON Document collections (`Document`), or
//! - AI Agent Memory + Knowledge Graph nodes (`AiMemory`).
//!
//! Complies strictly with `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

use crate::Connection;
use crate::error::{Error, Result};
use crate::lakehouse::csv_stream::IngestStats;
use crate::memory::embedder::EmbeddingEngine;
use crate::traits::{Row, Value};
use std::path::Path;
use std::time::Instant;

/// Destination mode when ingesting `.pdf`, `.txt`, `.log`, or `.md` documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextImportMode {
    /// Store extracted pages/chunks into a structured relational SQL table.
    #[default]
    Table,
    /// Store extracted pages/chunks as JSON documents in a Collection.
    Document,
    /// Chunk and embed into AI Agent Memory (128D + BM25) and Knowledge Graph (GraphRAG).
    AiMemory,
}

/// Extracted logical text chunk from a PDF page or text document.
#[derive(Debug, Clone)]
pub struct ExtractedChunk {
    /// Source filename.
    pub source: String,
    /// 1-based page number (1 for plain text/markdown files).
    pub page: usize,
    /// 0-based chunk index within the document.
    pub chunk_index: usize,
    /// Section heading or page title.
    pub heading: String,
    /// Cleaned chunk text content.
    pub content: String,
}

/// Extract text chunks from a `.pdf` file using `lopdf` (requires `lakehouse` feature).
#[cfg(feature = "lakehouse")]
pub fn extract_pdf_chunks(
    path: &Path,
    max_chars: usize,
    overlap_chars: usize,
) -> Result<Vec<ExtractedChunk>> {
    let doc = lopdf::Document::load(path)
        .map_err(|e| Error::Corrupted(format!("Failed to load PDF '{}': {e}", path.display())))?;
    let source = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document.pdf")
        .to_string();

    let pages = doc.get_pages();
    let mut chunks = Vec::new();
    let mut chunk_idx = 0usize;

    for &page_num in pages.keys() {
        let raw_text = doc.extract_text(&[page_num]).unwrap_or_default();
        let cleaned = normalize_whitespace(&raw_text);
        if cleaned.is_empty() {
            continue;
        }
        let page_title = format!("Page {page_num}");
        for part in split_text_chunks(&cleaned, max_chars, overlap_chars) {
            chunks.push(ExtractedChunk {
                source: source.clone(),
                page: page_num as usize,
                chunk_index: chunk_idx,
                heading: page_title.clone(),
                content: part,
            });
            chunk_idx += 1;
        }
    }

    Ok(chunks)
}

/// Fallback error when the `lakehouse` feature is disabled.
#[cfg(not(feature = "lakehouse"))]
pub fn extract_pdf_chunks(
    _path: &Path,
    _max_chars: usize,
    _overlap_chars: usize,
) -> Result<Vec<ExtractedChunk>> {
    Err(Error::Corrupted(
        "PDF extraction requires the 'lakehouse' feature to be enabled".into(),
    ))
}

/// Extract text chunks from `.txt`, `.log`, or `.md` files.
pub fn extract_text_file_chunks(
    path: &Path,
    max_chars: usize,
    overlap_chars: usize,
) -> Result<Vec<ExtractedChunk>> {
    let raw = std::fs::read_to_string(path)?;
    let source = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document.txt")
        .to_string();

    let is_markdown = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));

    let mut sections: Vec<(String, String)> = Vec::new();
    if is_markdown {
        let mut current_heading = source.clone();
        let mut body_lines: Vec<&str> = Vec::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                let body = body_lines.join("\n").trim().to_string();
                if !body.is_empty() {
                    sections.push((current_heading.clone(), body));
                }
                current_heading = trimmed.trim_start_matches('#').trim().to_string();
                if current_heading.is_empty() {
                    current_heading = source.clone();
                }
                body_lines.clear();
            } else {
                body_lines.push(line);
            }
        }
        let tail = body_lines.join("\n").trim().to_string();
        if !tail.is_empty() {
            sections.push((current_heading, tail));
        }
    } else {
        let cleaned = normalize_plain_text(&raw);
        if !cleaned.is_empty() {
            sections.push((source.clone(), cleaned));
        }
    }

    let mut chunks = Vec::new();
    let mut chunk_idx = 0usize;
    for (heading, body) in sections {
        for part in split_text_chunks(&body, max_chars, overlap_chars) {
            chunks.push(ExtractedChunk {
                source: source.clone(),
                page: 1,
                chunk_index: chunk_idx,
                heading: heading.clone(),
                content: part,
            });
            chunk_idx += 1;
        }
    }
    Ok(chunks)
}

/// Convert extracted chunks directly into `Vec<Row>` for SQL `read_pdf(...)` / `read_text(...)`.
pub fn chunks_to_rows(chunks: &[ExtractedChunk]) -> Vec<Row> {
    let cols = vec![
        "chunk_id".to_string(),
        "source".to_string(),
        "page".to_string(),
        "chunk_index".to_string(),
        "heading".to_string(),
        "content".to_string(),
        "char_count".to_string(),
    ];
    chunks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            Row::new(
                cols.clone(),
                vec![
                    Value::Integer((i + 1) as i64),
                    Value::Text(c.source.clone()),
                    Value::Integer(c.page as i64),
                    Value::Integer(c.chunk_index as i64),
                    Value::Text(c.heading.clone()),
                    Value::Text(c.content.clone()),
                    Value::Integer(c.content.chars().count() as i64),
                ],
            )
        })
        .collect()
}

/// Ingest a `.pdf`, `.txt`, `.log`, or `.md` file into TapirusDB using the chosen `TextImportMode`.
pub fn ingest_unstructured_file(
    conn: &Connection,
    path: &Path,
    target_name: &str,
    mode: TextImportMode,
    max_chars: usize,
    overlap_chars: usize,
    tags: &[&str],
) -> Result<IngestStats> {
    let start = Instant::now();
    let is_pdf = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));

    let chunks = if is_pdf {
        extract_pdf_chunks(path, max_chars, overlap_chars)?
    } else {
        extract_text_file_chunks(path, max_chars, overlap_chars)?
    };

    if chunks.is_empty() {
        return Ok(IngestStats {
            rows_ingested: 0,
            files_processed: 1,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            target: target_name.to_string(),
            columns: 0,
        });
    }

    match mode {
        TextImportMode::Table => {
            let create_sql = format!(
                "CREATE TABLE IF NOT EXISTS {target_name} (id INTEGER PRIMARY KEY, source TEXT, page INTEGER, chunk_index INTEGER, heading TEXT, content TEXT, char_count INTEGER);"
            );
            conn.execute(&create_sql)?;
            let cols = vec![
                "source".to_string(),
                "page".to_string(),
                "chunk_index".to_string(),
                "heading".to_string(),
                "content".to_string(),
                "char_count".to_string(),
            ];
            let mut total = 0usize;
            for batch in chunks.chunks(1000) {
                let rows: Vec<Vec<Value>> = batch
                    .iter()
                    .map(|c| {
                        vec![
                            Value::Text(c.source.clone()),
                            Value::Integer(c.page as i64),
                            Value::Integer(c.chunk_index as i64),
                            Value::Text(c.heading.clone()),
                            Value::Text(c.content.clone()),
                            Value::Integer(c.content.chars().count() as i64),
                        ]
                    })
                    .collect();
                total += conn.insert_rows_batch(target_name, Some(&cols), &rows)?;
            }
            Ok(IngestStats {
                rows_ingested: total,
                files_processed: 1,
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                target: target_name.to_string(),
                columns: 7,
            })
        }
        TextImportMode::Document => {
            let col = conn.collection(target_name)?;
            for chunk in &chunks {
                let doc = serde_json::json!({
                    "source": chunk.source,
                    "page": chunk.page,
                    "chunk_index": chunk.chunk_index,
                    "heading": chunk.heading,
                    "content": chunk.content,
                    "char_count": chunk.content.chars().count(),
                });
                col.insert_one(&doc)?;
            }
            Ok(IngestStats {
                rows_ingested: chunks.len(),
                files_processed: 1,
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                target: target_name.to_string(),
                columns: 6,
            })
        }
        TextImportMode::AiMemory => {
            let embedder = crate::DeterministicHashEmbedder::default();
            let mut next_node_id = conn
                .graph_nodes()
                .into_iter()
                .map(|n| n.id)
                .max()
                .unwrap_or(0)
                + 1;

            let doc_node_id = next_node_id;
            next_node_id += 1;
            let filename = chunks[0].source.clone();
            let doc_props = serde_json::json!({
                "filename": filename,
                "path": path.to_string_lossy(),
                "chunks_count": chunks.len(),
                "namespace": target_name,
            });
            let _ = conn.graph_add_node(doc_node_id, "Document", &doc_props.to_string());

            let mut prev_node: Option<u64> = None;
            let mut stored = 0usize;
            for chunk in &chunks {
                let text = if chunk.heading == chunk.source {
                    chunk.content.clone()
                } else {
                    format!("[{}] {}", chunk.heading, chunk.content)
                };
                if conn
                    .memory_remember_text_scoped(&text, 0.7, tags, Some(target_name), None)
                    .is_ok()
                {
                    stored += 1;
                }
                let node_id = next_node_id;
                next_node_id += 1;
                let vec = embedder.embed_text(&text);
                let props = serde_json::json!({
                    "title": chunk.heading,
                    "page": chunk.page,
                    "chunk_index": chunk.chunk_index,
                    "source": chunk.source,
                    "char_count": chunk.content.chars().count(),
                });
                if conn
                    .graph_add_node_with_vector(node_id, "Section", &props.to_string(), Some(&vec))
                    .is_ok()
                {
                    let _ = conn.graph_add_edge(doc_node_id, node_id, "CONTAINS", 1.0, "{}");
                    if let Some(prev) = prev_node {
                        let _ = conn.graph_add_edge(prev, node_id, "PRECEDES", 1.0, "{}");
                    }
                    prev_node = Some(node_id);
                }
            }
            Ok(IngestStats {
                rows_ingested: stored,
                files_processed: 1,
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                target: target_name.to_string(),
                columns: 1,
            })
        }
    }
}

fn normalize_whitespace(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_space = true;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    out.trim().to_string()
}

fn normalize_plain_text(raw: &str) -> String {
    raw.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Split text cleanly along character/word boundaries with optional overlap.
pub fn split_text_chunks(text: &str, max_chars: usize, overlap_chars: usize) -> Vec<String> {
    let max_len = max_chars.max(64);
    let overlap = overlap_chars.min(max_len / 2);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= max_len {
        return vec![trimmed.to_string()];
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let mut end = (start + max_len).min(chars.len());
        if end < chars.len() {
            // Try to snap backwards to a sentence or whitespace boundary.
            let min_snap = start + (max_len / 2);
            let mut snap = end;
            while snap > min_snap {
                let c = chars[snap - 1];
                if c == '.' || c == '!' || c == '?' || c == '\n' || c == ' ' {
                    break;
                }
                snap -= 1;
            }
            if snap > min_snap {
                end = snap;
            }
        }
        let slice: String = chars[start..end].iter().collect();
        let cleaned = slice.trim();
        if !cleaned.is_empty() {
            out.push(cleaned.to_string());
        }
        if end >= chars.len() {
            break;
        }
        let step = (end - start).saturating_sub(overlap).max(1);
        start += step;
    }
    out
}
