//! Micro-Columnar Vectorized Aggregation Engine.
//!
//! Provides ultra-fast batched scans for aggregations (`COUNT`, `SUM`, `AVG`, `MIN`, `MAX`)
//! operating directly on columnar primitive arrays with unrolled loops without allocating
//! per-row `Row` structs.
//!
//! Complies strictly with `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

use crate::traits::Value;

/// Contiguous columnar batch of numeric or text values for vectorized aggregation
#[derive(Debug, Clone)]
pub enum ColumnBatch {
    /// Vector of 64-bit signed integers
    Integer(Vec<i64>),
    /// Vector of 64-bit floating point numbers
    Real(Vec<f64>),
    /// Vector of string text values
    Text(Vec<String>),
    /// Run of null values with specified count
    Null(usize),
}

/// Accumulator state for vectorized aggregations
#[derive(Debug, Clone, Default)]
pub struct VectorizedAccumulator {
    /// Count of non-null values
    pub count: usize,
    /// Total row count including nulls
    pub count_all: usize,
    /// Integer sum accumulator
    pub sum_i: i64,
    /// Floating-point sum accumulator
    pub sum_f: f64,
    /// Flag indicating if any floating point values were accumulated
    pub has_float: bool,
    /// Minimum integer value observed
    pub min_i: Option<i64>,
    /// Maximum integer value observed
    pub max_i: Option<i64>,
    /// Minimum float value observed
    pub min_f: Option<f64>,
    /// Maximum float value observed
    pub max_f: Option<f64>,
    /// Minimum string value observed
    pub min_str: Option<String>,
    /// Maximum string value observed
    pub max_str: Option<String>,
}

impl VectorizedAccumulator {
    /// Create a new empty accumulator
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a batch of 64-bit integers with unrolled loop
    #[inline]
    pub fn accumulate_integers(&mut self, values: &[i64], mask: Option<&[bool]>) {
        self.count_all += values.len();
        match mask {
            Some(m) => {
                for (&v, &active) in values.iter().zip(m.iter()) {
                    if active {
                        self.count += 1;
                        self.sum_i = self.sum_i.wrapping_add(v);
                        self.sum_f += v as f64;
                        self.min_i = Some(self.min_i.map_or(v, |curr| curr.min(v)));
                        self.max_i = Some(self.max_i.map_or(v, |curr| curr.max(v)));
                    }
                }
            }
            None => {
                self.count += values.len();
                // 4x unrolled accumulation loop
                let chunks = values.chunks_exact(4);
                let remainder = chunks.remainder();
                let mut s0 = 0i64;
                let mut s1 = 0i64;
                let mut s2 = 0i64;
                let mut s3 = 0i64;

                for c in chunks {
                    s0 = s0.wrapping_add(c[0]);
                    s1 = s1.wrapping_add(c[1]);
                    s2 = s2.wrapping_add(c[2]);
                    s3 = s3.wrapping_add(c[3]);
                    let m = c[0].min(c[1]).min(c[2].min(c[3]));
                    let mx = c[0].max(c[1]).max(c[2].max(c[3]));
                    self.min_i = Some(self.min_i.map_or(m, |curr| curr.min(m)));
                    self.max_i = Some(self.max_i.map_or(mx, |curr| curr.max(mx)));
                }

                self.sum_i = self
                    .sum_i
                    .wrapping_add(s0)
                    .wrapping_add(s1)
                    .wrapping_add(s2)
                    .wrapping_add(s3);
                self.sum_f += (s0 as f64) + (s1 as f64) + (s2 as f64) + (s3 as f64);

                for &v in remainder {
                    self.sum_i = self.sum_i.wrapping_add(v);
                    self.sum_f += v as f64;
                    self.min_i = Some(self.min_i.map_or(v, |curr| curr.min(v)));
                    self.max_i = Some(self.max_i.map_or(v, |curr| curr.max(v)));
                }
            }
        }
    }

    /// Process a batch of 64-bit floats with unrolled loop
    #[inline]
    pub fn accumulate_floats(&mut self, values: &[f64], mask: Option<&[bool]>) {
        self.count_all += values.len();
        self.has_float = true;
        match mask {
            Some(m) => {
                for (&v, &active) in values.iter().zip(m.iter()) {
                    if active {
                        self.count += 1;
                        self.sum_f += v;
                        self.min_f = Some(self.min_f.map_or(v, |curr| curr.min(v)));
                        self.max_f = Some(self.max_f.map_or(v, |curr| curr.max(v)));
                    }
                }
            }
            None => {
                self.count += values.len();
                let chunks = values.chunks_exact(4);
                let remainder = chunks.remainder();
                let mut s0 = 0.0f64;
                let mut s1 = 0.0f64;
                let mut s2 = 0.0f64;
                let mut s3 = 0.0f64;

                for c in chunks {
                    s0 += c[0];
                    s1 += c[1];
                    s2 += c[2];
                    s3 += c[3];
                    let m = c[0].min(c[1]).min(c[2].min(c[3]));
                    let mx = c[0].max(c[1]).max(c[2].max(c[3]));
                    self.min_f = Some(self.min_f.map_or(m, |curr| curr.min(m)));
                    self.max_f = Some(self.max_f.map_or(mx, |curr| curr.max(mx)));
                }

                self.sum_f += s0 + s1 + s2 + s3;

                for &v in remainder {
                    self.sum_f += v;
                    self.min_f = Some(self.min_f.map_or(v, |curr| curr.min(v)));
                    self.max_f = Some(self.max_f.map_or(v, |curr| curr.max(v)));
                }
            }
        }
    }

