//! Core database API traits and Value/Row representations.
//!
//! Provides the public interfaces for querying, executing statements,
//! and converting typed relational and vector values.

use crate::error::{Error, Result};
use crate::vector::DistanceMetric;
use serde::{Deserialize, Serialize};

/// A dynamic SQL/Document value in TapirusDB
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    /// NULL
    Null,
    /// 64-bit signed integer
    Integer(i64),
    /// 64-bit IEEE floating point
    Real(f64),
    /// UTF-8 string
    Text(String),
    /// Raw binary blob
    Blob(Vec<u8>),
    /// Dense float embedding vector
    Vector(Vec<f32>),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Null => write!(f, "NULL"),
            Value::Integer(i) => write!(f, "{i}"),
            Value::Real(r) => write!(f, "{r}"),
            Value::Text(s) => write!(f, "{s}"),
            Value::Blob(b) => write!(f, "<BLOB {} B>", b.len()),
            Value::Vector(v) => {
                if v.len() <= 4 {
                    write!(f, "{:?}", v)
                } else {
                    write!(f, "[{:.4}, {:.4}, ... ({} dims)]", v[0], v[1], v.len())
                }
            }
        }
    }
}

impl Value {
    /// Check if the value is NULL
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Compare two values for SQL sorting (ORDER BY)
    pub fn compare(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
            (Value::Null, _) => std::cmp::Ordering::Less,
            (_, Value::Null) => std::cmp::Ordering::Greater,
            (Value::Integer(a), Value::Integer(b)) => a.cmp(b),
            (Value::Integer(a), Value::Real(b)) => (*a as f64).total_cmp(b),
            (Value::Real(a), Value::Integer(b)) => a.total_cmp(&(*b as f64)),
            (Value::Real(a), Value::Real(b)) => a.total_cmp(b),
            (Value::Text(a), Value::Text(b)) => a.cmp(b),
            (Value::Blob(a), Value::Blob(b)) => a.cmp(b),
            (Value::Vector(a), Value::Vector(b)) => a.len().cmp(&b.len()),
            _ => std::cmp::Ordering::Equal,
        }
    }
}

/// A relational row returned from a query
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    columns: Vec<String>,
    values: Vec<Value>,
}

fn col_name_matches(stored: &str, target: &str) -> bool {
    if stored.eq_ignore_ascii_case(target) {
        return true;
    }
    if let Some(pos) = stored.rfind('.') {
        if stored[pos + 1..].eq_ignore_ascii_case(target) {
            return true;
        }
    }
    if let Some(pos) = target.rfind('.') {
        if target[pos + 1..].eq_ignore_ascii_case(stored) {
            return true;
        }
    }
    false
}

fn extract_json_path_value(json_str: &str, raw_path: &str) -> Option<Value> {
    let clean_path = raw_path
        .trim_matches(|c| c == '\'' || c == '"')
        .trim_start_matches('$')
        .trim_start_matches('.');

    let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;
    if clean_path.is_empty() {
        return Some(json_value_to_db_value(&parsed));
    }

    let mut current = &parsed;
    for segment in clean_path.split('.') {
        if segment.is_empty() {
            continue;
        }
        match current {
            serde_json::Value::Object(map) => {
                current = map.get(segment)?;
            }
            serde_json::Value::Array(arr) => {
                let idx: usize = segment.parse().ok()?;
                current = arr.get(idx)?;
            }
            _ => return None,
        }
    }

    Some(json_value_to_db_value(current))
}

fn json_value_to_db_value(val: &serde_json::Value) -> Value {
    match val {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Integer(if *b { 1 } else { 0 }),
        serde_json::Value::Number(num) => {
            if let Some(i) = num.as_i64() {
                Value::Integer(i)
            } else if let Some(f) = num.as_f64() {
                Value::Real(f)
            } else {
                Value::Null
            }
        }
        serde_json::Value::String(s) => Value::Text(s.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            Value::Text(val.to_string())
        }
    }
}

impl Row {
    /// Create a new Row from column names and values
    pub fn new(columns: Vec<String>, values: Vec<Value>) -> Self {
        Self { columns, values }
    }

