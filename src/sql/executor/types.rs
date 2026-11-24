use super::eval::{matches_condition, try_eval_scalar_expr};
use crate::btree::BTreeStorage;
use crate::error::{Error, Result};
use crate::pager::Pager;
use crate::sql::catalog::{Catalog, IndexDef, TableDef, TableForeignKey};
use crate::sql::codec::encode_row_into;
use crate::sql::lexer::Token;
use crate::sql::parser::{BinaryOp, JoinClause, JoinType, Statement, WhereCondition, WhereExpr};
use crate::traits::{HashableValue, Row, Value};
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

    /// Number of distinct candidate values stored in this lookup table.
    #[inline]
    pub fn distinct_len(&self) -> usize {
        self.ints.len() + self.reals.len() + self.texts.len() + self.others.len()
    }

    /// Extract all distinct `i64` keys (including exact-integer `Real`s), sorted ascending.
    pub fn collect_sorted_int_keys(&self) -> Vec<i64> {
        let mut keys: Vec<i64> = self.ints.iter().copied().collect();
        for &bits in &self.reals {
            let r = f64::from_bits(bits);
            if r.fract() == 0.0 && r >= (i64::MIN as f64) && r < 9223372036854775808.0 {
                let cand = r as i64;
                if (cand as f64) == r && !self.ints.contains(&cand) {
                    keys.push(cand);
                }
            }
        }
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    /// Extract all deduplicated non-NULL candidate values for index probing.
    pub fn collect_probe_values(&self) -> Vec<Value> {
        let mut vals = Vec::with_capacity(self.distinct_len());
        for &i in &self.ints {
            vals.push(Value::Integer(i));
        }
        for &bits in &self.reals {
            vals.push(Value::Real(f64::from_bits(bits)));
        }
        for s in &self.texts {
            vals.push(Value::Text(s.clone()));
        }
        for o in &self.others {
            if !vals.iter().any(|v| v == o) {
                vals.push(o.clone());
            }
        }
        vals
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
    /// Column-to-column comparison (e.g. `price > cost`)
    ColumnComparison {
        /// Left-hand column name
        left_col: String,
        /// Comparison operator
        op: BinaryOp,
        /// Right-hand column name
        right_col: String,
    },
    /// Correlated `[NOT] EXISTS (SELECT ...)` evaluated per outer row with memoization
    CorrelatedExists {
        /// Inner SELECT subquery referencing outer columns
        subquery: Box<Statement>,
        /// Whether negated (`NOT EXISTS`)
        negated: bool,
        /// Deduplicated outer column names referenced by the subquery
        outer_refs: Vec<String>,
    },
    /// Correlated `col [NOT] IN (SELECT ...)` evaluated per outer row with memoization
    CorrelatedInSubquery {
        /// Target LHS column name in outer row
        column: String,
        /// Inner SELECT subquery referencing outer columns
        subquery: Box<Statement>,
        /// Whether negated (`NOT IN`)
        negated: bool,
        /// Deduplicated outer column names referenced by the subquery
        outer_refs: Vec<String>,
    },
    /// Correlated scalar subquery comparison:
    /// - `column: Some(col), literal: None` represents `col <op> (SELECT ...)`
    /// - `column: None, literal: Some(val)` represents `(SELECT ...) <op> val`
    CorrelatedScalarSubquery {
        /// Optional LHS column name when form is `col <op> (SELECT ...)`
        column: Option<String>,
        /// Comparison operator
        op: BinaryOp,
        /// Inner SELECT subquery referencing outer columns
        subquery: Box<Statement>,
        /// Optional RHS literal value when form is `(SELECT ...) <op> val`
        literal: Option<Value>,
        /// Deduplicated outer column names referenced by the subquery
        outer_refs: Vec<String>,
    },
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

#[inline]
fn eval_index_key_component(row: &Row, target: &str) -> Option<Value> {
    if let Some(val) = row.get_field_or_json_path(target) {
        return Some(val);
    }
    super::eval::eval_projected_expr(row, target, target, None).ok()
}

/// Normalize an index target expression (single column or scalar expression) for index-planner matching.
pub(crate) fn normalize_index_expr(expr: &str, table: Option<&str>) -> String {
    let mut s = super::eval::strip_outer_parens(expr).trim().to_string();
    if let Some(tbl) = table {
        let prefix = format!("{}.", tbl.to_ascii_lowercase());
        if let Ok(toks) = crate::sql::lexer::tokenize(&s) {
            let mut stripped = Vec::with_capacity(toks.len());
            let mut i = 0usize;
            while i < toks.len() {
                if let crate::sql::lexer::Token::Ident(ref id) = toks[i]
                    && toks.get(i + 1) == Some(&crate::sql::lexer::Token::Dot)
                    && id.eq_ignore_ascii_case(tbl)
                {
                    i += 2;
                    continue;
                }
                stripped.push(toks[i].clone());
                i += 1;
            }
            let _ = prefix;
            s = crate::sql::lexer::tokens_to_sql(&stripped);
        }
    }
    s.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Extract a composite, expression, or single-column index value from a row.
/// Returns `None` if all indexed components evaluate to `NULL` (SQL indexes omit all-NULL tuples).
/// The boolean indicates whether ALL components in the key are non-NULL (for SQL UNIQUE checks).
pub(crate) fn extract_index_key_value<S: AsRef<str>>(
    row: &Row,
    cols: &[S],
) -> Option<(Value, bool)> {
    if cols.is_empty() {
        return None;
    }
    if cols.len() == 1 {
        let val = eval_index_key_component(row, cols[0].as_ref())?;
        if val.is_null() {
            return None;
        }
        return Some((val, true));
    }
    let mut vals = Vec::with_capacity(cols.len());
    let mut any_non_null = false;
    let mut all_non_null = true;
    for col in cols {
        let v = eval_index_key_component(row, col.as_ref()).unwrap_or(Value::Null);
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
        WhereExpr::InSubquery { .. }
        | WhereExpr::Exists { .. }
        | WhereExpr::ScalarSubqueryComparison { .. }
        | WhereExpr::SubqueryComparison { .. } => Some(true),
        WhereExpr::ColumnComparison {
            left_col,
            op,
            right_col,
        } => {
            let l_val = match row.get_field_or_json_path(left_col) {
                Some(v) => v,
                None => match try_eval_scalar_expr(row, left_col, udfs) {
                    Ok(Some(v)) => v,
                    _ => return None,
                },
            };
            let r_val = match row.get_field_or_json_path(right_col) {
                Some(v) => v,
                None => match try_eval_scalar_expr(row, right_col, udfs) {
                    Ok(Some(v)) => v,
                    _ => return None,
                },
            };
            if l_val.is_null() || r_val.is_null() {
                None
            } else {
                Some(matches_condition(&l_val, op, &r_val))
            }
        }
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
    let row_key = |r: &Row| format!("{:?}", r.values());
    match op {
        crate::sql::parser::SetOperation::UnionAll => left.extend(right),
        crate::sql::parser::SetOperation::Union => {
            let mut seen = HashSet::with_capacity(left.len() + right.len());
            left.retain(|r| seen.insert(row_key(r)));
            for row in right {
                if seen.insert(row_key(&row)) {
                    left.push(row);
                }
            }
        }
        crate::sql::parser::SetOperation::Intersect => {
            let right_set: HashSet<String> = right.iter().map(row_key).collect();
            let mut seen = HashSet::new();
            left.retain(|r| {
                let k = row_key(r);
                right_set.contains(&k) && seen.insert(k)
            });
        }
        crate::sql::parser::SetOperation::Except => {
            let right_set: HashSet<String> = right.iter().map(row_key).collect();
            let mut seen = HashSet::new();
            left.retain(|r| {
                let k = row_key(r);
                !right_set.contains(&k) && seen.insert(k)
            });
        }
    }
}

pub(crate) fn finalize_compound_rows(
    rows: &mut Vec<Row>,
    order_by: &[crate::sql::parser::OrderByClause],
    limit: Option<usize>,
    offset: Option<usize>,
) {
    if !order_by.is_empty() {
        rows.sort_by(|a, b| super::eval::compare_rows_by_order(a, b, order_by));
    }
    if let Some(off) = offset {
        if off < rows.len() {
            *rows = rows.split_off(off);
        } else {
            rows.clear();
        }
    }
    if let Some(lim) = limit {
        rows.truncate(lim);
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
        ResolvedWhereExpr::ColumnComparison {
            left_col,
            right_col,
            ..
        } => {
            if let (Some(l_idx), Some(r_idx)) = (
                resolve_direct_table_col(table_def, left_col),
                resolve_direct_table_col(table_def, right_col),
            ) && l_idx < mask.len()
                && r_idx < mask.len()
            {
                mask[l_idx] = true;
                mask[r_idx] = true;
                true
            } else {
                false
            }
        }
        ResolvedWhereExpr::CorrelatedExists { outer_refs, .. } => {
            for r in outer_refs {
                let Some(idx) = resolve_direct_table_col(table_def, r) else {
                    return false;
                };
                if idx < mask.len() {
                    mask[idx] = true;
                } else {
                    return false;
                }
            }
            true
        }
        ResolvedWhereExpr::CorrelatedInSubquery {
            column, outer_refs, ..
        } => {
            let Some(c_idx) = resolve_direct_table_col(table_def, column) else {
                return false;
            };
            if c_idx >= mask.len() {
                return false;
            }
            mask[c_idx] = true;
            for r in outer_refs {
                let Some(idx) = resolve_direct_table_col(table_def, r) else {
                    return false;
                };
                if idx < mask.len() {
                    mask[idx] = true;
                } else {
                    return false;
                }
            }
            true
        }
        ResolvedWhereExpr::CorrelatedScalarSubquery {
            column, outer_refs, ..
        } => {
            if let Some(col) = column {
                let Some(c_idx) = resolve_direct_table_col(table_def, col) else {
                    return false;
                };
                if c_idx >= mask.len() {
                    return false;
                }
                mask[c_idx] = true;
            }
            for r in outer_refs {
                let Some(idx) = resolve_direct_table_col(table_def, r) else {
                    return false;
                };
                if idx < mask.len() {
                    mask[idx] = true;
                } else {
                    return false;
                }
            }
            true
        }
    }
}

/// Returns `true` if `expr` contains any correlated subquery node requiring per-row execution.
pub(crate) fn resolved_where_has_correlated(expr: &ResolvedWhereExpr) -> bool {
    match expr {
        ResolvedWhereExpr::CorrelatedExists { .. }
        | ResolvedWhereExpr::CorrelatedInSubquery { .. }
        | ResolvedWhereExpr::CorrelatedScalarSubquery { .. } => true,
        ResolvedWhereExpr::And(l, r) | ResolvedWhereExpr::Or(l, r) => {
            resolved_where_has_correlated(l) || resolved_where_has_correlated(r)
        }
        ResolvedWhereExpr::Boolean(_)
        | ResolvedWhereExpr::Condition(_)
        | ResolvedWhereExpr::InList { .. }
        | ResolvedWhereExpr::InSet { .. }
        | ResolvedWhereExpr::ColumnComparison { .. } => false,
    }
}

/// Split top-level `AND` conjuncts into `(non_correlated_filter, correlated_filter)`.
pub(crate) fn split_correlated_and_predicates(
    expr: &ResolvedWhereExpr,
) -> (Option<ResolvedWhereExpr>, Option<ResolvedWhereExpr>) {
    let mut conjuncts = Vec::new();
    collect_and_conjuncts(expr, &mut conjuncts);
    let mut non_corr = Vec::new();
    let mut corr = Vec::new();
    for c in conjuncts {
        if resolved_where_has_correlated(c) {
            corr.push(c.clone());
        } else {
            non_corr.push(c.clone());
        }
    }
    (combine_and(non_corr), combine_and(corr))
}

/// Collect top-level `AND`-conjunctive positive `InSet` expressions.
pub(crate) fn extract_top_level_in_sets<'a>(
    expr: &'a ResolvedWhereExpr,
    out: &mut Vec<(&'a str, &'a InValueLookup)>,
) {
    match expr {
        ResolvedWhereExpr::And(l, r) => {
            extract_top_level_in_sets(l, out);
            extract_top_level_in_sets(r, out);
        }
        ResolvedWhereExpr::InSet {
            column,
            lookup,
            negated: false,
            ..
        } => {
            out.push((column.as_str(), lookup.as_ref()));
        }
        _ => {}
    }
}

fn collect_and_conjuncts<'a>(expr: &'a ResolvedWhereExpr, out: &mut Vec<&'a ResolvedWhereExpr>) {
    if let ResolvedWhereExpr::And(l, r) = expr {
        collect_and_conjuncts(l, out);
        collect_and_conjuncts(r, out);
    } else {
        out.push(expr);
    }
}

fn join_predicate_table_owner(expr: &ResolvedWhereExpr, tables: &[&TableDef]) -> Option<usize> {
    match expr {
        ResolvedWhereExpr::Boolean(_) => Some(0),
        ResolvedWhereExpr::And(l, r) | ResolvedWhereExpr::Or(l, r) => {
            let lo = join_predicate_table_owner(l, tables)?;
            let ro = join_predicate_table_owner(r, tables)?;
            (lo == ro).then_some(lo)
        }
        ResolvedWhereExpr::Condition(cond) => column_owner_in_tables(&cond.column, tables),
        ResolvedWhereExpr::InList { column, .. } | ResolvedWhereExpr::InSet { column, .. } => {
            column_owner_in_tables(column, tables)
        }
        ResolvedWhereExpr::ColumnComparison {
            left_col,
            right_col,
            ..
        } => {
            let lo = column_owner_in_tables(left_col, tables)?;
            let ro = column_owner_in_tables(right_col, tables)?;
            (lo == ro).then_some(lo)
        }
        ResolvedWhereExpr::CorrelatedExists { .. }
        | ResolvedWhereExpr::CorrelatedInSubquery { .. }
        | ResolvedWhereExpr::CorrelatedScalarSubquery { .. } => None,
    }
}

fn column_owner_in_tables(col: &str, tables: &[&TableDef]) -> Option<usize> {
    let trimmed = col.trim();
    if trimmed.contains("->") || trimmed.contains('(') || trimmed.contains(' ') {
        return None;
    }
    if let Some((tbl, c)) = trimmed.split_once('.') {
        let mut owner = None;
        for (idx, tdef) in tables.iter().enumerate() {
            if tdef.name.eq_ignore_ascii_case(tbl) && tdef.column_index(c).is_some() {
                if owner.is_some() {
                    return None;
                }
                owner = Some(idx);
            }
        }
        owner
    } else {
        let mut owner = None;
        for (idx, tdef) in tables.iter().enumerate() {
            if tdef.column_index(trimmed).is_some() {
                if owner.is_some() {
                    return None;
                }
                owner = Some(idx);
            }
        }
        owner
    }
}

fn combine_and(items: Vec<ResolvedWhereExpr>) -> Option<ResolvedWhereExpr> {
    let mut iter = items.into_iter();
    let mut acc = iter.next()?;
    for next in iter {
        acc = ResolvedWhereExpr::And(Box::new(acc), Box::new(next));
    }
    Some(acc)
}

/// Partition a resolved `WHERE` predicate into per-table pushdowns and a post-join residual filter.
///
/// Only pushes left-table filters when no `RIGHT`/`FULL` outer join can manufacture left `NULL`s,
/// and only pushes right-table `k` filters when every join up to `k` is `INNER`.
pub(crate) fn split_join_where_predicates(
    expr: &ResolvedWhereExpr,
    tables: &[&TableDef],
    joins: &[&JoinClause],
) -> (
    Option<ResolvedWhereExpr>,
    Vec<Option<ResolvedWhereExpr>>,
    Option<ResolvedWhereExpr>,
) {
    let mut right_pushed = vec![Vec::new(); joins.len()];
    let has_right_or_full = joins
        .iter()
        .any(|j| j.join_type == JoinType::Right || j.join_type == JoinType::Full);
    if has_right_or_full {
        return (None, vec![None; joins.len()], Some(expr.clone()));
    }

    let mut conjuncts = Vec::new();
    collect_and_conjuncts(expr, &mut conjuncts);
    let mut left_items = Vec::new();
    let mut post_items = Vec::new();

    for c in conjuncts {
        match join_predicate_table_owner(c, tables) {
            Some(0) => left_items.push(c.clone()),
            Some(owner_idx) if owner_idx > 0 => {
                let join_pos = owner_idx - 1;
                let all_inner_prefix = joins[..=join_pos].iter().all(|j| {
                    (j.join_type == JoinType::Inner && !j.is_left) || j.join_type == JoinType::Cross
                });
                if all_inner_prefix {
                    right_pushed[join_pos].push(c.clone());
                } else {
                    post_items.push(c.clone());
                }
            }
            _ => post_items.push(c.clone()),
        }
    }

    (
        combine_and(left_items),
        right_pushed.into_iter().map(combine_and).collect(),
        combine_and(post_items),
    )
}

/// Cache-line 512-bit (`[u64; 8]`) 8-lane SIMD Block Bloom Filter for join-key pushdown.
///
/// Maps each key hash into a single 64-byte block and tests/sets 8 bit positions across
/// the 8 `u64` lanes in parallel, pruning non-matching probe rows in L1 cache before
/// touching the hash table.
#[derive(Debug, Clone)]
pub struct SimdJoinBloomFilter {
    blocks: Vec<[u64; 8]>,
    item_count: usize,
}

impl SimdJoinBloomFilter {
    /// Allocate a SIMD Block Bloom Filter sized for `expected_items`.
    pub fn with_capacity(expected_items: usize) -> Self {
        let min_blocks = ((expected_items.max(16) * 12) / 512).max(1);
        let num_blocks = min_blocks.next_power_of_two();
        Self {
            blocks: vec![[0u64; 8]; num_blocks],
            item_count: 0,
        }
    }

    #[inline]
    fn mix_u64(mut z: u64) -> u64 {
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    #[inline]
    fn lane_masks(hash: u64) -> [u64; 8] {
        const SALT: [u64; 8] = [
            0x47B6_137B_9E37_79B9,
            0x8823_D2F3_BF58_476D,
            0x71D3_626B_94D0_49BB,
            0x2F6E_2B15_D1B5_4A32,
            0xA5A5_5A5A_C6BC_2796,
            0x517C_C1B7_2722_0A95,
            0x9E37_79B9_7F4A_7C15,
            0xD6E8_FEB8_6659_FD93,
        ];
        let mut masks = [0u64; 8];
        for i in 0..8 {
            let bit = (hash.wrapping_mul(SALT[i]) >> 58) & 63;
            masks[i] = 1u64 << bit;
        }
        masks
    }

    /// Insert a signed 64-bit integer join key.
    #[inline]
    pub fn insert_i64(&mut self, key: i64) {
        self.insert_hash(Self::mix_u64(key as u64));
    }

    /// Insert a `HashableValue` join key.
    #[inline]
    pub fn insert_value(&mut self, val: &HashableValue) {
        use std::hash::{Hash, Hasher};
        let mut hasher = FastIntHasher::default();
        val.hash(&mut hasher);
        self.insert_hash(Self::mix_u64(hasher.finish()));
    }

    /// Insert a 64-bit hash into a single 64-byte SIMD block.
    #[inline]
    pub fn insert_hash(&mut self, hash: u64) {
        let block_idx = ((hash >> 32) as usize) & (self.blocks.len() - 1);
        let masks = Self::lane_masks(hash);
        let blk = &mut self.blocks[block_idx];
        for i in 0..8 {
            blk[i] |= masks[i];
        }
        self.item_count += 1;
    }

    /// Test whether integer `key` might exist in the build relation.
    #[inline]
    pub fn might_contain_i64(&self, key: i64) -> bool {
        self.might_contain_hash(Self::mix_u64(key as u64))
    }

    /// Test whether `HashableValue` might exist in the build relation.
    #[inline]
    pub fn might_contain_value(&self, val: &HashableValue) -> bool {
        use std::hash::{Hash, Hasher};
        let mut hasher = FastIntHasher::default();
        val.hash(&mut hasher);
        self.might_contain_hash(Self::mix_u64(hasher.finish()))
    }

    /// Test a 64-bit hash across all 8 `u64` SIMD block lanes.
    #[inline]
    pub fn might_contain_hash(&self, hash: u64) -> bool {
        let block_idx = ((hash >> 32) as usize) & (self.blocks.len() - 1);
        let masks = Self::lane_masks(hash);
        let blk = &self.blocks[block_idx];
        let mut missed = 0u64;
        for i in 0..8 {
            missed |= (blk[i] & masks[i]) ^ masks[i];
        }
        missed == 0
    }
}

/// Cost-Based Multi-Join Reordering planner (`Dynamic Programming / Greedy Cardinality Ordering`).
///
/// Given a base left table and a chain of `JoinClause`s along with their estimated post-filter
/// cardinalities, returns the optimal execution order indices `(0..joins.len())` that minimizes
/// intermediate hash-join cardinalities while respecting table-reference dependencies and
/// preserving outer join ordering.
pub fn plan_cbo_inner_join_order(
    base_table: &str,
    joins: &[&JoinClause],
    estimated_right_rows: &[usize],
) -> Vec<usize> {
    let n = joins.len();
    if n <= 1 {
        return (0..n).collect();
    }
    let all_inner = joins
        .iter()
        .all(|j| j.join_type == JoinType::Inner && !j.is_left);
    if !all_inner {
        return (0..n).collect();
    }

    let mut available_tables: Vec<String> = vec![base_table.to_ascii_lowercase()];
    let mut remaining: Vec<usize> = (0..n).collect();
    let mut order = Vec::with_capacity(n);

    while !remaining.is_empty() {
        // Pick the lowest-cardinality join whose left_col table prefix (if qualified) is already in `available_tables`
        let mut best_pos = 0usize;
        let mut best_card = usize::MAX;
        let mut found_ready = false;

        for (pos, &j_idx) in remaining.iter().enumerate() {
            let jc = joins[j_idx];
            let left_prefix = jc
                .left_col
                .split_once('.')
                .map(|(t, _)| t.to_ascii_lowercase());
            let is_ready = match left_prefix {
                Some(ref prefix) => available_tables.iter().any(|t| t == prefix),
                None => true,
            };
            let card = estimated_right_rows.get(j_idx).copied().unwrap_or(1_000);
            if is_ready && (!found_ready || card < best_card) {
                found_ready = true;
                best_card = card;
                best_pos = pos;
            }
        }

        let chosen_idx = remaining.remove(best_pos);
        available_tables.push(joins[chosen_idx].table.to_ascii_lowercase());
        order.push(chosen_idx);
    }

    order
}

#[derive(Default)]
pub(crate) struct CorrelatedSubqueryMemo {
    pub exists_cache: HashMap<(usize, Vec<HashableValue>), bool>,
    pub in_cache: HashMap<(usize, Vec<HashableValue>), Arc<InValueLookup>>,
    pub scalar_cache: HashMap<(usize, Vec<HashableValue>), Value>,
}

#[derive(Clone)]
pub(crate) enum PreparedProjPiece {
    Raw(String),
    Uncorrelated(Value),
    Correlated {
        stmt: Box<Statement>,
        outer_refs: Vec<String>,
    },
}

pub(crate) struct PreparedProjColumn {
    pub(crate) raw_col: String,
    pub(crate) expr: String,
    pub(crate) out_name: String,
    pub(crate) pieces: Vec<PreparedProjPiece>,
    pub(crate) is_pure_subquery: bool,
}

#[inline]
pub(crate) fn has_projection_subquery(columns: &[String]) -> bool {
    columns
        .iter()
        .any(|c| c.contains('(') && c.to_ascii_uppercase().contains("SELECT"))
}

#[inline]
pub(crate) fn inject_scalar_limit(sub: &mut Statement) {
    if let Statement::Select { limit, offset, .. } = sub
        && limit.is_none()
        && offset.is_none()
    {
        *limit = Some(2);
    }
}

#[inline]
pub(crate) fn inject_exists_limit(sub: &mut Statement) {
    if let Statement::Select {
        distinct,
        order_by,
        limit,
        offset,
        ..
    } = sub
        && limit.is_none()
    {
        *limit = Some(1);
        if offset.is_none() {
            order_by.clear();
            *distinct = false;
        }
    }
}

pub(crate) fn build_outer_ref_key(row: &Row, outer_refs: &[String]) -> Vec<HashableValue> {
    outer_refs
        .iter()
        .map(|col| HashableValue::from(&row.get_field_or_json_path(col).unwrap_or(Value::Null)))
        .collect()
}

fn push_unique_outer_ref(out: &mut Vec<String>, name: &str) {
    if !out
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(name))
    {
        out.push(name.to_string());
    }
}

fn collect_inner_tables<'a>(
    stmt: &Statement,
    catalog: &'a Catalog,
) -> (Vec<&'a TableDef>, Vec<String>, bool) {
    let mut defs = Vec::new();
    let mut names = Vec::new();
    let mut all_known = true;
    if let Statement::Select {
        table,
        join,
        extra_joins,
        ..
    } = stmt
    {
        names.push(table.clone());
        if let Some(td) = catalog.get_table(table) {
            defs.push(td);
        } else {
            all_known = false;
        }
        for j in join.iter().chain(extra_joins.iter()) {
            names.push(j.table.clone());
            if let Some(td) = catalog.get_table(&j.table) {
                defs.push(td);
            } else {
                all_known = false;
            }
        }
    }
    (defs, names, all_known)
}

fn lookup_outer_col(
    col: &str,
    inner_tables: &[&TableDef],
    inner_names: &[String],
    inner_all_known: bool,
    outer_tables: &[&TableDef],
) -> Option<String> {
    let trimmed = col.trim();
    if trimmed.is_empty() || trimmed.contains('(') || trimmed.contains(' ') {
        return None;
    }
    if let Some((tbl, c)) = trimmed.split_once('.') {
        if inner_names.iter().any(|n| n.eq_ignore_ascii_case(tbl)) {
            return None;
        }
        if outer_tables
            .iter()
            .any(|t| t.name.eq_ignore_ascii_case(tbl) && t.column_index(c).is_some())
        {
            return Some(trimmed.to_string());
        }
        None
    } else {
        if !inner_all_known {
            return None;
        }
        if inner_tables
            .iter()
            .any(|t| t.column_index(trimmed).is_some())
        {
            return None;
        }
        if outer_tables
            .iter()
            .any(|t| t.column_index(trimmed).is_some())
        {
            return Some(trimmed.to_string());
        }
        None
    }
}

fn collect_where_outer_refs(
    expr: &WhereExpr,
    inner_tables: &[&TableDef],
    inner_names: &[String],
    inner_all_known: bool,
    outer_tables: &[&TableDef],
    catalog: &Catalog,
    out: &mut Vec<String>,
) {
    match expr {
        WhereExpr::Condition(c) => {
            if let Some(r) = lookup_outer_col(
                &c.column,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
            ) {
                push_unique_outer_ref(out, &r);
            }
        }
        WhereExpr::ColumnComparison {
            left_col,
            right_col,
            ..
        } => {
            if let Some(r) = lookup_outer_col(
                left_col,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
            ) {
                push_unique_outer_ref(out, &r);
            }
            if let Some(r) = lookup_outer_col(
                right_col,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
            ) {
                push_unique_outer_ref(out, &r);
            }
        }
        WhereExpr::InList { column, .. } => {
            if let Some(r) = lookup_outer_col(
                column,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
            ) {
                push_unique_outer_ref(out, &r);
            }
        }
        WhereExpr::And(l, r) | WhereExpr::Or(l, r) => {
            collect_where_outer_refs(
                l,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
                catalog,
                out,
            );
            collect_where_outer_refs(
                r,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
                catalog,
                out,
            );
        }
        WhereExpr::InSubquery {
            column, subquery, ..
        } => {
            if let Some(r) = lookup_outer_col(
                column,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
            ) {
                push_unique_outer_ref(out, &r);
            }
            collect_statement_outer_refs(subquery, outer_tables, catalog, out);
        }
        WhereExpr::Exists { subquery, .. } | WhereExpr::SubqueryComparison { subquery, .. } => {
            collect_statement_outer_refs(subquery, outer_tables, catalog, out);
        }
        WhereExpr::ScalarSubqueryComparison {
            column, subquery, ..
        } => {
            if let Some(r) = lookup_outer_col(
                column,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
            ) {
                push_unique_outer_ref(out, &r);
            }
            collect_statement_outer_refs(subquery, outer_tables, catalog, out);
        }
    }
}

pub(crate) fn collect_statement_outer_refs(
    stmt: &Statement,
    outer_tables: &[&TableDef],
    catalog: &Catalog,
    out: &mut Vec<String>,
) {
    match stmt {
        Statement::Select {
            where_clause,
            having,
            ..
        } => {
            let (inner_tables, inner_names, inner_all_known) = collect_inner_tables(stmt, catalog);
            if let Some(w) = where_clause {
                collect_where_outer_refs(
                    w,
                    &inner_tables,
                    &inner_names,
                    inner_all_known,
                    outer_tables,
                    catalog,
                    out,
                );
            }
            if let Some(h) = having {
                collect_where_outer_refs(
                    h,
                    &inner_tables,
                    &inner_names,
                    inner_all_known,
                    outer_tables,
                    catalog,
                    out,
                );
            }
        }
        Statement::Compound { left, right, .. } => {
            collect_statement_outer_refs(left, outer_tables, catalog, out);
            collect_statement_outer_refs(right, outer_tables, catalog, out);
        }
        _ => {}
    }
}

enum SubstWhere {
    Expr(WhereExpr),
    Always(bool),
}

fn flip_comparison_op(op: &BinaryOp) -> Option<BinaryOp> {
    Some(match op {
        BinaryOp::Equals => BinaryOp::Equals,
        BinaryOp::NotEquals => BinaryOp::NotEquals,
        BinaryOp::GreaterThan => BinaryOp::LessThan,
        BinaryOp::LessThan => BinaryOp::GreaterThan,
        BinaryOp::GreaterOrEqual => BinaryOp::LessOrEqual,
        BinaryOp::LessOrEqual => BinaryOp::GreaterOrEqual,
        BinaryOp::Like
        | BinaryOp::NotLike
        | BinaryOp::Match
        | BinaryOp::IsNull
        | BinaryOp::IsNotNull => {
            return None;
        }
    })
}

fn substitute_outer_where(
    expr: &WhereExpr,
    outer_row: &Row,
    inner_tables: &[&TableDef],
    inner_names: &[String],
    inner_all_known: bool,
    outer_tables: &[&TableDef],
    catalog: &Catalog,
) -> SubstWhere {
    match expr {
        WhereExpr::Condition(c) => {
            if let Some(outer_col) = lookup_outer_col(
                &c.column,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
            ) {
                let val = outer_row
                    .get_field_or_json_path(&outer_col)
                    .unwrap_or(Value::Null);
                SubstWhere::Always(matches_condition(&val, &c.op, &c.value))
            } else {
                SubstWhere::Expr(WhereExpr::Condition(c.clone()))
            }
        }
        WhereExpr::ColumnComparison {
            left_col,
            op,
            right_col,
        } => {
            let l_outer = lookup_outer_col(
                left_col,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
            );
            let r_outer = lookup_outer_col(
                right_col,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
            );
            match (l_outer, r_outer) {
                (Some(lc), Some(rc)) => {
                    let lv = outer_row.get_field_or_json_path(&lc).unwrap_or(Value::Null);
                    let rv = outer_row.get_field_or_json_path(&rc).unwrap_or(Value::Null);
                    SubstWhere::Always(matches_condition(&lv, op, &rv))
                }
                (None, Some(rc)) => {
                    let rv = outer_row.get_field_or_json_path(&rc).unwrap_or(Value::Null);
                    SubstWhere::Expr(WhereExpr::Condition(WhereCondition {
                        column: left_col.clone(),
                        op: op.clone(),
                        value: rv,
                    }))
                }
                (Some(lc), None) => {
                    let lv = outer_row.get_field_or_json_path(&lc).unwrap_or(Value::Null);
                    if let Some(flipped) = flip_comparison_op(op) {
                        SubstWhere::Expr(WhereExpr::Condition(WhereCondition {
                            column: right_col.clone(),
                            op: flipped,
                            value: lv,
                        }))
                    } else {
                        SubstWhere::Always(false)
                    }
                }
                (None, None) => SubstWhere::Expr(WhereExpr::ColumnComparison {
                    left_col: left_col.clone(),
                    op: op.clone(),
                    right_col: right_col.clone(),
                }),
            }
        }
        WhereExpr::InList {
            column,
            values,
            negated,
        } => {
            if let Some(outer_col) = lookup_outer_col(
                column,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
            ) {
                let val = outer_row
                    .get_field_or_json_path(&outer_col)
                    .unwrap_or(Value::Null);
                if val.is_null() {
                    return SubstWhere::Always(false);
                }
                let has_null = values.iter().any(|v| v.is_null());
                let in_list = values.iter().any(|target| val.compare(target).is_eq());
                let matched = if *negated {
                    !has_null && !in_list
                } else {
                    in_list
                };
                SubstWhere::Always(matched)
            } else {
                SubstWhere::Expr(WhereExpr::InList {
                    column: column.clone(),
                    values: values.clone(),
                    negated: *negated,
                })
            }
        }
        WhereExpr::And(l, r) => {
            let ls = substitute_outer_where(
                l,
                outer_row,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
                catalog,
            );
            if matches!(ls, SubstWhere::Always(false)) {
                return SubstWhere::Always(false);
            }
            let rs = substitute_outer_where(
                r,
                outer_row,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
                catalog,
            );
            match (ls, rs) {
                (SubstWhere::Always(false), _) | (_, SubstWhere::Always(false)) => {
                    SubstWhere::Always(false)
                }
                (SubstWhere::Always(true), other) | (other, SubstWhere::Always(true)) => other,
                (SubstWhere::Expr(le), SubstWhere::Expr(re)) => {
                    SubstWhere::Expr(WhereExpr::And(Box::new(le), Box::new(re)))
                }
            }
        }
        WhereExpr::Or(l, r) => {
            let ls = substitute_outer_where(
                l,
                outer_row,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
                catalog,
            );
            if matches!(ls, SubstWhere::Always(true)) {
                return SubstWhere::Always(true);
            }
            let rs = substitute_outer_where(
                r,
                outer_row,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
                catalog,
            );
            match (ls, rs) {
                (SubstWhere::Always(true), _) | (_, SubstWhere::Always(true)) => {
                    SubstWhere::Always(true)
                }
                (SubstWhere::Always(false), other) | (other, SubstWhere::Always(false)) => other,
                (SubstWhere::Expr(le), SubstWhere::Expr(re)) => {
                    SubstWhere::Expr(WhereExpr::Or(Box::new(le), Box::new(re)))
                }
            }
        }
        WhereExpr::InSubquery {
            column,
            subquery,
            negated,
        } => {
            if let Some(sub) =
                substitute_outer_statement(subquery, outer_row, outer_tables, catalog)
            {
                SubstWhere::Expr(WhereExpr::InSubquery {
                    column: column.clone(),
                    subquery: Box::new(sub),
                    negated: *negated,
                })
            } else {
                SubstWhere::Always(*negated)
            }
        }
        WhereExpr::Exists { subquery, negated } => {
            if let Some(sub) =
                substitute_outer_statement(subquery, outer_row, outer_tables, catalog)
            {
                SubstWhere::Expr(WhereExpr::Exists {
                    subquery: Box::new(sub),
                    negated: *negated,
                })
            } else {
                SubstWhere::Always(*negated)
            }
        }
        WhereExpr::ScalarSubqueryComparison {
            column,
            op,
            subquery,
        } => {
            let sub = substitute_outer_statement(subquery, outer_row, outer_tables, catalog);
            let outer_col = lookup_outer_col(
                column,
                inner_tables,
                inner_names,
                inner_all_known,
                outer_tables,
            );
            match (outer_col, sub) {
                (_, None) => SubstWhere::Always(false),
                (Some(oc), Some(s)) => {
                    let val = outer_row.get_field_or_json_path(&oc).unwrap_or(Value::Null);
                    SubstWhere::Expr(WhereExpr::SubqueryComparison {
                        subquery: Box::new(s),
                        op: flip_comparison_op(op).unwrap_or_else(|| op.clone()),
                        value: val,
                    })
                }
                (None, Some(s)) => SubstWhere::Expr(WhereExpr::ScalarSubqueryComparison {
                    column: column.clone(),
                    op: op.clone(),
                    subquery: Box::new(s),
                }),
            }
        }
        WhereExpr::SubqueryComparison {
            subquery,
            op,
            value,
        } => {
            if let Some(sub) =
                substitute_outer_statement(subquery, outer_row, outer_tables, catalog)
            {
                SubstWhere::Expr(WhereExpr::SubqueryComparison {
                    subquery: Box::new(sub),
                    op: op.clone(),
                    value: value.clone(),
                })
            } else {
                SubstWhere::Always(matches_condition(&Value::Null, op, value))
            }
        }
    }
}

pub(crate) fn substitute_outer_statement(
    stmt: &Statement,
    outer_row: &Row,
    outer_tables: &[&TableDef],
    catalog: &Catalog,
) -> Option<Statement> {
    let mut cloned = stmt.clone();
    match &mut cloned {
        Statement::Select {
            columns,
            where_clause,
            group_by,
            having,
            ..
        } => {
            let (inner_tables, inner_names, inner_all_known) = collect_inner_tables(stmt, catalog);
            let is_agg = group_by.is_some() || super::eval::is_aggregate_query(columns);
            if let Some(w) = where_clause.take() {
                match substitute_outer_where(
                    &w,
                    outer_row,
                    &inner_tables,
                    &inner_names,
                    inner_all_known,
                    outer_tables,
                    catalog,
                ) {
                    SubstWhere::Always(false) => {
                        if !is_agg {
                            return None;
                        }
                        *where_clause = Some(WhereExpr::Condition(WhereCondition {
                            column: "__tapirus_empty".to_string(),
                            op: BinaryOp::Equals,
                            value: Value::Integer(1),
                        }));
                    }
                    SubstWhere::Always(true) => {
                        *where_clause = None;
                    }
                    SubstWhere::Expr(e) => {
                        *where_clause = Some(e);
                    }
                }
            }
            if let Some(h) = having.take() {
                match substitute_outer_where(
                    &h,
                    outer_row,
                    &inner_tables,
                    &inner_names,
                    inner_all_known,
                    outer_tables,
                    catalog,
                ) {
                    SubstWhere::Always(false) => return None,
                    SubstWhere::Always(true) => *having = None,
                    SubstWhere::Expr(e) => *having = Some(e),
                }
            }
            Some(cloned)
        }
        Statement::Compound {
            left,
            right,
            op,
            order_by,
            limit,
            offset,
        } => {
            let l_sub = substitute_outer_statement(left, outer_row, outer_tables, catalog);
            let r_sub = substitute_outer_statement(right, outer_row, outer_tables, catalog);
            match (l_sub, r_sub) {
                (None, None) => None,
                (Some(s), None) | (None, Some(s)) => Some(s),
                (Some(l), Some(r)) => Some(Statement::Compound {
                    left: Box::new(l),
                    right: Box::new(r),
                    op: *op,
                    order_by: order_by.clone(),
                    limit: *limit,
                    offset: *offset,
                }),
            }
        }
        _ => Some(cloned),
    }
}

#[cfg(test)]
mod cbo_bloom_tests {
    use super::*;
    use crate::sql::parser::{JoinClause, JoinType};

    #[test]
    fn test_simd_join_bloom_filter_and_cbo_multi_join_order() {
        let mut bloom = SimdJoinBloomFilter::with_capacity(256);
        for k in 0..120i64 {
            bloom.insert_i64(k * 7);
        }
        for k in 0..120i64 {
            assert!(
                bloom.might_contain_i64(k * 7),
                "Inserted integer key must never yield false negative"
            );
        }
        let mut fp = 0;
        for k in 10_000..10_500i64 {
            if bloom.might_contain_i64(k) {
                fp += 1;
            }
        }
        assert!(fp < 25, "Expected low false positive rate, got {fp}/500");

        let j0 = JoinClause {
            join_type: JoinType::Inner,
            is_left: false,
            table: "events_large".to_string(),
            left_col: "orders.user_id".to_string(),
            right_col: "events_large.user_id".to_string(),
        };
        let j1 = JoinClause {
            join_type: JoinType::Inner,
            is_left: false,
            table: "regions_small".to_string(),
            left_col: "orders.region_id".to_string(),
            right_col: "regions_small.id".to_string(),
        };
        let j2 = JoinClause {
            join_type: JoinType::Inner,
            is_left: false,
            table: "tiers_tiny".to_string(),
            left_col: "orders.tier_id".to_string(),
            right_col: "tiers_tiny.id".to_string(),
        };
        let join_refs = vec![&j0, &j1, &j2];
        let est_rows = vec![200_000, 150, 8];

        let order = plan_cbo_inner_join_order("orders", &join_refs, &est_rows);
        assert_eq!(order, vec![2, 1, 0]);
    }
}
