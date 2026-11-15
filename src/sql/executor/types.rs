use super::eval::{matches_condition, try_eval_scalar_expr};
use crate::btree::BTreeStorage;
use crate::error::{Error, Result};
use crate::pager::Pager;
use crate::sql::catalog::{IndexDef, TableDef, TableForeignKey};
use crate::sql::codec::encode_row_into;
use crate::sql::lexer::Token;
use crate::sql::parser::{BinaryOp, WhereCondition, WhereExpr};
use crate::traits::{Row, Value};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[inline]
fn canonical_f64_bits(v: f64) -> u64 {
    if v == 0.0 { 0u64 } else { v.to_bits() }
}

/// Pre-indexed lookup structure for O(1) `IN (...)` and `IN (SELECT ...)` predicate checks.
#[derive(Debug, Clone, PartialEq)]
pub struct InValueLookup {
    /// Whether the candidate set contains `NULL` (governs `NOT IN` three-valued logic).
    pub has_null: bool,
    /// Integer candidates.
    pub ints: HashSet<i64>,
    /// Canonicalized `f64` bit patterns (`-0.0` normalized to `0.0`).
    pub reals: HashSet<u64>,
    /// Text candidates (probed via `&str` with zero cloning).
    pub texts: HashSet<String>,
    /// Fallback list for `Blob` and `Vector` values.
    pub others: Vec<Value>,
}

impl InValueLookup {
    /// Build a hash-indexed lookup table from a slice of candidate `Value` items.
    pub fn from_values(values: &[Value]) -> Self {
        let mut lookup = Self {
            has_null: false,
            ints: HashSet::new(),
            reals: HashSet::new(),
            texts: HashSet::new(),
            others: Vec::new(),
        };
        for v in values {
            match v {
                Value::Null => lookup.has_null = true,
                Value::Integer(i) => {
                    lookup.ints.insert(*i);
                }
                Value::Real(r) => {
                    if !r.is_nan() {
                        lookup.reals.insert(canonical_f64_bits(*r));
                    }
                }
                Value::Text(s) => {
                    if !lookup.texts.contains(s.as_str()) {
                        lookup.texts.insert(s.clone());
                    }
                }
                other => lookup.others.push(other.clone()),
            }
        }
        lookup
    }

    /// Check in O(1) expected time whether a non-NULL `Value` matches any candidate.
    #[inline]
    pub fn contains(&self, val: &Value) -> bool {
        match val {
            Value::Null => false,
            Value::Integer(i) => {
                self.ints.contains(i)
                    || (!self.reals.is_empty()
                        && self.reals.contains(&canonical_f64_bits(*i as f64)))
            }
            Value::Real(r) => {
                if r.is_nan() {
                    return false;
                }
                if self.reals.contains(&canonical_f64_bits(*r)) {
                    return true;
                }
                if !self.ints.is_empty() && r.fract() == 0.0 && *r >= (i64::MIN as f64) {
                    let cand = *r as i64;
                    if (cand as f64) == *r && self.ints.contains(&cand) {
                        return true;
                    }
                }
                false
            }
            Value::Text(s) => self.texts.contains(s.as_str()),
            other => self.others.iter().any(|t| t == other),
        }
    }
}

/// Resolved Boolean expression tree with subqueries evaluated
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedWhereExpr {
    /// Basic comparison condition
    Condition(WhereCondition),
    /// Logical AND
    And(Box<ResolvedWhereExpr>, Box<ResolvedWhereExpr>),
    /// Logical OR
    Or(Box<ResolvedWhereExpr>, Box<ResolvedWhereExpr>),
    /// Resolved IN list
    InList {
        /// Target column name
        column: String,
        /// Set of resolved candidate values
        values: Vec<Value>,
        /// Whether the condition is NOT IN
        negated: bool,
    },
    /// Hash-indexed IN list / materialized subquery for O(1) per-row evaluation
    InSet {
        /// Target column name
        column: String,
        /// Original candidate values
        values: Vec<Value>,
        /// Pre-indexed lookup structure
        lookup: Arc<InValueLookup>,
        /// Whether the condition is NOT IN
        negated: bool,
    },
    /// Evaluated boolean predicate (e.g. from EXISTS / NOT EXISTS subqueries)
    Boolean(bool),
}