    /// Push or overwrite a column value in the row
    pub fn push_column(&mut self, col_name: String, val: Value) {
        if let Some(pos) = self.columns.iter().position(|c| col_name_matches(c, &col_name)) {
            self.values[pos] = val;
        } else {
            self.columns.push(col_name);
            self.values.push(val);
        }
    }

    /// Extract a typed column by name (supports both qualified "table.col" and unqualified "col")
    pub fn get<T: FromValue>(&self, col_name: &str) -> Result<T> {
        let idx = self.columns.iter().position(|c| col_name_matches(c, col_name))
            .ok_or_else(|| Error::Corrupted(format!("Column '{col_name}' not found in row")))?;
        T::from_value(&self.values[idx])
    }

    /// Extract raw Value reference by column name
    pub fn get_value(&self, col_name: &str) -> Option<&Value> {
        let idx = self.columns.iter().position(|c| col_name_matches(c, col_name))?;
        self.values.get(idx)
    }

    /// Extract a Value reference or evaluate a nested JSON path expression (e.g. `doc.address.city` or `JSON_EXTRACT(doc, '$.city')`)
    pub fn get_field_or_json_path(&self, col_expr: &str) -> Option<Value> {
        // 1. Direct match on column name
        if let Some(val) = self.get_value(col_expr) {
            return Some(val.clone());
        }

        // 2. Function syntax: JSON_EXTRACT(col, '$.path')
        let trimmed = col_expr.trim();
        if trimmed.to_ascii_uppercase().starts_with("JSON_EXTRACT(") && trimmed.ends_with(')') {
            let inner = &trimmed[13..trimmed.len() - 1];
            if let Some(comma_pos) = inner.find(',') {
                let col = inner[..comma_pos].trim();
                let path = inner[comma_pos + 1..].trim();
                if let Some(Value::Text(json_str)) = self.get_value(col) {
                    return extract_json_path_value(json_str, path);
                }
            }
        }

        // 3. Dotted path syntax: base_col.field.subfield
        if let Some(dot_pos) = col_expr.find('.') {
            let base = &col_expr[..dot_pos];
            let path = &col_expr[dot_pos + 1..];

            // Check if base matches any column in the row
            if let Some(Value::Text(json_str)) = self.get_value(base) {
                return extract_json_path_value(json_str, path);
            }
        }

        None
    }

    /// Extract a typed column by index (0-based)
    pub fn get_idx<T: FromValue>(&self, idx: usize) -> Result<T> {
        let val = self.values.get(idx)
            .ok_or_else(|| Error::Corrupted(format!("Column index {idx} out of bounds (len: {})", self.values.len())))?;
        T::from_value(val)
    }

    /// Column names
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Column values
    pub fn values(&self) -> &[Value] {
        &self.values
    }
}

/// Conversion trait from a TapirusDB `Value` into a native Rust type
pub trait FromValue: Sized {
    /// Convert from value reference
    fn from_value(value: &Value) -> Result<Self>;
}

impl FromValue for i64 {
    fn from_value(value: &Value) -> Result<Self> {
        match value {
            Value::Integer(i) => Ok(*i),
            _ => Err(Error::Corrupted("Type mismatch: expected Integer".into())),
        }
    }
}

impl FromValue for i32 {
    fn from_value(value: &Value) -> Result<Self> {
        match value {
            Value::Integer(i) => Ok(*i as i32),
            _ => Err(Error::Corrupted("Type mismatch: expected Integer".into())),
        }
    }
}

impl FromValue for u64 {
    fn from_value(value: &Value) -> Result<Self> {
        match value {
            Value::Integer(i) if *i >= 0 => Ok(*i as u64),
            _ => Err(Error::Corrupted("Type mismatch: expected non-negative Integer for u64".into())),
        }
    }
}

impl FromValue for usize {
    fn from_value(value: &Value) -> Result<Self> {
        match value {
            Value::Integer(i) if *i >= 0 => Ok(*i as usize),
            _ => Err(Error::Corrupted("Type mismatch: expected non-negative Integer for usize".into())),
        }
    }
}

