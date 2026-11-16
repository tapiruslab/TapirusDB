//! Dynamic User-Defined Function (UDF) Registry for TapirusDB SQL Engine.
//!
//! Allows registering custom scalar Rust closures callable directly in SQL queries.

use crate::error::{Error, Result};
use crate::traits::Value;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Dynamic scalar function signature taking an argument slice of Values and returning a Value
pub type ScalarUdf = Arc<dyn Fn(&[Value]) -> Result<Value> + Send + Sync>;

/// Thread-safe registry of user-defined and standard scalar functions
#[derive(Clone)]
pub struct UdfRegistry {
    functions: Arc<RwLock<HashMap<String, ScalarUdf>>>,
}

impl std::fmt::Debug for UdfRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<String> = self.functions.read().keys().cloned().collect();
        f.debug_struct("UdfRegistry")
            .field("functions", &names)
            .finish()
    }
}

impl Default for UdfRegistry {
    fn default() -> Self {
        let registry = Self {
            functions: Arc::new(RwLock::new(HashMap::new())),
        };
        registry.register_standard_builtins();
        registry
    }
}

impl UdfRegistry {
    /// Create a new empty UdfRegistry with standard built-in functions
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a custom scalar UDF
    pub fn register<F>(&self, name: &str, func: F)
    where
        F: Fn(&[Value]) -> Result<Value> + Send + Sync + 'static,
    {
        let key = name.trim().to_ascii_uppercase();
        self.functions.write().insert(key, Arc::new(func));
    }

    /// Remove a registered UDF
    pub fn unregister(&self, name: &str) -> bool {
        let key = name.trim().to_ascii_uppercase();
        self.functions.write().remove(&key).is_some()
    }

    /// Check if a function exists in the registry
    pub fn contains(&self, name: &str) -> bool {
        let key = name.trim().to_ascii_uppercase();
        self.functions.read().contains_key(&key)
    }

    /// Call a registered scalar function with arguments
    pub fn call(&self, name: &str, args: &[Value]) -> Result<Value> {
        let key = name.trim().to_ascii_uppercase();
        let func = {
            let guard = self.functions.read();
            guard
                .get(&key)
                .cloned()
                .ok_or_else(|| Error::SqlSyntax(format!("Unknown scalar function: '{name}'")))?
        };
        func(args)
    }

    /// Try calling a function if it exists, returning None if not found
    pub fn try_call(&self, name: &str, args: &[Value]) -> Result<Option<Value>> {
        let key = name.trim().to_ascii_uppercase();
        let func = {
            let guard = self.functions.read();
            match guard.get(&key).cloned() {
                Some(f) => f,
                None => return Ok(None),
            }
        };
        func(args).map(Some)
    }