/// A temporal version record for point-in-time SQL time-travel queries
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalRecord {
    /// Unique temporal version ID
    pub id: u64,
    /// Target table name
    pub table_name: String,
    /// Target row ID
    pub row_id: u64,
    /// Inclusive epoch timestamp from which this version became valid
    pub valid_from: u64,
    /// Exclusive epoch timestamp up to which this version was valid (u64::MAX if currently active)
    pub valid_to: u64,
    /// Encoded row binary payload
    pub payload: Vec<u8>,
}

/// Ultra-fast 64-bit integer hasher using splitmix64 mixing for internal hash joins
#[derive(Default, Clone, Copy)]
pub struct FastIntHasher(u64);

impl std::hash::Hasher for FastIntHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, _bytes: &[u8]) {}
    #[inline]
    fn write_i64(&mut self, i: i64) {
        let mut x = (i as u64) ^ 0x517cc1b727220a95;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
        self.0 = x ^ (x >> 31);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.write_i64(i as i64);
    }
}

/// BuildHasher for FastIntHasher
pub type FastIntBuildHasher = std::hash::BuildHasherDefault<FastIntHasher>;

/// Zero-allocation hash table entry for 1-to-1 or 1-to-many joins
#[derive(Clone, Debug)]
pub enum IntEntry {
    /// Single row index match (zero allocation)
    One(usize),
    /// Multiple row index matches
    Many(Vec<usize>),
}

/// Identifies whether a projected join column originates from the left or right table
#[derive(Clone, Copy, Debug)]
pub enum JoinOutputCol {
    /// Column belongs to left table at given index
    Left(usize),
    /// Column belongs to right table at given index
    Right(usize),
}

/// Resolves a column name (qualified like 'table.column' or unqualified 'column') to its index in TableDef
#[inline]
pub fn resolve_table_col_index(table_def: &TableDef, col: &str) -> Option<usize> {
    if let Some((_, c)) = col.split_once('.') {
        table_def.column_index(c)
    } else {
        table_def.column_index(col)
    }
}

/// Deterministic 64-bit hash of a SQL Value for B+Tree secondary index key.
///
/// Backed by the frozen [`crate::stable_hash`] SipHash-1-3 so persisted index keys
/// never change across Rust releases, endianness or pointer width.
#[inline]
pub fn value_to_index_key(val: &Value) -> u64 {
    crate::stable_hash::key_for_value(val)
}

/// A posting entry associating a specific Value with its matching row IDs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexPostingEntry {
    /// Indexed value key
    pub value: Value,
    /// List of row IDs containing this indexed value
    pub row_ids: Vec<u64>,
}

/// An inverted index posting list bucket stored in B+Tree leaf cells
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexBucket {
    /// List of indexed value posting entries
    pub entries: Vec<IndexPostingEntry>,
}

impl IndexBucket {
    /// Create a new empty index posting bucket
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Retrieve the row IDs matching the specified value, if present
    pub fn get_row_ids(&self, val: &Value) -> Option<&[u64]> {
        self.entries
            .iter()
            .find(|e| &e.value == val)
            .map(|e| e.row_ids.as_slice())
    }