    /// Process arbitrary TapirusDB Value slice
    pub fn accumulate_values<'a, I>(&mut self, values: I)
    where
        I: IntoIterator<Item = &'a Value>,
    {
        for v in values {
            self.count_all += 1;
            match v {
                Value::Integer(i) => {
                    self.count += 1;
                    self.sum_i = self.sum_i.wrapping_add(*i);
                    self.sum_f += *i as f64;
                    self.min_i = Some(self.min_i.map_or(*i, |curr| curr.min(*i)));
                    self.max_i = Some(self.max_i.map_or(*i, |curr| curr.max(*i)));
                }
                Value::Real(f) => {
                    self.count += 1;
                    self.has_float = true;
                    self.sum_f += *f;
                    self.min_f = Some(self.min_f.map_or(*f, |curr| curr.min(*f)));
                    self.max_f = Some(self.max_f.map_or(*f, |curr| curr.max(*f)));
                }
                Value::Text(s) => {
                    self.count += 1;
                    self.min_str = Some(
                        self.min_str
                            .as_ref()
                            .map_or(s.clone(), |curr| curr.min(s).clone()),
                    );
                    self.max_str = Some(
                        self.max_str
                            .as_ref()
                            .map_or(s.clone(), |curr| curr.max(s).clone()),
                    );
                }
                Value::Null => {}
                _ => {
                    self.count += 1;
                }
            }
        }
    }

    /// Finalize COUNT aggregate
    pub fn finalize_count(&self, is_count_all: bool) -> Value {
        if is_count_all {
            Value::Integer(self.count_all as i64)
        } else {
            Value::Integer(self.count as i64)
        }
    }

    /// Finalize SUM aggregate
    pub fn finalize_sum(&self) -> Value {
        if self.count == 0 {
            Value::Null
        } else if self.has_float {
            Value::Real(self.sum_f)
        } else {
            Value::Integer(self.sum_i)
        }
    }

    /// Finalize AVG aggregate
    pub fn finalize_avg(&self) -> Value {
        if self.count == 0 {
            Value::Null
        } else {
            Value::Real(self.sum_f / (self.count as f64))
        }
    }

    /// Finalize MIN aggregate
    pub fn finalize_min(&self) -> Value {
        if self.count == 0 {
            Value::Null
        } else if self.has_float {
            Value::Real(self.min_f.unwrap_or(0.0))
        } else if let Some(i) = self.min_i {
            Value::Integer(i)
        } else if let Some(ref s) = self.min_str {
            Value::Text(s.clone())
        } else {
            Value::Null
        }
    }

    /// Finalize MAX aggregate
    pub fn finalize_max(&self) -> Value {
        if self.count == 0 {
            Value::Null
        } else if self.has_float {
            Value::Real(self.max_f.unwrap_or(0.0))
        } else if let Some(i) = self.max_i {
            Value::Integer(i)
        } else if let Some(ref s) = self.max_str {
            Value::Text(s.clone())
        } else {
            Value::Null
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vectorized_integers() {
        let mut acc = VectorizedAccumulator::new();
        let ints: Vec<i64> = (1..=1000).collect();
        acc.accumulate_integers(&ints, None);

        assert_eq!(acc.finalize_count(true), Value::Integer(1000));
        assert_eq!(acc.finalize_sum(), Value::Integer(500500));
        assert_eq!(acc.finalize_avg(), Value::Real(500.5));
        assert_eq!(acc.finalize_min(), Value::Integer(1));
        assert_eq!(acc.finalize_max(), Value::Integer(1000));
    }

    #[test]
    fn test_vectorized_masked() {
        let mut acc = VectorizedAccumulator::new();
        let ints = vec![10, 20, 30, 40, 50];
        let mask = vec![true, false, true, false, true];
        acc.accumulate_integers(&ints, Some(&mask));

        assert_eq!(acc.finalize_count(false), Value::Integer(3));
        assert_eq!(acc.finalize_sum(), Value::Integer(90));
        assert_eq!(acc.finalize_min(), Value::Integer(10));
        assert_eq!(acc.finalize_max(), Value::Integer(50));
    }
}
