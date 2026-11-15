//! Micro-Columnar Vectorized Aggregation Engine.
//!
//! Provides ultra-fast batched scans for aggregations (`COUNT`, `SUM`, `AVG`, `MIN`, `MAX`)
//! operating directly on columnar primitive arrays with unrolled loops without allocating
//! per-row `Row` structs.
//!
//! Complies strictly with `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

use crate::traits::{HashableValue, Value};
use std::collections::HashSet;

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

/// Accumulator state for vectorized and analytical aggregations
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
    /// Welford running mean for online variance/stddev
    mean: f64,
    /// Welford sum of squares of differences from the current mean
    m2: f64,
    /// Number of numeric values accumulated into Welford state
    numeric_count: usize,
    /// Whether numeric samples should be buffered for exact MEDIAN/PERCENTILE
    collect_samples: bool,
    /// Numeric sample buffer (allocated only when `collect_samples` is true)
    samples: Vec<f64>,
    /// Whether distinct values should be tracked for COUNT(DISTINCT ...)
    collect_distinct: bool,
    /// Distinct non-null values set (allocated only when `collect_distinct` is true)
    distinct_values: HashSet<HashableValue>,
}

impl VectorizedAccumulator {
    /// Create a new empty accumulator (zero-allocation fast path for streaming metrics)
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an accumulator configured to retain numeric samples and/or distinct sets
    pub fn with_tracking(collect_samples: bool, collect_distinct: bool) -> Self {
        Self {
            collect_samples,
            collect_distinct,
            ..Self::default()
        }
    }

    #[inline]
    fn push_numeric_stat(&mut self, x: f64) {
        if !x.is_finite() {
            return;
        }
        self.numeric_count += 1;
        let delta = x - self.mean;
        self.mean += delta / (self.numeric_count as f64);
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;
        if self.collect_samples {
            self.samples.push(x);
        }
    }

    /// Accumulate a contiguous `i64` slice, optionally computing Welford/sample/distinct stats.
    ///
    /// When `needs_welford`, `collect_samples`, and `collect_distinct` are all false, executes a
    /// zero-branch SIMD/unrolled reduction loop directly on `&[i64]`.
    #[inline]
    pub fn accumulate_int64_slice(&mut self, values: &[i64], needs_welford: bool) {
        if !needs_welford && !self.collect_samples && !self.collect_distinct {
            self.accumulate_integers(values, None);
            return;
        }
        for &v in values {
            self.count_all += 1;
            self.count += 1;
            self.sum_i = self.sum_i.wrapping_add(v);
            let f = v as f64;
            self.sum_f += f;
            self.min_i = Some(self.min_i.map_or(v, |curr| curr.min(v)));
            self.max_i = Some(self.max_i.map_or(v, |curr| curr.max(v)));
            self.push_numeric_stat(f);
            if self.collect_distinct {
                self.distinct_values
                    .insert(HashableValue::from(&Value::Integer(v)));
            }
        }
    }

    /// Accumulate a contiguous `f64` slice, optionally computing Welford/sample/distinct stats.
    #[inline]
    pub fn accumulate_float64_slice(&mut self, values: &[f64], needs_welford: bool) {
        if !needs_welford && !self.collect_samples && !self.collect_distinct {
            self.accumulate_floats(values, None);
            return;
        }
        self.has_float = true;
        for &f in values {
            self.count_all += 1;
            self.count += 1;
            self.sum_f += f;
            self.min_f = Some(self.min_f.map_or(f, |curr| curr.min(f)));
            self.max_f = Some(self.max_f.map_or(f, |curr| curr.max(f)));
            self.push_numeric_stat(f);
            if self.collect_distinct {
                self.distinct_values
                    .insert(HashableValue::from(&Value::Real(f)));
            }
        }
    }

    /// Merge another thread/partition's partial accumulator into `self` using Chan's parallel Welford variance merge.
    pub fn merge(&mut self, other: &Self) {
        if other.count_all == 0 && other.count == 0 {
            return;
        }
        self.count_all += other.count_all;
        self.count += other.count;
        self.sum_i = self.sum_i.wrapping_add(other.sum_i);
        self.sum_f += other.sum_f;
        self.has_float |= other.has_float;

        if let Some(om) = other.min_i {
            self.min_i = Some(self.min_i.map_or(om, |curr| curr.min(om)));
        }
        if let Some(ox) = other.max_i {
            self.max_i = Some(self.max_i.map_or(ox, |curr| curr.max(ox)));
        }
        if let Some(om) = other.min_f {
            self.min_f = Some(self.min_f.map_or(om, |curr| curr.min(om)));
        }
        if let Some(ox) = other.max_f {
            self.max_f = Some(self.max_f.map_or(ox, |curr| curr.max(ox)));
        }
        if let Some(ref os) = other.min_str
            && self.min_str.as_ref().is_none_or(|curr| os < curr)
        {
            self.min_str = Some(os.clone());
        }
        if let Some(ref os) = other.max_str
            && self.max_str.as_ref().is_none_or(|curr| os > curr)
        {
            self.max_str = Some(os.clone());
        }

        if other.numeric_count > 0 {
            if self.numeric_count == 0 {
                self.numeric_count = other.numeric_count;
                self.mean = other.mean;
                self.m2 = other.m2;
            } else {
                let n_a = self.numeric_count as f64;
                let n_b = other.numeric_count as f64;
                let total = n_a + n_b;
                let delta = other.mean - self.mean;
                self.mean += delta * (n_b / total);
                self.m2 += other.m2 + delta * delta * (n_a * n_b / total);
                self.numeric_count += other.numeric_count;
            }
        }

        if self.collect_samples && !other.samples.is_empty() {
            self.samples.extend_from_slice(&other.samples);
        }
        if self.collect_distinct && !other.distinct_values.is_empty() {
            for val in &other.distinct_values {
                self.distinct_values.insert(val.clone());
            }
        }
    }