    /// Associate a row ID with the specified indexed value
    pub fn add_row_id(&mut self, val: Value, row_id: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.value == val) {
            if !entry.row_ids.contains(&row_id) {
                entry.row_ids.push(row_id);
            }
        } else {
            self.entries.push(IndexPostingEntry {
                value: val,
                row_ids: vec![row_id],
            });
        }
    }

    /// Remove a row ID from an indexed value, returning whether it was found and removed
    pub fn remove_row_id(&mut self, val: &Value, row_id: u64) -> bool {
        let mut changed = false;
        if let Some(entry) = self.entries.iter_mut().find(|e| &e.value == val) {
            let before = entry.row_ids.len();
            entry.row_ids.retain(|&id| id != row_id);
            changed = entry.row_ids.len() != before;
        }
        self.entries.retain(|e| !e.row_ids.is_empty());
        changed
    }

    /// Returns true if this bucket has no indexed entries
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serialize into the compact binary posting format (`TIB1`).
    ///
    /// Layout: `[0xB7, 0x01] varint(entries)` then per entry
    /// `varint(value_len) value(codec row) varint(row_count) zigzag-delta varint row_ids`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.entries.len() * 24);
        out.extend_from_slice(&[INDEX_BUCKET_MAGIC, INDEX_BUCKET_VERSION]);
        let mut vbuf = [0u8; 9];
        let mut put = |out: &mut Vec<u8>, v: u64| {
            let n = crate::btree::encode_varint(v, &mut vbuf);
            out.extend_from_slice(&vbuf[..n]);
        };
        put(&mut out, self.entries.len() as u64);
        let mut value_buf = Vec::new();
        for entry in &self.entries {
            encode_row_into(std::slice::from_ref(&entry.value), &mut value_buf);
            put(&mut out, value_buf.len() as u64);
            out.extend_from_slice(&value_buf);
            put(&mut out, entry.row_ids.len() as u64);
            let mut prev = 0i64;
            for &rid in &entry.row_ids {
                let cur = rid as i64;
                let delta = cur.wrapping_sub(prev);
                put(&mut out, ((delta << 1) ^ (delta >> 63)) as u64);
                prev = cur;
            }
        }
        out
    }

    /// Decode a posting bucket, accepting both the binary format and legacy JSON
    /// buckets written by TapirusDB `<= 1.1.0`. Corruption is reported, never ignored.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        match bytes.first() {
            Some(&INDEX_BUCKET_MAGIC) => Self::decode_binary(bytes),
            Some(_) => serde_json::from_slice::<IndexBucket>(bytes)
                .map_err(|e| Error::Corrupted(format!("Invalid legacy index bucket: {e}"))),
            None => Ok(Self::new()),
        }
    }

    fn decode_binary(bytes: &[u8]) -> Result<Self> {
        let corrupt = |what: &str| Error::Corrupted(format!("Index bucket corrupted: {what}"));
        if bytes.len() < 2 || bytes[1] != INDEX_BUCKET_VERSION {
            return Err(corrupt("unsupported version"));
        }
        let mut pos = 2usize;
        let next = |pos: &mut usize| -> Result<u64> {
            let slice = bytes.get(*pos..).ok_or_else(|| corrupt("truncated"))?;
            let (v, n) = crate::btree::decode_varint(slice)?;
            *pos += n;
            Ok(v)
        };
        let count = next(&mut pos)? as usize;
        if count > bytes.len() {
            return Err(corrupt("entry count exceeds payload"));
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let vlen = next(&mut pos)? as usize;
            let end = pos
                .checked_add(vlen)
                .ok_or_else(|| corrupt("value length"))?;
            let raw = bytes
                .get(pos..end)
                .ok_or_else(|| corrupt("value truncated"))?;
            pos = end;
            let value = crate::sql::codec::decode_row_values(raw)?
                .into_iter()
                .next()
                .ok_or_else(|| corrupt("empty value"))?;
            let rcount = next(&mut pos)? as usize;
            if rcount > bytes.len() {
                return Err(corrupt("row count exceeds payload"));
            }
            let mut row_ids = Vec::with_capacity(rcount);
            let mut prev = 0i64;
            for _ in 0..rcount {
                let z = next(&mut pos)?;
                let delta = ((z >> 1) as i64) ^ -((z & 1) as i64);
                prev = prev.wrapping_add(delta);
                row_ids.push(prev as u64);
            }
            entries.push(IndexPostingEntry { value, row_ids });
        }
        if pos != bytes.len() {
            return Err(corrupt("trailing bytes"));
        }
        Ok(Self { entries })
    }
}

/// First byte of a binary [`IndexBucket`] payload (never a valid JSON/UTF-8 lead byte).
pub const INDEX_BUCKET_MAGIC: u8 = 0xB7;
/// Binary [`IndexBucket`] format version.
pub const INDEX_BUCKET_VERSION: u8 = 0x01;

