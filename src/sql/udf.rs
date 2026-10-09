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
            guard.get(&key).cloned().ok_or_else(|| {
                Error::SqlSyntax(format!("Unknown scalar function: '{name}'"))
            })?
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
        self.register("UPPER", |args| {
            match args.first() {
                Some(Value::Text(s)) => Ok(Value::Text(s.to_uppercase())),
                Some(Value::Null) | None => Ok(Value::Null),
                Some(other) => Ok(Value::Text(other.to_string().to_uppercase())),
            }
        });

        // LOWER(str)
        self.register("LOWER", |args| {
            match args.first() {
                Some(Value::Text(s)) => Ok(Value::Text(s.to_lowercase())),
                Some(Value::Null) | None => Ok(Value::Null),
                Some(other) => Ok(Value::Text(other.to_string().to_lowercase())),
            }
        });

        // LENGTH(str) / LEN(str)
        let len_fn = |args: &[Value]| {
            match args.first() {
                Some(Value::Text(s)) => Ok(Value::Integer(s.chars().count() as i64)),
                Some(Value::Blob(b)) => Ok(Value::Integer(b.len() as i64)),
                Some(Value::Null) | None => Ok(Value::Null),
                Some(other) => Ok(Value::Integer(other.to_string().chars().count() as i64)),
            }
        };
        self.register("LENGTH", len_fn);
        self.register("LEN", len_fn);

        // ABS(num)
        self.register("ABS", |args| {
            match args.first() {
                Some(Value::Integer(i)) => Ok(Value::Integer(i.abs())),
                Some(Value::Real(r)) => Ok(Value::Real(r.abs())),
                Some(Value::Null) | None => Ok(Value::Null),
                Some(other) => Err(Error::ConstraintViolation(format!("Cannot compute ABS on {other:?}"))),
            }
        });

        // ROUND(num, [decimals])
        self.register("ROUND", |args| {
            match args.first() {
                Some(Value::Real(r)) => {
                    let decimals = args.get(1).and_then(|v| match v {
                        Value::Integer(i) => Some(*i as i32),
                        _ => None,
                    }).unwrap_or(0);
                    if decimals <= 0 {
                        Ok(Value::Real(r.round()))
                    } else {
                        let factor = 10f64.powi(decimals);
                        Ok(Value::Real((r * factor).round() / factor))
                    }
                }
                Some(Value::Integer(i)) => Ok(Value::Integer(*i)),
                Some(Value::Null) | None => Ok(Value::Null),
                Some(other) => Err(Error::ConstraintViolation(format!("Cannot ROUND {other:?}"))),
            }
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

        // TRIM(str)
        self.register("TRIM", |args| {
            match args.first() {
                Some(Value::Text(s)) => Ok(Value::Text(s.trim().to_string())),
                Some(Value::Null) | None => Ok(Value::Null),
                Some(other) => Ok(Value::Text(other.to_string().trim().to_string())),
            }
        });

        // REVERSE(str)
        self.register("REVERSE", |args| {
            match args.first() {
                Some(Value::Text(s)) => Ok(Value::Text(s.chars().rev().collect())),
                Some(Value::Null) | None => Ok(Value::Null),
                Some(other) => Ok(Value::Text(other.to_string().chars().rev().collect())),
            }
        });
    }
}