    /// Register standard built-in scalar SQL functions
    fn register_standard_builtins(&self) {
        // UPPER(str)
        self.register("UPPER", |args| match args.first() {
            Some(Value::Text(s)) => Ok(Value::Text(s.to_uppercase())),
            Some(Value::Null) | None => Ok(Value::Null),
            Some(other) => Ok(Value::Text(other.to_string().to_uppercase())),
        });

        // LOWER(str)
        self.register("LOWER", |args| match args.first() {
            Some(Value::Text(s)) => Ok(Value::Text(s.to_lowercase())),
            Some(Value::Null) | None => Ok(Value::Null),
            Some(other) => Ok(Value::Text(other.to_string().to_lowercase())),
        });

        // LENGTH(str) / LEN(str)
        let len_fn = |args: &[Value]| match args.first() {
            Some(Value::Text(s)) => Ok(Value::Integer(s.chars().count() as i64)),
            Some(Value::Blob(b)) => Ok(Value::Integer(b.len() as i64)),
            Some(Value::Null) | None => Ok(Value::Null),
            Some(other) => Ok(Value::Integer(other.to_string().chars().count() as i64)),
        };
        self.register("LENGTH", len_fn);
        self.register("LEN", len_fn);

        // ABS(num)
        self.register("ABS", |args| match args.first() {
            Some(Value::Integer(i)) => Ok(Value::Integer(i.abs())),
            Some(Value::Real(r)) => Ok(Value::Real(r.abs())),
            Some(Value::Null) | None => Ok(Value::Null),
            Some(other) => Err(Error::ConstraintViolation(format!(
                "Cannot compute ABS on {other:?}"
            ))),
        });

        // ROUND(num, [decimals])
        self.register("ROUND", |args| match args.first() {
            Some(Value::Real(r)) => {
                let decimals = args
                    .get(1)
                    .and_then(|v| match v {
                        Value::Integer(i) => Some(*i as i32),
                        _ => None,
                    })
                    .unwrap_or(0);
                if decimals <= 0 {
                    Ok(Value::Real(r.round()))
                } else {
                    let factor = 10f64.powi(decimals);
                    Ok(Value::Real((r * factor).round() / factor))
                }
            }
            Some(Value::Integer(i)) => Ok(Value::Integer(*i)),
            Some(Value::Null) | None => Ok(Value::Null),
            Some(other) => Err(Error::ConstraintViolation(format!(
                "Cannot ROUND {other:?}"
            ))),
        });

        // COALESCE(val1, val2, ...)
        self.register("COALESCE", |args| {
            for arg in args {
                if !arg.is_null() {
                    return Ok(arg.clone());
                }
            }
            Ok(Value::Null)
        });

        // CONCAT(str1, str2, ...)
        self.register("CONCAT", |args| {
            let mut result = String::new();
            for arg in args {
                match arg {
                    Value::Null => {}
                    Value::Text(s) => result.push_str(s),
                    other => result.push_str(&other.to_string()),
                }
            }
            Ok(Value::Text(result))
        });

        // SUBSTR(str, start, [length])
        self.register("SUBSTR", |args| {
            let s_owned;
            let s = match args.first() {
                Some(Value::Text(s)) => s.as_str(),
                Some(Value::Null) | None => return Ok(Value::Null),
                Some(other) => {
                    s_owned = other.to_string();
                    s_owned.as_str()
                }
            };
            let start = match args.get(1) {
                Some(Value::Integer(i)) => (*i).max(1) as usize - 1,
                _ => 0,
            };
            let chars: Vec<char> = s.chars().collect();
            if start >= chars.len() {
                return Ok(Value::Text(String::new()));
            }
            let len = match args.get(2) {
                Some(Value::Integer(i)) => (*i).max(0) as usize,
                _ => chars.len() - start,
            };
            let end = (start + len).min(chars.len());
            let sub: String = chars[start..end].iter().collect();
            Ok(Value::Text(sub))
        });

        // TRIM(str [, chars]), LTRIM(str [, chars]), RTRIM(str [, chars])
        let apply_trim = |args: &[Value], left: bool, right: bool| -> Result<Value> {
            let Some(val) = args.first() else {
                return Ok(Value::Null);
            };
            if val.is_null() {
                return Ok(Value::Null);
            }
            let s = match val {
                Value::Text(s) => s.clone(),
                other => other.to_string(),
            };
            let chars_owned: Option<Vec<char>> = match args.get(1) {
                Some(Value::Text(c)) => Some(c.chars().collect()),
                Some(Value::Null) => return Ok(Value::Null),
                Some(other) => Some(other.to_string().chars().collect()),
                None => None,
            };
            let out = match chars_owned {
                Some(ref pat) => {
                    let slice = pat.as_slice();
                    match (left, right) {
                        (true, true) => s.trim_matches(slice),
                        (true, false) => s.trim_start_matches(slice),
                        (false, true) => s.trim_end_matches(slice),
                        (false, false) => s.as_str(),
                    }
                }
                None => match (left, right) {
                    (true, true) => s.trim(),
                    (true, false) => s.trim_start(),
                    (false, true) => s.trim_end(),
                    (false, false) => s.as_str(),
                },
            };
            Ok(Value::Text(out.to_string()))
        };
        self.register("TRIM", move |args| apply_trim(args, true, true));
        self.register("LTRIM", move |args| apply_trim(args, true, false));
        self.register("RTRIM", move |args| apply_trim(args, false, true));

        // REVERSE(str)
        self.register("REVERSE", |args| match args.first() {
            Some(Value::Text(s)) => Ok(Value::Text(s.chars().rev().collect())),
            Some(Value::Null) | None => Ok(Value::Null),
            Some(other) => Ok(Value::Text(other.to_string().chars().rev().collect())),
        });

        // IFNULL(x, y)
        self.register("IFNULL", |args| {
            for arg in args {
                if !arg.is_null() {
                    return Ok(arg.clone());
                }
            }
            Ok(Value::Null)
        });

        // NULLIF(x, y)
        self.register("NULLIF", |args| match (args.first(), args.get(1)) {
            (Some(x), Some(y)) if !x.is_null() && x == y => Ok(Value::Null),
            (Some(x), _) => Ok(x.clone()),
            (None, _) => Ok(Value::Null),
        });

        // REPLACE(str, from, to)
        self.register("REPLACE", |args| {
            let (Some(a0), Some(a1), Some(a2)) = (args.first(), args.get(1), args.get(2)) else {
                return Ok(Value::Null);
            };
            if a0.is_null() || a1.is_null() || a2.is_null() {
                return Ok(Value::Null);
            }
            let s = match a0 {
                Value::Text(v) => v.clone(),
                other => other.to_string(),
            };
            let from = match a1 {
                Value::Text(v) => v.clone(),
                other => other.to_string(),
            };
            let to = match a2 {
                Value::Text(v) => v.clone(),
                other => other.to_string(),
            };
            if from.is_empty() {
                return Ok(Value::Text(s));
            }
            Ok(Value::Text(s.replace(&from, &to)))
        });

        // INSTR(haystack, needle) — 1-based char index, 0 when not found
        self.register("INSTR", |args| {
            let (Some(a0), Some(a1)) = (args.first(), args.get(1)) else {
                return Ok(Value::Null);
            };
            if a0.is_null() || a1.is_null() {
                return Ok(Value::Null);
            }
            let hay = match a0 {
                Value::Text(v) => v.clone(),
                other => other.to_string(),
            };
            let needle = match a1 {
                Value::Text(v) => v.clone(),
                other => other.to_string(),
            };
            if needle.is_empty() {
                return Ok(Value::Integer(1));
            }
            let pos = hay
                .find(&needle)
                .map(|byte_idx| hay[..byte_idx].chars().count() as i64 + 1)
                .unwrap_or(0);
            Ok(Value::Integer(pos))
        });

        // TYPEOF(val)
        self.register("TYPEOF", |args| {
            let kind = match args.first() {
                None | Some(Value::Null) => "null",
                Some(Value::Integer(_)) => "integer",
                Some(Value::Real(_)) => "real",
                Some(Value::Text(_)) => "text",
                Some(Value::Blob(_)) => "blob",
                Some(Value::Vector(_)) => "vector",
            };
            Ok(Value::Text(kind.to_string()))
        });

        // FLOOR(num), CEIL(num) / CEILING(num), SQRT(num), POWER(a, b) / POW(a, b), MOD(a, b)
        let to_f64 = |v: &Value| -> Option<f64> {
            match v {
                Value::Integer(i) => Some(*i as f64),
                Value::Real(f) => Some(*f),
                Value::Text(s) => s.trim().parse::<f64>().ok(),
                _ => None,
            }
        };
        self.register("FLOOR", move |args| match args.first() {
            Some(Value::Null) | None => Ok(Value::Null),
            Some(Value::Integer(i)) => Ok(Value::Integer(*i)),
            Some(v) => match to_f64(v) {
                Some(f) => Ok(Value::Real(f.floor())),
                None => Err(Error::ConstraintViolation(format!(
                    "Cannot compute FLOOR on {v:?}"
                ))),
            },
        });
        let ceil_fn = move |args: &[Value]| match args.first() {
            Some(Value::Null) | None => Ok(Value::Null),
            Some(Value::Integer(i)) => Ok(Value::Integer(*i)),
            Some(v) => match to_f64(v) {
                Some(f) => Ok(Value::Real(f.ceil())),
                None => Err(Error::ConstraintViolation(format!(
                    "Cannot compute CEIL on {v:?}"
                ))),
            },
        };
        self.register("CEIL", ceil_fn);
        self.register("CEILING", ceil_fn);
        self.register("SQRT", move |args| match args.first() {
            Some(Value::Null) | None => Ok(Value::Null),
            Some(v) => match to_f64(v) {
                Some(f) if f >= 0.0 => Ok(Value::Real(f.sqrt())),
                Some(_) => Ok(Value::Null),
                None => Err(Error::ConstraintViolation(format!(
                    "Cannot compute SQRT on {v:?}"
                ))),
            },
        });
        let power_fn = move |args: &[Value]| match (args.first(), args.get(1)) {
            (Some(Value::Null), _) | (_, Some(Value::Null)) | (None, _) | (_, None) => {
                Ok(Value::Null)
            }
            (Some(a), Some(b)) => match (to_f64(a), to_f64(b)) {
                (Some(x), Some(y)) => Ok(Value::Real(x.powf(y))),
                _ => Err(Error::ConstraintViolation(format!(
                    "Cannot compute POWER({a:?}, {b:?})"
                ))),
            },
        };
        self.register("POWER", power_fn);
        self.register("POW", power_fn);
        self.register("MOD", move |args| match (args.first(), args.get(1)) {
            (Some(Value::Null), _) | (_, Some(Value::Null)) | (None, _) | (_, None) => {
                Ok(Value::Null)
            }
            (Some(Value::Integer(_)), Some(Value::Integer(0))) => Ok(Value::Null),
            (Some(Value::Integer(a)), Some(Value::Integer(b))) => Ok(Value::Integer(a % b)),
            (Some(a), Some(b)) => match (to_f64(a), to_f64(b)) {
                (Some(_), Some(0.0)) => Ok(Value::Null),
                (Some(x), Some(y)) => Ok(Value::Real(x % y)),
                _ => Err(Error::ConstraintViolation(format!(
                    "Cannot compute MOD({a:?}, {b:?})"
                ))),
            },
        });

        // HEX(val)
        self.register("HEX", |args| {
            let hex_bytes = |bytes: &[u8]| -> String {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                let mut out = String::with_capacity(bytes.len() * 2);
                for &b in bytes {
                    out.push(HEX[(b >> 4) as usize] as char);
                    out.push(HEX[(b & 0x0F) as usize] as char);
                }
                out
            };
            match args.first() {
                None | Some(Value::Null) => Ok(Value::Null),
                Some(Value::Blob(b)) => Ok(Value::Text(hex_bytes(b))),
                Some(Value::Text(s)) => Ok(Value::Text(hex_bytes(s.as_bytes()))),
                Some(other) => Ok(Value::Text(hex_bytes(other.to_string().as_bytes()))),
            }
        });
    }
}