impl IndexBucket {
    /// Load the bucket stored under `key`, or an empty bucket when absent.
    pub(crate) fn load(
        btree: &BTreeStorage,
        pager: &mut Pager,
        root_page: crate::pager::PageId,
        key: u64,
    ) -> Result<Option<Self>> {
        match btree.search(pager, root_page, key)? {
            Some(payload) => Self::from_bytes(&payload).map(Some),
            None => Ok(None),
        }
    }
}

impl Default for IndexBucket {
    fn default() -> Self {
        Self::new()
    }
}

/// Persistent disk snapshot record of an HNSW multi-layer graph topology stored in system table
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSnapshotRecord {
    /// Target table name
    pub table_name: String,
    /// Serialized HNSW snapshot graph topology
    pub snapshot: crate::vector::HnswSnapshot,
    /// Checksum of the durable table rows used to build this snapshot.
    #[serde(default)]
    pub table_fingerprint: Option<u32>,
}

/// Extract a composite or single-column index value from a row.
/// Returns `None` if all indexed columns are `NULL` (SQL indexes omit all-NULL tuples).
/// The boolean indicates whether ALL columns in the key are non-NULL (for SQL UNIQUE checks).
pub(crate) fn extract_index_key_value<S: AsRef<str>>(
    row: &Row,
    cols: &[S],
) -> Option<(Value, bool)> {
    if cols.is_empty() {
        return None;
    }
    if cols.len() == 1 {
        let val = row.get_field_or_json_path(cols[0].as_ref())?;
        if val.is_null() {
            return None;
        }
        return Some((val, true));
    }
    let mut vals = Vec::with_capacity(cols.len());
    let mut any_non_null = false;
    let mut all_non_null = true;
    for col in cols {
        let v = row
            .get_field_or_json_path(col.as_ref())
            .unwrap_or(Value::Null);
        if v.is_null() {
            all_non_null = false;
        } else {
            any_non_null = true;
        }
        vals.push(v);
    }
    if !any_non_null {
        return None;
    }
    let encoded = serde_json::to_string(&vals).unwrap_or_default();
    Some((Value::Text(format!("__comp:{encoded}")), all_non_null))
}

pub(crate) fn collect_table_foreign_keys(table_def: &TableDef) -> Vec<TableForeignKey> {
    let mut fks = Vec::new();
    for col in &table_def.columns {
        if let Some(ref fk) = col.references {
            fks.push(TableForeignKey {
                name: None,
                columns: vec![col.name.clone()],
                ref_table: fk.table.clone(),
                ref_columns: fk.column.iter().cloned().collect(),
                on_delete: fk.on_delete,
                on_update: fk.on_update,
            });
        }
    }
    fks.extend(table_def.foreign_keys.iter().cloned());
    fks
}

pub(crate) fn resolve_fk_parent_cols(parent_def: &TableDef, fk: &TableForeignKey) -> Vec<String> {
    if !fk.ref_columns.is_empty() {
        fk.ref_columns.clone()
    } else if !parent_def.primary_key_columns.is_empty() {
        parent_def.primary_key_columns.clone()
    } else if let Some(pk) = parent_def.columns.iter().find(|c| c.primary_key) {
        vec![pk.name.clone()]
    } else {
        Vec::new()
    }
}