impl FromValue for bool {
    fn from_value(value: &Value) -> Result<Self> {
        match value {
            Value::Integer(i) => Ok(*i != 0),
            Value::Text(s) => match s.to_lowercase().as_str() {
                "true" | "t" | "1" | "yes" => Ok(true),
                "false" | "f" | "0" | "no" => Ok(false),
                _ => Err(Error::Corrupted(format!("Invalid boolean text representation: {s}"))),
            },
            _ => Err(Error::Corrupted("Type mismatch: expected Boolean (Integer or Text)".into())),
        }
    }
}

impl FromValue for f64 {
    fn from_value(value: &Value) -> Result<Self> {
        match value {
            Value::Real(r) => Ok(*r),
            Value::Integer(i) => Ok(*i as f64),
            _ => Err(Error::Corrupted("Type mismatch: expected Real".into())),
        }
    }
}

impl FromValue for String {
    fn from_value(value: &Value) -> Result<Self> {
        match value {
            Value::Text(s) => Ok(s.clone()),
            _ => Err(Error::Corrupted("Type mismatch: expected Text".into())),
        }
    }
}

impl FromValue for Vec<u8> {
    fn from_value(value: &Value) -> Result<Self> {
        match value {
            Value::Blob(b) => Ok(b.clone()),
            _ => Err(Error::Corrupted("Type mismatch: expected Blob".into())),
        }
    }
}

impl FromValue for Vec<f32> {
    fn from_value(value: &Value) -> Result<Self> {
        match value {
            Value::Vector(v) => Ok(v.clone()),
            _ => Err(Error::Corrupted("Type mismatch: expected Vector".into())),
        }
    }
}

impl FromValue for Value {
    fn from_value(value: &Value) -> Result<Self> {
        Ok(value.clone())
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(value: &Value) -> Result<Self> {
        if value.is_null() {
            Ok(None)
        } else {
            T::from_value(value).map(Some)
        }
    }
}

/// Abstract Vector Index Engine trait for native ANN search
pub trait VectorIndexEngine {
    /// Insert a vector into the index with an associated record ID
    fn insert_vector(&mut self, id: u64, vector: &[f32]) -> Result<()>;

    /// Search K-nearest neighbors for a query vector
    fn search_knn(&self, query: &[f32], k: usize, metric: DistanceMetric) -> Result<Vec<(u64, f32)>>;
}

/// Public Database Connection Trait
pub trait DatabaseConnection {
    /// Execute a SQL DDL or DML statement
    fn execute(&self, sql: &str) -> Result<usize>;

    /// Execute a SQL query returning multiple rows
    fn query(&self, sql: &str) -> Result<Vec<Row>>;

    /// Execute a parameterized non-query SQL command
    fn execute_with_params(&self, sql: &str, params: &[Value]) -> Result<usize> {
        let _ = (sql, params);
        Err(Error::Corrupted("Parameterized execution is not supported by this connection implementation".into()))
    }

    /// Execute a parameterized SQL query returning multiple rows
    fn query_with_params(&self, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
        let _ = (sql, params);
        Err(Error::Corrupted("Parameterized queries are not supported by this connection implementation".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_row_extraction() {
        let cols = vec!["id".into(), "name".into(), "score".into(), "embedding".into()];
        let vals = vec![
            Value::Integer(1),
            Value::Text("Faiz".into()),
            Value::Real(99.5),
            Value::Vector(vec![0.1, 0.2, 0.3]),
        ];

        let row = Row::new(cols, vals);
        let id: i64 = row.get("id").expect("Failed to get id");
        let name: String = row.get("name").expect("Failed to get name");
        let score: f64 = row.get("score").expect("Failed to get score");
        let embedding: Vec<f32> = row.get("embedding").expect("Failed to get embedding");

        assert_eq!(id, 1);
        assert_eq!(name, "Faiz");
        assert!((score - 99.5).abs() < 1e-4);
        assert_eq!(embedding.len(), 3);
    }
}