    /// Process a batch of 64-bit integers with SIMD/unrolled reduction loop
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
                let (chunks, remainder) = values.as_chunks::<4>();
                let mut s0 = 0i64;
                let mut s1 = 0i64;
                let mut s2 = 0i64;
                let mut s3 = 0i64;

                #[cfg(feature = "simd")]
                {
                    if let Some((first, rest)) = chunks.split_first() {
                        let mut v_sum = wide::i64x4::from(*first);
                        let mut m = first[0].min(first[1]).min(first[2].min(first[3]));
                        let mut mx = first[0].max(first[1]).max(first[2].max(first[3]));
                        for c in rest {
                            let vc = wide::i64x4::from(*c);
                            v_sum = v_sum + vc;
                            m = m.min(c[0].min(c[1]).min(c[2].min(c[3])));
                            mx = mx.max(c[0].max(c[1]).max(c[2].max(c[3])));
                        }
                        let [a0, a1, a2, a3] = v_sum.to_array();
                        s0 = a0;
                        s1 = a1;
                        s2 = a2;
                        s3 = a3;
                        self.min_i = Some(self.min_i.map_or(m, |curr| curr.min(m)));
                        self.max_i = Some(self.max_i.map_or(mx, |curr| curr.max(mx)));
                    }
                }
                #[cfg(not(feature = "simd"))]
                {
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

    /// Process a batch of 64-bit floats with SIMD/unrolled reduction loop
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
                let (chunks, remainder) = values.as_chunks::<4>();
                let mut s0 = 0.0f64;
                let mut s1 = 0.0f64;
                let mut s2 = 0.0f64;
                let mut s3 = 0.0f64;

                #[cfg(feature = "simd")]
                {
                    if let Some((first, rest)) = chunks.split_first() {
                        let mut v_sum = wide::f64x4::from(*first);
                        let mut v_min = v_sum;
                        let mut v_max = v_sum;
                        for c in rest {
                            let vc = wide::f64x4::from(*c);
                            v_sum += vc;
                            v_min = v_min.min(vc);
                            v_max = v_max.max(vc);
                        }
                        let [a0, a1, a2, a3] = v_sum.to_array();
                        s0 = a0;
                        s1 = a1;
                        s2 = a2;
                        s3 = a3;
                        let mins = v_min.to_array();
                        let maxs = v_max.to_array();
                        let m = mins[0].min(mins[1]).min(mins[2].min(mins[3]));
                        let mx = maxs[0].max(maxs[1]).max(maxs[2].max(maxs[3]));
                        self.min_f = Some(self.min_f.map_or(m, |curr| curr.min(m)));
                        self.max_f = Some(self.max_f.map_or(mx, |curr| curr.max(mx)));
                    }
                }
                #[cfg(not(feature = "simd"))]
                {
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

    /// Process a single TapirusDB `Value` without iterator wrapper overhead
    #[inline]
    pub fn accumulate_value(&mut self, v: &Value) {
        self.count_all += 1;
        match v {
            Value::Integer(i) => {
                self.count += 1;
                self.sum_i = self.sum_i.wrapping_add(*i);
                let f = *i as f64;
                self.sum_f += f;
                self.min_i = Some(self.min_i.map_or(*i, |curr| curr.min(*i)));
                self.max_i = Some(self.max_i.map_or(*i, |curr| curr.max(*i)));
                self.push_numeric_stat(f);
                if self.collect_distinct {
                    self.distinct_values.insert(HashableValue::from(v));
                }
            }
            Value::Real(f) => {
                self.count += 1;
                self.has_float = true;
                self.sum_f += *f;
                self.min_f = Some(self.min_f.map_or(*f, |curr| curr.min(*f)));
                self.max_f = Some(self.max_f.map_or(*f, |curr| curr.max(*f)));
                self.push_numeric_stat(*f);
                if self.collect_distinct {
                    self.distinct_values.insert(HashableValue::from(v));
                }
            }
            Value::Text(s) => {
                self.count += 1;
                if self.min_str.as_ref().is_none_or(|curr| s < curr) {
                    self.min_str = Some(s.clone());
                }
                if self.max_str.as_ref().is_none_or(|curr| s > curr) {
                    self.max_str = Some(s.clone());
                }
                if self.collect_distinct {
                    self.distinct_values.insert(HashableValue::from(v));
                }
            }
            Value::Null => {}
            _ => {
                self.count += 1;
                if self.collect_distinct {
                    self.distinct_values.insert(HashableValue::from(v));
                }
            }
        }
    }

    /// Process arbitrary TapirusDB Value iterator
    pub fn accumulate_values<'a, I>(&mut self, values: I)
    where
        I: IntoIterator<Item = &'a Value>,
    {
        for v in values {
            self.accumulate_value(v);
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

    /// Finalize COUNT(DISTINCT col) aggregate
    pub fn finalize_count_distinct(&self) -> Value {
        Value::Integer(self.distinct_values.len() as i64)
    }

    /// Finalize VARIANCE (`VAR_SAMP` or `VAR_POP`) aggregate using Welford state
    pub fn finalize_variance(&self, is_sample: bool) -> Value {
        if self.numeric_count == 0 || (is_sample && self.numeric_count < 2) {
            return Value::Null;
        }
        let denom = if is_sample {
            (self.numeric_count - 1) as f64
        } else {
            self.numeric_count as f64
        };
        Value::Real(self.m2 / denom)
    }

    /// Finalize STDDEV (`STDDEV_SAMP` or `STDDEV_POP`) aggregate using Welford state
    pub fn finalize_stddev(&self, is_sample: bool) -> Value {
        match self.finalize_variance(is_sample) {
            Value::Real(v) => Value::Real(v.max(0.0).sqrt()),
            _ => Value::Null,
        }
    }

    /// Finalize MEDIAN aggregate (`PERCENTILE(col, 0.5)`) in $O(N)$ expected time
    pub fn finalize_median(&mut self) -> Value {
        self.finalize_percentile(0.5)
    }

    /// Finalize continuous percentile aggregate (`PERCENTILE_CONT(col, q)`) using linear interpolation
    pub fn finalize_percentile(&mut self, quantile: f64) -> Value {
        let n = self.samples.len();
        if n == 0 || !quantile.is_finite() {
            return Value::Null;
        }
        let q = quantile.clamp(0.0, 1.0);
        if n == 1 {
            return Value::Real(self.samples[0]);
        }
        let pos = q * ((n - 1) as f64);
        let lo = pos.floor() as usize;
        let hi = pos.ceil() as usize;
        let frac = pos - (lo as f64);

        let (_, lo_val_ref, _) = self.samples.select_nth_unstable_by(lo, f64::total_cmp);
        let lo_val = *lo_val_ref;
        if lo == hi {
            return Value::Real(lo_val);
        }
        let (_, hi_val_ref, _) = self.samples.select_nth_unstable_by(hi, f64::total_cmp);
        let hi_val = *hi_val_ref;
        Value::Real(lo_val + frac * (hi_val - lo_val))
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

    #[test]
    fn test_vectorized_analytics_stats() {
        let mut acc = VectorizedAccumulator::with_tracking(true, true);
        for v in [10, 20, 20, 30, 100] {
            acc.accumulate_value(&Value::Integer(v));
        }
        assert_eq!(acc.finalize_count_distinct(), Value::Integer(4));
        assert_eq!(acc.finalize_median(), Value::Real(20.0));
        assert_eq!(acc.finalize_percentile(0.25), Value::Real(20.0));
        if let Value::Real(std) = acc.finalize_stddev(true) {
            assert!((std - 36.469165).abs() < 0.01, "got stddev={std}");
        } else {
            panic!("Expected Real stddev");
        }
    }

    #[test]
    fn test_vectorized_parallel_merge() {
        let mut seq = VectorizedAccumulator::with_tracking(true, true);
        let mut left = VectorizedAccumulator::with_tracking(true, true);
        let mut right = VectorizedAccumulator::with_tracking(true, true);

        let left_vals = [10.0, 20.0, 20.0, 35.0];
        let right_vals = [40.0, 55.0, 70.0, 100.0];
        for v in left_vals {
            seq.accumulate_value(&Value::Real(v));
            left.accumulate_value(&Value::Real(v));
        }
        for v in right_vals {
            seq.accumulate_value(&Value::Real(v));
            right.accumulate_value(&Value::Real(v));
        }
        left.merge(&right);

        assert_eq!(left.finalize_count(true), seq.finalize_count(true));
        assert_eq!(left.finalize_sum(), seq.finalize_sum());
        assert_eq!(left.finalize_avg(), seq.finalize_avg());
        assert_eq!(left.finalize_min(), seq.finalize_min());
        assert_eq!(left.finalize_max(), seq.finalize_max());
        assert_eq!(
            left.finalize_count_distinct(),
            seq.finalize_count_distinct()
        );
        assert_eq!(left.finalize_median(), seq.finalize_median());
        match (left.finalize_stddev(true), seq.finalize_stddev(true)) {
            (Value::Real(a), Value::Real(b)) => assert!((a - b).abs() < 1e-9),
            other => panic!("Unexpected stddev pair: {other:?}"),
        }
    }
}