/// Three-valued SQL predicate evaluation for CHECK constraints (None = UNKNOWN -> CHECK passes).
pub(crate) fn eval_where_three_valued(
    row: &Row,
    expr: &WhereExpr,
    udfs: Option<&crate::sql::udf::UdfRegistry>,
) -> Option<bool> {
    match expr {
        WhereExpr::Condition(cond) => {
            let val = match row.get_field_or_json_path(&cond.column) {
                Some(v) => v,
                None => match try_eval_scalar_expr(row, &cond.column, udfs) {
                    Ok(Some(v)) => v,
                    _ => return None,
                },
            };
            match cond.op {
                BinaryOp::IsNull => Some(val.is_null()),
                BinaryOp::IsNotNull => Some(!val.is_null()),
                _ => {
                    if val.is_null() {
                        None
                    } else {
                        Some(matches_condition(&val, &cond.op, &cond.value))
                    }
                }
            }
        }
        WhereExpr::And(l, r) => match (
            eval_where_three_valued(row, l, udfs),
            eval_where_three_valued(row, r, udfs),
        ) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        },
        WhereExpr::Or(l, r) => match (
            eval_where_three_valued(row, l, udfs),
            eval_where_three_valued(row, r, udfs),
        ) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        },
        WhereExpr::InList {
            column,
            values,
            negated,
        } => {
            let val = row.get_field_or_json_path(column)?;
            if val.is_null() {
                None
            } else {
                let found = values.contains(&val);
                Some(if *negated { !found } else { found })
            }
        }
        WhereExpr::InSubquery { .. } | WhereExpr::Exists { .. } => Some(true),
    }
}

pub(crate) fn value_to_tokens(val: &Value) -> Vec<Token> {
    match val {
        Value::Null => vec![Token::Null],
        Value::Integer(i) if *i < 0 => vec![Token::Dash, Token::IntLit(i.unsigned_abs() as i64)],
        Value::Integer(i) => vec![Token::IntLit(*i)],
        Value::Real(f) if *f < 0.0 => vec![Token::Dash, Token::FloatLit(-*f)],
        Value::Real(f) => vec![Token::FloatLit(*f)],
        Value::Text(s) => vec![Token::StringLit(s.clone())],
        Value::Blob(b) => vec![Token::BlobLit(b.clone())],
        Value::Vector(v) => vec![Token::VectorLit(v.clone())],
    }
}

pub(crate) fn token_as_col_name(tok: &Token) -> Option<String> {
    crate::sql::parser::parse_token_identifier(tok)
}

pub(crate) fn substitute_trigger_row_tokens(
    tokens: &[Token],
    old_row: Option<&Row>,
    new_row: Option<&Row>,
) -> Vec<Token> {
    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0usize;
    while i < tokens.len() {
        if i + 2 < tokens.len()
            && tokens[i + 1] == Token::Dot
            && let Token::Ident(qualifier) = &tokens[i]
        {
            let target_row = if qualifier.eq_ignore_ascii_case("NEW") {
                new_row
            } else if qualifier.eq_ignore_ascii_case("OLD") {
                old_row
            } else {
                None
            };
            if let Some(row) = target_row
                && let Some(col_name) = token_as_col_name(&tokens[i + 2])
            {
                let val = row.get_value(&col_name).cloned().unwrap_or(Value::Null);
                out.extend(value_to_tokens(&val));
                i += 3;
                continue;
            }
        }
        out.push(tokens[i].clone());
        i += 1;
    }
    out
}

pub(crate) fn split_token_statements(tokens: &[Token]) -> Vec<Vec<Token>> {
    let mut stmts = Vec::new();
    let mut current = Vec::new();
    let mut depth = 0usize;
    for tok in tokens {
        match tok {
            Token::OpenParen => {
                depth += 1;
                current.push(tok.clone());
            }
            Token::CloseParen => {
                depth = depth.saturating_sub(1);
                current.push(tok.clone());
            }
            Token::Semicolon if depth == 0 => {
                if !current.is_empty() {
                    stmts.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(tok.clone()),
        }
    }
    if !current.is_empty() {
        stmts.push(current);
    }
    stmts
}

pub(crate) fn collect_and_eq_conditions(expr: &WhereExpr, out: &mut HashMap<String, Value>) {
    match expr {
        WhereExpr::Condition(c) if c.op == BinaryOp::Equals => {
            out.insert(c.column.to_lowercase(), c.value.clone());
        }
        WhereExpr::And(l, r) => {
            collect_and_eq_conditions(l, out);
            collect_and_eq_conditions(r, out);
        }
        _ => {}
    }
}

pub(crate) fn collect_and_leaf_predicates(
    expr: &WhereExpr,
    out: &mut Vec<crate::sql::parser::WhereCondition>,
) {
    match expr {
        WhereExpr::Condition(c) => out.push(c.clone()),
        WhereExpr::And(l, r) => {
            collect_and_leaf_predicates(l, out);
            collect_and_leaf_predicates(r, out);
        }
        _ => {}
    }
}

pub(crate) fn combine_compound_rows(
    left: &mut Vec<Row>,
    right: Vec<Row>,
    op: crate::sql::parser::SetOperation,
) {
    match op {
        crate::sql::parser::SetOperation::UnionAll => left.extend(right),
        crate::sql::parser::SetOperation::Union => {
            let mut seen = HashSet::with_capacity(left.len() + right.len());
            left.retain(|r| seen.insert(format!("{:?}", r.values())));
            for row in right {
                if seen.insert(format!("{:?}", row.values())) {
                    left.push(row);
                }
            }
        }
    }
}

pub(crate) fn apply_cte_column_names(rows: Vec<Row>, explicit_cols: Option<&[String]>) -> Vec<Row> {
    let Some(cols) = explicit_cols else {
        return rows;
    };
    rows.into_iter()
        .map(|r| {
            if r.values().len() == cols.len() {
                Row::new(cols.to_vec(), r.values().to_vec())
            } else {
                let vals = cols
                    .iter()
                    .map(|col| r.get_value(col).cloned().unwrap_or(Value::Null))
                    .collect();
                Row::new(cols.to_vec(), vals)
            }
        })
        .collect()
}
pub(crate) fn parse_partial_index_where(predicate_sql: &str) -> Option<WhereExpr> {
    let tokens = crate::sql::lexer::tokenize(predicate_sql).ok()?;
    let mut cursor = 0usize;
    let expr = crate::sql::parser::parse_where_expr_public(&tokens, &mut cursor).ok()?;
    if cursor == tokens.len() {
        Some(expr)
    } else {
        None
    }
}

pub(crate) fn partial_index_matches_query(
    index_def: &IndexDef,
    query_where: Option<&WhereExpr>,
) -> bool {
    let Some(ref pred_sql) = index_def.predicate_sql else {
        return true;
    };
    let Some(q_where) = query_where else {
        return false;
    };
    let Some(pred_expr) = parse_partial_index_where(pred_sql) else {
        return false;
    };
    // Match exact subtree OR every top-level AND condition of the partial predicate inside query ANDs.
    if pred_expr == *q_where {
        return true;
    }
    let mut pred_conds = Vec::new();
    let mut q_conds = Vec::new();
    collect_and_leaf_predicates(&pred_expr, &mut pred_conds);
    collect_and_leaf_predicates(q_where, &mut q_conds);
    if pred_conds.is_empty() {
        return false;
    }
    pred_conds.iter().all(|pc| {
        q_conds.iter().any(|qc| {
            pc.column.eq_ignore_ascii_case(&qc.column) && pc.op == qc.op && pc.value == qc.value
        })
    })
}

pub(crate) fn extract_fts_match_query(expr: &WhereExpr, table: &str) -> Option<String> {
    match expr {
        WhereExpr::Condition(c)
            if c.op == BinaryOp::Match && c.column.eq_ignore_ascii_case(table) =>
        {
            if let Value::Text(ref q) = c.value {
                Some(q.clone())
            } else {
                None
            }
        }
        WhereExpr::And(l, r) | WhereExpr::Or(l, r) => {
            extract_fts_match_query(l, table).or_else(|| extract_fts_match_query(r, table))
        }
        _ => None,
    }
}

pub(crate) fn resolve_fts_match_predicate(
    expr: &ResolvedWhereExpr,
    table: &str,
    row_matched: bool,
) -> ResolvedWhereExpr {
    match expr {
        ResolvedWhereExpr::Condition(c)
            if c.op == BinaryOp::Match && c.column.eq_ignore_ascii_case(table) =>
        {
            ResolvedWhereExpr::Boolean(row_matched)
        }
        ResolvedWhereExpr::And(l, r) => ResolvedWhereExpr::And(
            Box::new(resolve_fts_match_predicate(l, table, row_matched)),
            Box::new(resolve_fts_match_predicate(r, table, row_matched)),
        ),
        ResolvedWhereExpr::Or(l, r) => ResolvedWhereExpr::Or(
            Box::new(resolve_fts_match_predicate(l, table, row_matched)),
            Box::new(resolve_fts_match_predicate(r, table, row_matched)),
        ),
        other => other.clone(),
    }
}

/// Hash a slice of `Value` items canonically for O(1) deduplication without String formatting.
#[inline]
pub(crate) fn hash_values(values: &[Value]) -> u64 {
    let mut h = crate::stable_hash::StableHasher::new();
    h.write_u64(values.len() as u64);
    for v in values {
        match v {
            Value::Null => h.write_u8(0),
            Value::Integer(i) => {
                h.write_u8(1);
                h.write_u64(*i as u64);
            }
            Value::Real(f) => {
                h.write_u8(2);
                let norm = if *f == 0.0 { 0.0_f64 } else { *f };
                h.write_u64(norm.to_bits());
            }
            Value::Text(s) => {
                h.write_u8(3);
                h.write_len_prefixed(s.as_bytes());
            }
            Value::Blob(b) => {
                h.write_u8(4);
                h.write_len_prefixed(b);
            }
            Value::Vector(vec) => {
                h.write_u8(5);
                h.write_u64(vec.len() as u64);
                for &x in vec {
                    let norm = if x == 0.0 { 0.0_f32 } else { x };
                    h.write_u32(norm.to_bits());
                }
            }
        }
    }
    h.finish()
}

/// O(1) amortized collision-safe row deduplication set replacing O(N^2) `Vec::contains`.
#[derive(Default)]
pub(crate) struct RowDistinctSet {
    buckets: HashMap<u64, IntEntry, FastIntBuildHasher>,
    skipped_unique: Vec<Row>,
}

impl RowDistinctSet {
    #[inline]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn lookup_row<'a>(
        skipped_unique: &'a [Row],
        result_rows: &'a [Row],
        combined_idx: usize,
    ) -> &'a Row {
        if combined_idx < skipped_unique.len() {
            &skipped_unique[combined_idx]
        } else {
            &result_rows[combined_idx - skipped_unique.len()]
        }
    }

    /// Push `candidate` into `result_rows` respecting `distinct`, `skip_count`, and optional `limit`.
    /// Returns `true` if `limit` has now been reached and callers may stop scanning early.
    pub(crate) fn push_paginated(
        &mut self,
        result_rows: &mut Vec<Row>,
        candidate: Row,
        skip_count: usize,
        limit: Option<usize>,
    ) -> bool {
        let skipped_unique = &self.skipped_unique;
        let total_seen = skipped_unique.len() + result_rows.len();
        let h = hash_values(candidate.values());
        let is_new = match self.buckets.get_mut(&h) {
            None => {
                self.buckets.insert(h, IntEntry::One(total_seen));
                true
            }
            Some(entry) => match entry {
                IntEntry::One(idx) => {
                    if Self::lookup_row(skipped_unique, result_rows, *idx) == &candidate {
                        false
                    } else {
                        let prev = *idx;
                        *entry = IntEntry::Many(vec![prev, total_seen]);
                        true
                    }
                }
                IntEntry::Many(indices) => {
                    if indices.iter().any(|&idx| {
                        Self::lookup_row(skipped_unique, result_rows, idx) == &candidate
                    }) {
                        false
                    } else {
                        indices.push(total_seen);
                        true
                    }
                }
            },
        };

        if !is_new {
            return false;
        }
        if self.skipped_unique.len() < skip_count {
            self.skipped_unique.push(candidate);
            return false;
        }
        result_rows.push(candidate);
        limit.is_some_and(|lim| result_rows.len() >= lim)
    }
}

/// Extract conservative signed integer bounds `(min_i64, max_i64, has_bound)` for an INTEGER PRIMARY KEY
/// from top-level `AND` conjunctions in a WHERE expression.
pub(crate) fn extract_pk_signed_range(
    expr: &WhereExpr,
    pk_col_name: &str,
) -> (Option<i64>, Option<i64>, bool) {
    fn visit(
        expr: &WhereExpr,
        pk_col_name: &str,
        min_b: &mut Option<i64>,
        max_b: &mut Option<i64>,
        found: &mut bool,
        impossible: &mut bool,
    ) {
        match expr {
            WhereExpr::And(l, r) => {
                visit(l, pk_col_name, min_b, max_b, found, impossible);
                visit(r, pk_col_name, min_b, max_b, found, impossible);
            }
            WhereExpr::Condition(c) if c.column.eq_ignore_ascii_case(pk_col_name) => {
                if let Value::Integer(k) = c.value {
                    match c.op {
                        BinaryOp::Equals => {
                            *found = true;
                            *min_b = Some(min_b.map_or(k, |cur| cur.max(k)));
                            *max_b = Some(max_b.map_or(k, |cur| cur.min(k)));
                        }
                        BinaryOp::GreaterOrEqual => {
                            *found = true;
                            *min_b = Some(min_b.map_or(k, |cur| cur.max(k)));
                        }
                        BinaryOp::GreaterThan => {
                            *found = true;
                            if let Some(next_k) = k.checked_add(1) {
                                *min_b = Some(min_b.map_or(next_k, |cur| cur.max(next_k)));
                            } else {
                                *impossible = true;
                            }
                        }
                        BinaryOp::LessOrEqual => {
                            *found = true;
                            *max_b = Some(max_b.map_or(k, |cur| cur.min(k)));
                        }
                        BinaryOp::LessThan => {
                            *found = true;
                            if let Some(prev_k) = k.checked_sub(1) {
                                *max_b = Some(max_b.map_or(prev_k, |cur| cur.min(prev_k)));
                            } else {
                                *impossible = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    let mut min_b = None;
    let mut max_b = None;
    let mut found = false;
    let mut impossible = false;
    visit(
        expr,
        pk_col_name,
        &mut min_b,
        &mut max_b,
        &mut found,
        &mut impossible,
    );
    if impossible {
        (Some(1), Some(0), true)
    } else {
        (min_b, max_b, found)
    }
}

/// Resolve a simple column or `table.column` reference (no JSON path, UDF, or expression)
/// to its index in `table_def`.
#[inline]
pub(crate) fn resolve_direct_table_col(table_def: &TableDef, col: &str) -> Option<usize> {
    let trimmed = col.trim();
    if trimmed.contains("->") || trimmed.contains('(') || trimmed.contains(' ') {
        return None;
    }
    if let Some((tbl, c)) = trimmed.split_once('.') {
        if tbl.eq_ignore_ascii_case(&table_def.name) {
            table_def.column_index(c)
        } else {
            None
        }
    } else {
        table_def.column_index(trimmed)
    }
}

/// Populate `mask` with column indices referenced by `expr`.
/// Returns `false` if any predicate requires a full-row evaluation (e.g., JSON path, UDF, or scalar expr).
pub(crate) fn mark_resolved_where_columns(
    expr: &ResolvedWhereExpr,
    table_def: &TableDef,
    mask: &mut [bool],
) -> bool {
    match expr {
        ResolvedWhereExpr::Boolean(_) => true,
        ResolvedWhereExpr::And(l, r) | ResolvedWhereExpr::Or(l, r) => {
            mark_resolved_where_columns(l, table_def, mask)
                && mark_resolved_where_columns(r, table_def, mask)
        }
        ResolvedWhereExpr::Condition(cond) => {
            if let Some(idx) = resolve_direct_table_col(table_def, &cond.column)
                && idx < mask.len()
            {
                mask[idx] = true;
                true
            } else {
                false
            }
        }
        ResolvedWhereExpr::InList { column, .. } | ResolvedWhereExpr::InSet { column, .. } => {
            if let Some(idx) = resolve_direct_table_col(table_def, column)
                && idx < mask.len()
            {
                mask[idx] = true;
                true
            } else {
                false
            }
        }
    }
}
