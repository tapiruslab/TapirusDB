//! Vectorized Columnar OLAP & Parallel Parquet Pushdown Engine.
//!
//! Provides high-throughput Big Data analytics directly over Apache Parquet files
//! and Hive-partitioned Data Lake directories without materializing unneeded columns
//! or irrelevant row groups:
//! - Hive partition directory pruning (`year=2026/...`) before file open
//! - Parquet row-group Zone-Map (`min`/`max`/`null_count`) pruning
//! - Column Projection Pushdown via `parquet::arrow::ProjectionMask`
//! - Footer-only metadata fast path for unpredicated `COUNT(*)`
//! - Parallel multi-file execution (`std::thread::scope`) directly over typed Arrow
//!   columnar arrays, merging `VectorizedAccumulator` states without per-row `Row` allocation
//!
//! Gated behind `feature = "lakehouse"` and `feature = "olap"` (`bigdata`).
//! Complies strictly with `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

use crate::error::{Error, Result};
use crate::lakehouse::csv_stream::{parse_field_inferred, sanitize_column_names};
use crate::lakehouse::parquet_io::extract_arrow_cell;
use crate::lakehouse::{ResolvedLakeFile, discover_lake_files};
use crate::sql::executor::{
    ParsedAggSpec, ResolvedWhereExpr, is_aggregate_query, matches_condition, parse_aggregate_spec,
    parse_col_and_alias,
};
use crate::sql::parser::{BinaryOp, OrderByClause, WhereCondition};
use crate::sql::vectorized::VectorizedAccumulator;
use crate::traits::{HashableValue, Row, Value};
use arrow_array::RecordBatch;
use arrow_array::array::{
    Array, BooleanArray, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array,
    LargeStringArray, StringArray, StringViewArray, UInt8Array, UInt16Array, UInt32Array,
    UInt64Array,
};
use arrow_schema::DataType as ArrowDataType;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::metadata::RowGroupMetaData;
use parquet::file::statistics::Statistics;
use std::collections::HashMap;
use std::fs::File;
use std::sync::Arc;
use std::time::Instant;

/// Execution telemetry from a Big Data / Parquet OLAP pushdown scan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParquetOlapStats {
    /// Number of Parquet files opened and scanned after Hive partition pruning.
    pub files_scanned: usize,
    /// Number of Parquet files skipped entirely via Hive partition directory pruning.
    pub files_pruned_hive: usize,
    /// Total Parquet row groups inspected across opened files.
    pub row_groups_total: usize,
    /// Parquet row groups skipped via min/max/null Zone-Map statistics.
    pub row_groups_pruned: usize,
    /// Number of Parquet physical columns projected (after projection pushdown).
    pub columns_projected: usize,
    /// Number of parallel worker threads utilized across files.
    pub parallel_workers: usize,
    /// Number of surviving rows scanned / processed by the analytical kernel.
    pub rows_scanned: usize,
    /// Total wall-clock latency in milliseconds.
    pub elapsed_ms: f64,
}

#[derive(Debug, Clone)]
struct ProjectedColPlan {
    output_name: String,
    source_name: String,
}

/// Evaluate whether a Parquet SQL query can execute via the vectorized OLAP pushdown engine.
///
/// Returns `Ok(None)` when the query uses features better served by the general ephemeral
/// SQL executor (such as scalar expressions in SELECT projections, window functions, etc.).
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_parquet_sql_select(
    path_pattern: &str,
    columns: &[String],
    where_expr: Option<&ResolvedWhereExpr>,
    group_by: Option<&[String]>,
    having: Option<&ResolvedWhereExpr>,
    order_by: Option<&OrderByClause>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Option<(Vec<Row>, ParquetOlapStats)>> {
    let start = Instant::now();
    let all_files = discover_lake_files(path_pattern, Some(".parquet"))?;
    if all_files.is_empty() {
        return Ok(None);
    }

    // 1. Validate that projections/GROUP BY match simple column or aggregate forms supported here.
    let has_agg = is_aggregate_query(columns);
    let mut agg_plans: Vec<Option<ParsedAggSpec>> = Vec::with_capacity(columns.len());
    let mut proj_plans: Vec<ProjectedColPlan> = Vec::new();
    let mut referenced_cols: Vec<String> = Vec::new();

    if has_agg || group_by.is_some() {
        for col in columns {
            if let Some(spec) = parse_aggregate_spec(col) {
                if !spec.is_star {
                    if !is_simple_ident(&spec.arg) {
                        return Ok(None);
                    }
                    push_unique_ci(&mut referenced_cols, &spec.arg);
                }
                agg_plans.push(Some(spec));
            } else {
                let (expr, _alias) = parse_col_and_alias(col);
                let trimmed = expr.trim();
                if !is_simple_ident(trimmed) {
                    return Ok(None);
                }
                if let Some(gb) = group_by {
                    if !gb.iter().any(|g| g.eq_ignore_ascii_case(trimmed)) {
                        return Ok(None);
                    }
                } else {
                    // Non-grouped aggregate query with a bare column is non-standard; fallback.
                    return Ok(None);
                }
                push_unique_ci(&mut referenced_cols, trimmed);
                agg_plans.push(None);
            }
        }
        if let Some(gb) = group_by {
            for g in gb {
                if !is_simple_ident(g) {
                    return Ok(None);
                }
                push_unique_ci(&mut referenced_cols, g);
            }
        }
    } else {
        for col in columns {
            let (expr, alias) = parse_col_and_alias(col);
            let trimmed = expr.trim();
            if trimmed == "*" {
                // Full-width projection needed; clear list sentinel.
                referenced_cols.clear();
                proj_plans.clear();
                break;
            }
            if !is_simple_ident(trimmed) {
                return Ok(None);
            }
            push_unique_ci(&mut referenced_cols, trimmed);
            proj_plans.push(ProjectedColPlan {
                output_name: alias.unwrap_or(trimmed).to_string(),
                source_name: trimmed.to_string(),
            });
        }
    }

    let select_all = columns.is_empty()
        || (!has_agg && group_by.is_none() && columns.iter().any(|c| c.trim() == "*"));

    if let Some(expr) = where_expr
        && !collect_where_columns(expr, &mut referenced_cols)
    {
        return Ok(None);
    }

    if let Some(ord) = order_by
        && !has_agg
        && group_by.is_none()
        && !select_all
    {
        if !is_simple_ident(&ord.column) {
            return Ok(None);
        }
        // Only project order_by column if it's not an alias defined in SELECT.
        let is_alias = proj_plans
            .iter()
            .any(|p| p.output_name.eq_ignore_ascii_case(&ord.column));
        if !is_alias {
            push_unique_ci(&mut referenced_cols, &ord.column);
        }
    }

    // 2. Hive partition pruning before opening any Parquet files.
    let mut active_files = Vec::with_capacity(all_files.len());
    let mut files_pruned_hive = 0usize;
    for file in all_files {
        if let Some(expr) = where_expr
            && !hive_partitions_can_match(&file.partitions, expr)
        {
            files_pruned_hive += 1;
            continue;
        }
        active_files.push(file);
    }

    let mut stats = ParquetOlapStats {
        files_pruned_hive,
        ..Default::default()
    };

    if active_files.is_empty() {
        stats.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        if has_agg && group_by.is_none() {
            let out = build_single_agg_row(
                columns,
                &agg_plans,
                vec![VectorizedAccumulator::new(); columns.len()],
            );
            return Ok(Some((vec![out], stats)));
        }
        return Ok(Some((Vec::new(), stats)));
    }

    // 3. Metadata-only COUNT(*) fast path (unfiltered, ungrouped).
    if has_agg
        && group_by.is_none()
        && where_expr.is_none()
        && having.is_none()
        && !agg_plans.is_empty()
        && agg_plans
            .iter()
            .all(|p| matches!(p, Some(spec) if spec.func == "COUNT" && spec.is_star))
    {
        let mut total_rows = 0i64;
        for f in &active_files {
            let file = File::open(&f.path)?;
            let builder = ParquetRecordBatchReaderBuilder::try_new(file)
                .map_err(|e| Error::Corrupted(format!("Parquet open error: {e}")))?;
            let meta = builder.metadata();
            stats.files_scanned += 1;
            stats.row_groups_total += meta.num_row_groups();
            for rg in meta.row_groups() {
                total_rows += rg.num_rows();
            }
        }
        stats.rows_scanned = total_rows as usize;
        stats.parallel_workers = 1;
        stats.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let out_cols: Vec<String> = agg_plans
            .iter()
            .map(|p| p.as_ref().unwrap().output_name.clone())
            .collect();
        let vals = vec![Value::Integer(total_rows); out_cols.len()];
        return Ok(Some((vec![Row::new(out_cols, vals)], stats)));
    }

    // 4. Execute across Parquet files in parallel (or sequentially for unordered LIMIT scans).
    let max_workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(active_files.len())
        .max(1);

    if has_agg && group_by.is_none() {
        let agg_specs: Vec<ParsedAggSpec> = agg_plans
            .iter()
            .map(|p| p.clone().expect("validated aggregate specs"))
            .collect();
        let (merged_accs, scan_stats) = execute_parallel_agg(
            &active_files,
            &agg_specs,
            &referenced_cols,
            where_expr,
            max_workers,
        )?;
        stats.files_scanned = scan_stats.files_scanned;
        stats.row_groups_total = scan_stats.row_groups_total;
        stats.row_groups_pruned = scan_stats.row_groups_pruned;
        stats.columns_projected = scan_stats.columns_projected;
        stats.parallel_workers = scan_stats.parallel_workers;
        stats.rows_scanned = scan_stats.rows_scanned;
        stats.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

        let out_row = build_single_agg_row(columns, &agg_plans, merged_accs);
        return Ok(Some((vec![out_row], stats)));
    }

    if let Some(gb_cols) = group_by {
        let (rows, scan_stats) = execute_parallel_group_by(
            &active_files,
            columns,
            &agg_plans,
            gb_cols,
            &referenced_cols,
            where_expr,
            having,
            order_by,
            limit,
            offset,
            max_workers,
        )?;
        stats.files_scanned = scan_stats.files_scanned;
        stats.row_groups_total = scan_stats.row_groups_total;
        stats.row_groups_pruned = scan_stats.row_groups_pruned;
        stats.columns_projected = scan_stats.columns_projected;
        stats.parallel_workers = scan_stats.parallel_workers;
        stats.rows_scanned = scan_stats.rows_scanned;
        stats.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        return Ok(Some((rows, stats)));
    }

    // 5. Plain SELECT / WHERE / Projection / ORDER BY / LIMIT scan with column pushdown & pruning.
    let (rows, scan_stats) = execute_projected_scan(
        &active_files,
        select_all,
        &proj_plans,
        &referenced_cols,
        where_expr,
        order_by,
        limit,
        offset,
        max_workers,
    )?;
    stats.files_scanned = scan_stats.files_scanned;
    stats.row_groups_total = scan_stats.row_groups_total;
    stats.row_groups_pruned = scan_stats.row_groups_pruned;
    stats.columns_projected = scan_stats.columns_projected;
    stats.parallel_workers = scan_stats.parallel_workers;
    stats.rows_scanned = scan_stats.rows_scanned;
    stats.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    Ok(Some((rows, stats)))
}

fn build_single_agg_row(
    columns: &[String],
    agg_plans: &[Option<ParsedAggSpec>],
    mut accs: Vec<VectorizedAccumulator>,
) -> Row {
    let mut out_cols = Vec::with_capacity(columns.len());
    let mut out_vals = Vec::with_capacity(columns.len());
    for (i, plan_opt) in agg_plans.iter().enumerate() {
        if let Some(spec) = plan_opt {
            out_cols.push(spec.output_name.clone());
            out_vals.push(spec.finalize(&mut accs[i]));
        }
    }
    Row::new(out_cols, out_vals)
}

fn is_simple_ident(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

fn push_unique_ci(list: &mut Vec<String>, name: &str) {
    let t = name.trim();
    if !t.is_empty() && !list.iter().any(|existing| existing.eq_ignore_ascii_case(t)) {
        list.push(t.to_string());
    }
}

fn collect_where_columns(expr: &ResolvedWhereExpr, cols: &mut Vec<String>) -> bool {
    match expr {
        ResolvedWhereExpr::Boolean(_) => true,
        ResolvedWhereExpr::Condition(c) => {
            if !is_simple_ident(&c.column) {
                return false;
            }
            push_unique_ci(cols, &c.column);
            true
        }
        ResolvedWhereExpr::InList { column, .. } | ResolvedWhereExpr::InSet { column, .. } => {
            if !is_simple_ident(column) {
                return false;
            }
            push_unique_ci(cols, column);
            true
        }
        ResolvedWhereExpr::And(l, r) | ResolvedWhereExpr::Or(l, r) => {
            collect_where_columns(l, cols) && collect_where_columns(r, cols)
        }
    }
}

fn eval_hive_expr_partial(part_map: &[(String, Value)], expr: &ResolvedWhereExpr) -> Option<bool> {
    match expr {
        ResolvedWhereExpr::Boolean(b) => Some(*b),
        ResolvedWhereExpr::Condition(c) => {
            let (_, pval) = part_map
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&c.column))?;
            if matches!(c.op, BinaryOp::Equals) {
                // Compare both typed inference and canonical string forms for Hive directories.
                let typed = matches_condition(pval, &c.op, &c.value);
                if typed {
                    return Some(true);
                }
                return Some(pval.to_string() == c.value.to_string());
            }
            Some(matches_condition(pval, &c.op, &c.value))
        }
        ResolvedWhereExpr::InList {
            column,
            values,
            negated,
        }
        | ResolvedWhereExpr::InSet {
            column,
            values,
            negated,
            ..
        } => {
            let (_, pval) = part_map
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(column))?;
            let in_list = values
                .iter()
                .any(|v| pval.compare(v).is_eq() || pval.to_string() == v.to_string());
            Some(if *negated { !in_list } else { in_list })
        }
        ResolvedWhereExpr::And(l, r) => {
            let lv = eval_hive_expr_partial(part_map, l);
            let rv = eval_hive_expr_partial(part_map, r);
            match (lv, rv) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            }
        }
        ResolvedWhereExpr::Or(l, r) => {
            let lv = eval_hive_expr_partial(part_map, l);
            let rv = eval_hive_expr_partial(part_map, r);
            match (lv, rv) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            }
        }
    }
}

fn hive_partitions_can_match(partitions: &[(String, String)], expr: &ResolvedWhereExpr) -> bool {
    if partitions.is_empty() {
        return true;
    }
    let parsed: Vec<(String, Value)> = partitions
        .iter()
        .map(|(k, v)| (k.clone(), parse_field_inferred(v)))
        .collect();
    eval_hive_expr_partial(&parsed, expr).unwrap_or(true)
}

struct OpenedParquet {
    file: File,
    clean_cols: Vec<String>,
    root_indices: Vec<usize>,
    projected_clean_cols: Vec<String>,
    hive_vals: Vec<(String, Value)>,
    kept_row_groups: Vec<usize>,
    row_groups_total: usize,
    row_groups_pruned: usize,
}

fn open_parquet_with_pushdown(
    lake_file: &ResolvedLakeFile,
    select_all: bool,
    referenced_cols: &[String],
    where_expr: Option<&ResolvedWhereExpr>,
) -> Result<OpenedParquet> {
    let file = File::open(&lake_file.path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file.try_clone()?)
        .map_err(|e| Error::Corrupted(format!("Parquet open error: {e}")))?;

    let arrow_schema = builder.schema();
    let raw_names: Vec<String> = arrow_schema
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    let clean_cols = sanitize_column_names(&raw_names);

    let parquet_schema = builder.parquet_schema();
    let root_fields = parquet_schema.root_schema().get_fields();

    let mut root_indices = Vec::new();
    let mut projected_clean_cols = Vec::new();

    if select_all || referenced_cols.is_empty() {
        for (idx, clean) in clean_cols.iter().enumerate() {
            if let Some(root_idx) = root_fields
                .iter()
                .position(|f| f.name() == raw_names[idx].as_str())
            {
                root_indices.push(root_idx);
                projected_clean_cols.push(clean.clone());
            }
        }
    } else {
        for wanted in referenced_cols {
            if let Some(idx) = clean_cols
                .iter()
                .position(|c| c.eq_ignore_ascii_case(wanted))
                && let Some(root_idx) = root_fields
                    .iter()
                    .position(|f| f.name() == raw_names[idx].as_str())
                && !root_indices.contains(&root_idx)
            {
                root_indices.push(root_idx);
                projected_clean_cols.push(clean_cols[idx].clone());
            }
        }
    }

    // Keep clean_cols order matching Parquet schema projection output order.
    let mut paired: Vec<(usize, String)> =
        root_indices.into_iter().zip(projected_clean_cols).collect();
    paired.sort_by_key(|(idx, _)| *idx);
    let root_indices: Vec<usize> = paired.iter().map(|(i, _)| *i).collect();
    let projected_clean_cols: Vec<String> = paired.into_iter().map(|(_, c)| c).collect();

    let hive_vals: Vec<(String, Value)> = lake_file
        .partitions
        .iter()
        .map(|(k, v)| (k.clone(), parse_field_inferred(v)))
        .collect();

    let meta = builder.metadata();
    let row_groups_total = meta.num_row_groups();
    let mut kept_row_groups = Vec::with_capacity(row_groups_total);
    let mut row_groups_pruned = 0usize;

    if let Some(expr) = where_expr {
        for (rg_idx, rg) in meta.row_groups().iter().enumerate() {
            if row_group_can_match(rg, &clean_cols, &hive_vals, expr) {
                kept_row_groups.push(rg_idx);
            } else {
                row_groups_pruned += 1;
            }
        }
    } else {
        kept_row_groups.extend(0..row_groups_total);
    }

    Ok(OpenedParquet {
        file,
        clean_cols,
        root_indices,
        projected_clean_cols,
        hive_vals,
        kept_row_groups,
        row_groups_total,
        row_groups_pruned,
    })
}

fn row_group_can_match(
    rg: &RowGroupMetaData,
    clean_cols: &[String],
    hive_vals: &[(String, Value)],
    expr: &ResolvedWhereExpr,
) -> bool {
    match expr {
        ResolvedWhereExpr::Boolean(b) => *b,
        ResolvedWhereExpr::And(l, r) => {
            row_group_can_match(rg, clean_cols, hive_vals, l)
                && row_group_can_match(rg, clean_cols, hive_vals, r)
        }
        ResolvedWhereExpr::Or(l, r) => {
            row_group_can_match(rg, clean_cols, hive_vals, l)
                || row_group_can_match(rg, clean_cols, hive_vals, r)
        }
        ResolvedWhereExpr::Condition(cond) => {
            if let Some((_, pval)) = hive_vals
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&cond.column))
            {
                return matches_condition(pval, &cond.op, &cond.value)
                    || (matches!(cond.op, BinaryOp::Equals)
                        && pval.to_string() == cond.value.to_string());
            }
            let Some(col_idx) = clean_cols
                .iter()
                .position(|c| c.eq_ignore_ascii_case(&cond.column))
            else {
                return true;
            };
            if col_idx >= rg.num_columns() {
                return true;
            }
            let Some(stats) = rg.column(col_idx).statistics() else {
                return true;
            };
            let rg_rows = rg.num_rows().max(0) as u64;
            condition_can_match_stats(stats, rg_rows, cond)
        }
        ResolvedWhereExpr::InList {
            column,
            values,
            negated,
        }
        | ResolvedWhereExpr::InSet {
            column,
            values,
            negated,
            ..
        } => {
            if *negated || values.is_empty() {
                return true;
            }
            let Some(col_idx) = clean_cols
                .iter()
                .position(|c| c.eq_ignore_ascii_case(column))
            else {
                return true;
            };
            if col_idx >= rg.num_columns() {
                return true;
            }
            let Some(stats) = rg.column(col_idx).statistics() else {
                return true;
            };
            let rg_rows = rg.num_rows().max(0) as u64;
            values.iter().any(|v| {
                condition_can_match_stats(
                    stats,
                    rg_rows,
                    &WhereCondition {
                        column: column.clone(),
                        op: BinaryOp::Equals,
                        value: v.clone(),
                    },
                )
            })
        }
    }
}

fn stats_bounds_as_values(stats: &Statistics) -> Option<(Value, Value)> {
    match stats {
        Statistics::Int32(s) => {
            let min = s.min_opt()?;
            let max = s.max_opt()?;
            Some((Value::Integer(*min as i64), Value::Integer(*max as i64)))
        }
        Statistics::Int64(s) => {
            let min = s.min_opt()?;
            let max = s.max_opt()?;
            Some((Value::Integer(*min), Value::Integer(*max)))
        }
        Statistics::Float(s) => {
            let min = s.min_opt()?;
            let max = s.max_opt()?;
            Some((Value::Real(*min as f64), Value::Real(*max as f64)))
        }
        Statistics::Double(s) => {
            let min = s.min_opt()?;
            let max = s.max_opt()?;
            Some((Value::Real(*min), Value::Real(*max)))
        }
        Statistics::ByteArray(s) => {
            let min = s.min_opt()?.as_utf8().ok()?;
            let max = s.max_opt()?.as_utf8().ok()?;
            Some((Value::Text(min.to_string()), Value::Text(max.to_string())))
        }
        _ => None,
    }
}

fn condition_can_match_stats(stats: &Statistics, rg_rows: u64, cond: &WhereCondition) -> bool {
    let null_count = stats.null_count_opt();
    match cond.op {
        BinaryOp::IsNull => {
            if null_count == Some(0) {
                return false;
            }
            return true;
        }
        BinaryOp::IsNotNull => {
            if let Some(nc) = null_count
                && nc >= rg_rows
                && rg_rows > 0
            {
                return false;
            }
            return true;
        }
        _ => {}
    }

    let Some((rg_min, rg_max)) = stats_bounds_as_values(stats) else {
        return true;
    };

    match cond.op {
        BinaryOp::Equals => {
            !cond.value.compare(&rg_min).is_lt() && !cond.value.compare(&rg_max).is_gt()
        }
        BinaryOp::NotEquals => {
            if null_count == Some(0) && rg_min.compare(&rg_max).is_eq() {
                !rg_min.compare(&cond.value).is_eq()
            } else {
                true
            }
        }
        BinaryOp::GreaterThan => rg_max.compare(&cond.value).is_gt(),
        BinaryOp::GreaterOrEqual => !rg_max.compare(&cond.value).is_lt(),
        BinaryOp::LessThan => rg_min.compare(&cond.value).is_lt(),
        BinaryOp::LessOrEqual => !rg_min.compare(&cond.value).is_gt(),
        _ => true,
    }
}

enum ArrowColRef<'a> {
    Projected(usize),
    Hive(Value),
    Missing(std::marker::PhantomData<&'a ()>),
}

fn resolve_batch_col<'a>(
    name: &str,
    batch_cols: &[String],
    hive_vals: &[(String, Value)],
) -> ArrowColRef<'a> {
    if let Some(idx) = batch_cols.iter().position(|c| c.eq_ignore_ascii_case(name)) {
        ArrowColRef::Projected(idx)
    } else if let Some((_, v)) = hive_vals.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
        ArrowColRef::Hive(v.clone())
    } else {
        ArrowColRef::Missing(std::marker::PhantomData)
    }
}

fn read_arrow_cell_fast(
    batch: &RecordBatch,
    col_ref: &ArrowColRef<'_>,
    row_idx: usize,
) -> Option<Value> {
    match col_ref {
        ArrowColRef::Projected(idx) => {
            Some(extract_arrow_cell(batch.column(*idx).as_ref(), row_idx))
        }
        ArrowColRef::Hive(v) => Some(v.clone()),
        ArrowColRef::Missing(_) => None,
    }
}

#[inline]
fn eval_batch_cond_at(
    batch: &RecordBatch,
    batch_cols: &[String],
    hive_vals: &[(String, Value)],
    cond: &WhereCondition,
    row_idx: usize,
) -> bool {
    let col_ref = resolve_batch_col(&cond.column, batch_cols, hive_vals);
    match col_ref {
        ArrowColRef::Projected(idx) => {
            let col = batch.column(idx);
            if col.is_null(row_idx) {
                return matches!(cond.op, BinaryOp::IsNull);
            }
            if matches!(cond.op, BinaryOp::IsNotNull) {
                return true;
            }
            if matches!(cond.op, BinaryOp::IsNull) {
                return false;
            }
            // Fast primitive paths avoiding allocation for numeric comparisons
            match (col.data_type(), &cond.value) {
                (ArrowDataType::Int64, Value::Integer(rhs)) => {
                    let v = col
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .unwrap()
                        .value(row_idx);
                    return compare_ord_op(v.cmp(rhs), &cond.op);
                }
                (ArrowDataType::Int32, Value::Integer(rhs)) => {
                    let v = col
                        .as_any()
                        .downcast_ref::<Int32Array>()
                        .unwrap()
                        .value(row_idx) as i64;
                    return compare_ord_op(v.cmp(rhs), &cond.op);
                }
                (ArrowDataType::Float64, Value::Real(rhs)) => {
                    let v = col
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .unwrap()
                        .value(row_idx);
                    return compare_ord_op(v.total_cmp(rhs), &cond.op);
                }
                (ArrowDataType::Float64, Value::Integer(rhs)) => {
                    let v = col
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .unwrap()
                        .value(row_idx);
                    return compare_ord_op(v.total_cmp(&(*rhs as f64)), &cond.op);
                }
                (ArrowDataType::Utf8, Value::Text(rhs)) => {
                    let v = col
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .unwrap()
                        .value(row_idx);
                    if let Some(res) = compare_str_op(v, rhs, &cond.op) {
                        return res;
                    }
                }
                _ => {}
            }
            let val = extract_arrow_cell(col.as_ref(), row_idx);
            matches_condition(&val, &cond.op, &cond.value)
        }
        ArrowColRef::Hive(v) => matches_condition(&v, &cond.op, &cond.value),
        ArrowColRef::Missing(_) => false,
    }
}

#[inline]
fn compare_ord_op(ord: std::cmp::Ordering, op: &BinaryOp) -> bool {
    match op {
        BinaryOp::Equals => ord.is_eq(),
        BinaryOp::NotEquals => !ord.is_eq(),
        BinaryOp::GreaterThan => ord.is_gt(),
        BinaryOp::GreaterOrEqual => !ord.is_lt(),
        BinaryOp::LessThan => ord.is_lt(),
        BinaryOp::LessOrEqual => !ord.is_gt(),
        _ => false,
    }
}

#[inline]
fn compare_str_op(lhs: &str, rhs: &str, op: &BinaryOp) -> Option<bool> {
    match op {
        BinaryOp::Equals => Some(lhs == rhs),
        BinaryOp::NotEquals => Some(lhs != rhs),
        BinaryOp::GreaterThan => Some(lhs > rhs),
        BinaryOp::GreaterOrEqual => Some(lhs >= rhs),
        BinaryOp::LessThan => Some(lhs < rhs),
        BinaryOp::LessOrEqual => Some(lhs <= rhs),
        _ => None,
    }
}

fn eval_batch_where_at(
    batch: &RecordBatch,
    batch_cols: &[String],
    hive_vals: &[(String, Value)],
    expr: &ResolvedWhereExpr,
    row_idx: usize,
) -> bool {
    match expr {
        ResolvedWhereExpr::Boolean(b) => *b,
        ResolvedWhereExpr::Condition(c) => {
            eval_batch_cond_at(batch, batch_cols, hive_vals, c, row_idx)
        }
        ResolvedWhereExpr::And(l, r) => {
            eval_batch_where_at(batch, batch_cols, hive_vals, l, row_idx)
                && eval_batch_where_at(batch, batch_cols, hive_vals, r, row_idx)
        }
        ResolvedWhereExpr::Or(l, r) => {
            eval_batch_where_at(batch, batch_cols, hive_vals, l, row_idx)
                || eval_batch_where_at(batch, batch_cols, hive_vals, r, row_idx)
        }
        ResolvedWhereExpr::InSet {
            column,
            lookup,
            negated,
            ..
        } => {
            let col_ref = resolve_batch_col(column, batch_cols, hive_vals);
            let Some(val) = read_arrow_cell_fast(batch, &col_ref, row_idx) else {
                return false;
            };
            if val.is_null() {
                return false;
            }
            let in_set = lookup.contains(&val);
            if *negated {
                !lookup.has_null && !in_set
            } else {
                in_set
            }
        }
        ResolvedWhereExpr::InList {
            column,
            values,
            negated,
        } => {
            let col_ref = resolve_batch_col(column, batch_cols, hive_vals);
            let Some(val) = read_arrow_cell_fast(batch, &col_ref, row_idx) else {
                return false;
            };
            if val.is_null() {
                return false;
            }
            let has_null = values.iter().any(|v| v.is_null());
            let in_list = values.iter().any(|target| val.compare(target).is_eq());
            if *negated {
                if has_null { false } else { !in_list }
            } else {
                in_list
            }
        }
    }
}

#[inline]
fn accumulate_arrow_cell(acc: &mut VectorizedAccumulator, col: &dyn Array, row_idx: usize) {
    if col.is_null(row_idx) {
        acc.count_all += 1;
        return;
    }
    match col.data_type() {
        ArrowDataType::Int64 => {
            let v = col
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .value(row_idx);
            acc.accumulate_value(&Value::Integer(v));
        }
        ArrowDataType::Int32 => {
            let v = col
                .as_any()
                .downcast_ref::<Int32Array>()
                .unwrap()
                .value(row_idx) as i64;
            acc.accumulate_value(&Value::Integer(v));
        }
        ArrowDataType::Float64 => {
            let v = col
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .value(row_idx);
            acc.accumulate_value(&Value::Real(v));
        }
        ArrowDataType::Float32 => {
            let v = col
                .as_any()
                .downcast_ref::<Float32Array>()
                .unwrap()
                .value(row_idx) as f64;
            acc.accumulate_value(&Value::Real(v));
        }
        ArrowDataType::Boolean => {
            let v = if col
                .as_any()
                .downcast_ref::<BooleanArray>()
                .unwrap()
                .value(row_idx)
            {
                1
            } else {
                0
            };
            acc.accumulate_value(&Value::Integer(v));
        }
        ArrowDataType::Int8 => {
            let v = col
                .as_any()
                .downcast_ref::<Int8Array>()
                .unwrap()
                .value(row_idx) as i64;
            acc.accumulate_value(&Value::Integer(v));
        }
        ArrowDataType::Int16 => {
            let v = col
                .as_any()
                .downcast_ref::<Int16Array>()
                .unwrap()
                .value(row_idx) as i64;
            acc.accumulate_value(&Value::Integer(v));
        }
        ArrowDataType::UInt8 => {
            let v = col
                .as_any()
                .downcast_ref::<UInt8Array>()
                .unwrap()
                .value(row_idx) as i64;
            acc.accumulate_value(&Value::Integer(v));
        }
        ArrowDataType::UInt16 => {
            let v = col
                .as_any()
                .downcast_ref::<UInt16Array>()
                .unwrap()
                .value(row_idx) as i64;
            acc.accumulate_value(&Value::Integer(v));
        }
        ArrowDataType::UInt32 => {
            let v = col
                .as_any()
                .downcast_ref::<UInt32Array>()
                .unwrap()
                .value(row_idx) as i64;
            acc.accumulate_value(&Value::Integer(v));
        }
        ArrowDataType::UInt64 => {
            let v = col
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(row_idx) as i64;
            acc.accumulate_value(&Value::Integer(v));
        }
        ArrowDataType::Utf8 => {
            let s = col
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .value(row_idx);
            acc.accumulate_value(&Value::Text(s.to_string()));
        }
        ArrowDataType::LargeUtf8 => {
            let s = col
                .as_any()
                .downcast_ref::<LargeStringArray>()
                .unwrap()
                .value(row_idx);
            acc.accumulate_value(&Value::Text(s.to_string()));
        }
        ArrowDataType::Utf8View => {
            let s = col
                .as_any()
                .downcast_ref::<StringViewArray>()
                .unwrap()
                .value(row_idx);
            acc.accumulate_value(&Value::Text(s.to_string()));
        }
        _ => {
            let val = extract_arrow_cell(col, row_idx);
            acc.accumulate_value(&val);
        }
    }
}

fn agg_needs_welford(spec: &ParsedAggSpec) -> bool {
    matches!(
        spec.func.as_str(),
        "STDDEV" | "STDDEV_SAMP" | "STDDEV_POP" | "STDEV" | "VARIANCE" | "VAR_SAMP" | "VAR_POP"
    )
}

fn execute_agg_file(
    lake_file: &ResolvedLakeFile,
    agg_specs: &[ParsedAggSpec],
    referenced_cols: &[String],
    where_expr: Option<&ResolvedWhereExpr>,
) -> Result<(Vec<VectorizedAccumulator>, ParquetOlapStats)> {
    let opened = open_parquet_with_pushdown(lake_file, false, referenced_cols, where_expr)?;
    let mut stats = ParquetOlapStats {
        files_scanned: 1,
        row_groups_total: opened.row_groups_total,
        row_groups_pruned: opened.row_groups_pruned,
        columns_projected: opened.root_indices.len(),
        ..Default::default()
    };

    let mut accs: Vec<VectorizedAccumulator> = agg_specs
        .iter()
        .map(|s| VectorizedAccumulator::with_tracking(s.needs_samples(), s.needs_distinct()))
        .collect();

    if opened.kept_row_groups.is_empty() {
        return Ok((accs, stats));
    }

    let mut builder = ParquetRecordBatchReaderBuilder::try_new(opened.file)
        .map_err(|e| Error::Corrupted(format!("Parquet open error: {e}")))?
        .with_batch_size(8192);

    if !opened.root_indices.is_empty() {
        let mask = ProjectionMask::roots(builder.parquet_schema(), opened.root_indices.clone());
        builder = builder.with_projection(mask);
    } else if !builder
        .parquet_schema()
        .root_schema()
        .get_fields()
        .is_empty()
    {
        // Minimal 1-column mask when only COUNT(*)+Hive partition columns are evaluated.
        let mask = ProjectionMask::roots(builder.parquet_schema(), [0]);
        builder = builder.with_projection(mask);
    }
    if opened.kept_row_groups.len() < opened.row_groups_total {
        builder = builder.with_row_groups(opened.kept_row_groups);
    }

    let reader = builder
        .build()
        .map_err(|e| Error::Corrupted(format!("Parquet reader build error: {e}")))?;

    for batch_res in reader {
        let batch =
            batch_res.map_err(|e| Error::Corrupted(format!("Parquet batch read error: {e}")))?;
        let num_rows = batch.num_rows();
        if num_rows == 0 {
            continue;
        }

        if let Some(expr) = where_expr {
            for r in 0..num_rows {
                if !eval_batch_where_at(
                    &batch,
                    &opened.projected_clean_cols,
                    &opened.hive_vals,
                    expr,
                    r,
                ) {
                    continue;
                }
                stats.rows_scanned += 1;
                for (agg_idx, spec) in agg_specs.iter().enumerate() {
                    if spec.is_star {
                        accs[agg_idx].count_all += 1;
                        accs[agg_idx].count += 1;
                        continue;
                    }
                    match resolve_batch_col(
                        &spec.arg,
                        &opened.projected_clean_cols,
                        &opened.hive_vals,
                    ) {
                        ArrowColRef::Projected(col_idx) => {
                            accumulate_arrow_cell(
                                &mut accs[agg_idx],
                                batch.column(col_idx).as_ref(),
                                r,
                            );
                        }
                        ArrowColRef::Hive(val) => accs[agg_idx].accumulate_value(&val),
                        ArrowColRef::Missing(_) => {}
                    }
                }
            }
        } else {
            stats.rows_scanned += num_rows;
            for (agg_idx, spec) in agg_specs.iter().enumerate() {
                if spec.is_star {
                    accs[agg_idx].count_all += num_rows;
                    accs[agg_idx].count += num_rows;
                    continue;
                }
                match resolve_batch_col(&spec.arg, &opened.projected_clean_cols, &opened.hive_vals)
                {
                    ArrowColRef::Projected(col_idx) => {
                        let col = batch.column(col_idx);
                        let needs_welford = agg_needs_welford(spec);
                        if col.null_count() == 0 {
                            if let Some(i64_arr) = col.as_any().downcast_ref::<Int64Array>() {
                                accs[agg_idx]
                                    .accumulate_int64_slice(i64_arr.values(), needs_welford);
                                continue;
                            }
                            if let Some(f64_arr) = col.as_any().downcast_ref::<Float64Array>() {
                                accs[agg_idx]
                                    .accumulate_float64_slice(f64_arr.values(), needs_welford);
                                continue;
                            }
                        }
                        for r in 0..num_rows {
                            accumulate_arrow_cell(&mut accs[agg_idx], col.as_ref(), r);
                        }
                    }
                    ArrowColRef::Hive(val) => {
                        for _ in 0..num_rows {
                            accs[agg_idx].accumulate_value(&val);
                        }
                    }
                    ArrowColRef::Missing(_) => {}
                }
            }
        }
    }

    Ok((accs, stats))
}

fn execute_parallel_agg(
    files: &[ResolvedLakeFile],
    agg_specs: &[ParsedAggSpec],
    referenced_cols: &[String],
    where_expr: Option<&ResolvedWhereExpr>,
    max_workers: usize,
) -> Result<(Vec<VectorizedAccumulator>, ParquetOlapStats)> {
    let mut final_accs: Vec<VectorizedAccumulator> = agg_specs
        .iter()
        .map(|s| VectorizedAccumulator::with_tracking(s.needs_samples(), s.needs_distinct()))
        .collect();
    let mut agg_stats = ParquetOlapStats {
        parallel_workers: max_workers,
        ..Default::default()
    };

    if max_workers <= 1 || files.len() <= 1 {
        for f in files {
            let (file_accs, fs) = execute_agg_file(f, agg_specs, referenced_cols, where_expr)?;
            for (dst, src) in final_accs.iter_mut().zip(file_accs.iter()) {
                dst.merge(src);
            }
            merge_stats(&mut agg_stats, &fs);
        }
        return Ok((final_accs, agg_stats));
    }

    let chunk_size = files.len().div_ceil(max_workers);
    let results: Vec<Result<(Vec<VectorizedAccumulator>, ParquetOlapStats)>> =
        std::thread::scope(|s| {
            let handles: Vec<_> = files
                .chunks(chunk_size)
                .map(|chunk| {
                    s.spawn(move || {
                        let mut local_accs: Vec<VectorizedAccumulator> = agg_specs
                            .iter()
                            .map(|sp| {
                                VectorizedAccumulator::with_tracking(
                                    sp.needs_samples(),
                                    sp.needs_distinct(),
                                )
                            })
                            .collect();
                        let mut local_stats = ParquetOlapStats::default();
                        for f in chunk {
                            let (fa, fs) =
                                execute_agg_file(f, agg_specs, referenced_cols, where_expr)?;
                            for (dst, src) in local_accs.iter_mut().zip(fa.iter()) {
                                dst.merge(src);
                            }
                            merge_stats(&mut local_stats, &fs);
                        }
                        Ok((local_accs, local_stats))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

    for res in results {
        let (local_accs, local_stats) = res?;
        for (dst, src) in final_accs.iter_mut().zip(local_accs.iter()) {
            dst.merge(src);
        }
        merge_stats(&mut agg_stats, &local_stats);
    }

    Ok((final_accs, agg_stats))
}

struct OlapGroupState {
    keys: Vec<Value>,
    accs: Vec<VectorizedAccumulator>,
}

fn execute_group_by_file(
    lake_file: &ResolvedLakeFile,
    agg_plans: &[Option<ParsedAggSpec>],
    gb_cols: &[String],
    referenced_cols: &[String],
    where_expr: Option<&ResolvedWhereExpr>,
) -> Result<(
    HashMap<Vec<HashableValue>, OlapGroupState>,
    ParquetOlapStats,
)> {
    let opened = open_parquet_with_pushdown(lake_file, false, referenced_cols, where_expr)?;
    let mut stats = ParquetOlapStats {
        files_scanned: 1,
        row_groups_total: opened.row_groups_total,
        row_groups_pruned: opened.row_groups_pruned,
        columns_projected: opened.root_indices.len(),
        ..Default::default()
    };
    let mut groups: HashMap<Vec<HashableValue>, OlapGroupState> = HashMap::new();
    if opened.kept_row_groups.is_empty() {
        return Ok((groups, stats));
    }

    let mut builder = ParquetRecordBatchReaderBuilder::try_new(opened.file)
        .map_err(|e| Error::Corrupted(format!("Parquet open error: {e}")))?
        .with_batch_size(8192);

    if !opened.root_indices.is_empty() {
        let mask = ProjectionMask::roots(builder.parquet_schema(), opened.root_indices.clone());
        builder = builder.with_projection(mask);
    } else if !builder
        .parquet_schema()
        .root_schema()
        .get_fields()
        .is_empty()
    {
        // Minimal 1-column mask when only COUNT(*)+Hive partition columns are evaluated.
        let mask = ProjectionMask::roots(builder.parquet_schema(), [0]);
        builder = builder.with_projection(mask);
    }
    if opened.kept_row_groups.len() < opened.row_groups_total {
        builder = builder.with_row_groups(opened.kept_row_groups);
    }

    let reader = builder
        .build()
        .map_err(|e| Error::Corrupted(format!("Parquet reader build error: {e}")))?;

    let gb_refs: Vec<ArrowColRef<'_>> = gb_cols
        .iter()
        .map(|c| resolve_batch_col(c, &opened.projected_clean_cols, &opened.hive_vals))
        .collect();
    let agg_col_refs: Vec<Option<ArrowColRef<'_>>> = agg_plans
        .iter()
        .map(|p| {
            p.as_ref().and_then(|sp| {
                if sp.is_star {
                    None
                } else {
                    Some(resolve_batch_col(
                        &sp.arg,
                        &opened.projected_clean_cols,
                        &opened.hive_vals,
                    ))
                }
            })
        })
        .collect();

    let mut hash_buf = Vec::with_capacity(gb_cols.len());

    for batch_res in reader {
        let batch =
            batch_res.map_err(|e| Error::Corrupted(format!("Parquet batch read error: {e}")))?;
        let num_rows = batch.num_rows();
        for r in 0..num_rows {
            if let Some(expr) = where_expr
                && !eval_batch_where_at(
                    &batch,
                    &opened.projected_clean_cols,
                    &opened.hive_vals,
                    expr,
                    r,
                )
            {
                continue;
            }
            stats.rows_scanned += 1;

            hash_buf.clear();
            for gr in &gb_refs {
                let v = read_arrow_cell_fast(&batch, gr, r).unwrap_or(Value::Null);
                hash_buf.push(HashableValue::from(&v));
            }

            if let Some(state) = groups.get_mut(&hash_buf) {
                for (slot, plan_opt) in agg_plans.iter().enumerate() {
                    if let Some(spec) = plan_opt {
                        if spec.is_star {
                            state.accs[slot].count_all += 1;
                            state.accs[slot].count += 1;
                        } else if let Some(cref) = &agg_col_refs[slot] {
                            match cref {
                                ArrowColRef::Projected(cidx) => accumulate_arrow_cell(
                                    &mut state.accs[slot],
                                    batch.column(*cidx).as_ref(),
                                    r,
                                ),
                                ArrowColRef::Hive(hv) => state.accs[slot].accumulate_value(hv),
                                ArrowColRef::Missing(_) => {}
                            }
                        }
                    }
                }
            } else {
                let keys: Vec<Value> = gb_refs
                    .iter()
                    .map(|gr| read_arrow_cell_fast(&batch, gr, r).unwrap_or(Value::Null))
                    .collect();
                let mut accs: Vec<VectorizedAccumulator> = agg_plans
                    .iter()
                    .map(|p| match p {
                        Some(sp) => VectorizedAccumulator::with_tracking(
                            sp.needs_samples(),
                            sp.needs_distinct(),
                        ),
                        None => VectorizedAccumulator::new(),
                    })
                    .collect();
                for (slot, plan_opt) in agg_plans.iter().enumerate() {
                    if let Some(spec) = plan_opt {
                        if spec.is_star {
                            accs[slot].count_all += 1;
                            accs[slot].count += 1;
                        } else if let Some(cref) = &agg_col_refs[slot] {
                            match cref {
                                ArrowColRef::Projected(cidx) => accumulate_arrow_cell(
                                    &mut accs[slot],
                                    batch.column(*cidx).as_ref(),
                                    r,
                                ),
                                ArrowColRef::Hive(hv) => accs[slot].accumulate_value(hv),
                                ArrowColRef::Missing(_) => {}
                            }
                        }
                    }
                }
                groups.insert(hash_buf.clone(), OlapGroupState { keys, accs });
            }
        }
    }

    Ok((groups, stats))
}

#[allow(clippy::too_many_arguments)]
fn execute_parallel_group_by(
    files: &[ResolvedLakeFile],
    columns: &[String],
    agg_plans: &[Option<ParsedAggSpec>],
    gb_cols: &[String],
    referenced_cols: &[String],
    where_expr: Option<&ResolvedWhereExpr>,
    having: Option<&ResolvedWhereExpr>,
    order_by: Option<&OrderByClause>,
    limit: Option<usize>,
    offset: Option<usize>,
    max_workers: usize,
) -> Result<(Vec<Row>, ParquetOlapStats)> {
    let mut merged_groups: HashMap<Vec<HashableValue>, OlapGroupState> = HashMap::new();
    let mut agg_stats = ParquetOlapStats {
        parallel_workers: max_workers,
        ..Default::default()
    };

    if max_workers <= 1 || files.len() <= 1 {
        for f in files {
            let (local_g, fs) =
                execute_group_by_file(f, agg_plans, gb_cols, referenced_cols, where_expr)?;
            merge_group_maps(&mut merged_groups, local_g);
            merge_stats(&mut agg_stats, &fs);
        }
    } else {
        let chunk_size = files.len().div_ceil(max_workers);
        type GroupChunkResult = Result<(
            HashMap<Vec<HashableValue>, OlapGroupState>,
            ParquetOlapStats,
        )>;
        let results: Vec<GroupChunkResult> = std::thread::scope(|s| {
            let handles: Vec<_> = files
                .chunks(chunk_size)
                .map(|chunk| {
                    s.spawn(move || {
                        let mut local_map: HashMap<Vec<HashableValue>, OlapGroupState> =
                            HashMap::new();
                        let mut local_stats = ParquetOlapStats::default();
                        for f in chunk {
                            let (g, fs) = execute_group_by_file(
                                f,
                                agg_plans,
                                gb_cols,
                                referenced_cols,
                                where_expr,
                            )?;
                            merge_group_maps(&mut local_map, g);
                            merge_stats(&mut local_stats, &fs);
                        }
                        Ok((local_map, local_stats))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        for res in results {
            let (local_map, local_stats) = res?;
            merge_group_maps(&mut merged_groups, local_map);
            merge_stats(&mut agg_stats, &local_stats);
        }
    }

    let mut out_cols = Vec::with_capacity(columns.len());
    for (col_idx, col) in columns.iter().enumerate() {
        if let Some(ref sp) = agg_plans[col_idx] {
            out_cols.push(sp.output_name.clone());
        } else {
            let (expr, alias) = parse_col_and_alias(col);
            out_cols.push(alias.unwrap_or(expr.trim()).to_string());
        }
    }
    let out_cols_arc = Arc::new(out_cols);

    let mut rows = Vec::with_capacity(merged_groups.len());
    let having_aliases: Vec<(String, usize)> = columns
        .iter()
        .enumerate()
        .map(|(idx, c)| (parse_col_and_alias(c).0.trim().to_string(), idx))
        .collect();
    for (_hk, mut st) in merged_groups {
        let mut vals = Vec::with_capacity(columns.len());
        for (col_idx, col) in columns.iter().enumerate() {
            if let Some(ref sp) = agg_plans[col_idx] {
                vals.push(sp.finalize(&mut st.accs[col_idx]));
            } else {
                let (expr, _) = parse_col_and_alias(col);
                let trimmed = expr.trim();
                let gpos = gb_cols
                    .iter()
                    .position(|g| g.eq_ignore_ascii_case(trimmed))
                    .unwrap_or(0);
                vals.push(st.keys.get(gpos).cloned().unwrap_or(Value::Null));
            }
        }
        let row = Row::with_shared_columns(out_cols_arc.clone(), vals);
        if let Some(h) = having
            && !row_matches_having(&row, h, &having_aliases)
        {
            continue;
        }
        rows.push(row);
    }

    if let Some(ord) = order_by {
        rows.sort_by(|a, b| {
            let va = a.get_value(&ord.column).unwrap_or(&Value::Null);
            let vb = b.get_value(&ord.column).unwrap_or(&Value::Null);
            if ord.is_ascending {
                va.compare(vb)
            } else {
                vb.compare(va)
            }
        });
    }

    let skip = offset.unwrap_or(0);
    if skip > 0 {
        if skip >= rows.len() {
            rows.clear();
        } else {
            rows.drain(..skip);
        }
    }
    if let Some(lim) = limit {
        rows.truncate(lim);
    }

    Ok((rows, agg_stats))
}

fn row_matches_having(row: &Row, expr: &ResolvedWhereExpr, aliases: &[(String, usize)]) -> bool {
    match expr {
        ResolvedWhereExpr::Boolean(b) => *b,
        ResolvedWhereExpr::Condition(c) => {
            let val = if let Some(v) = row.get_field_or_json_path(&c.column) {
                v
            } else if let Some((_, idx)) = aliases
                .iter()
                .find(|(raw, _)| raw.eq_ignore_ascii_case(c.column.trim()))
            {
                row.values().get(*idx).cloned().unwrap_or(Value::Null)
            } else {
                return false;
            };
            matches_condition(&val, &c.op, &c.value)
        }
        ResolvedWhereExpr::And(l, r) => {
            row_matches_having(row, l, aliases) && row_matches_having(row, r, aliases)
        }
        ResolvedWhereExpr::Or(l, r) => {
            row_matches_having(row, l, aliases) || row_matches_having(row, r, aliases)
        }
        ResolvedWhereExpr::InList { .. } | ResolvedWhereExpr::InSet { .. } => {
            crate::sql::executor::row_matches_resolved(row, expr)
        }
    }
}

fn merge_group_maps(
    dst: &mut HashMap<Vec<HashableValue>, OlapGroupState>,
    src: HashMap<Vec<HashableValue>, OlapGroupState>,
) {
    for (k, st) in src {
        if let Some(existing) = dst.get_mut(&k) {
            for (d_acc, s_acc) in existing.accs.iter_mut().zip(st.accs.iter()) {
                d_acc.merge(s_acc);
            }
        } else {
            dst.insert(k, st);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_projected_scan(
    files: &[ResolvedLakeFile],
    select_all: bool,
    proj_plans: &[ProjectedColPlan],
    referenced_cols: &[String],
    where_expr: Option<&ResolvedWhereExpr>,
    order_by: Option<&OrderByClause>,
    limit: Option<usize>,
    offset: Option<usize>,
    max_workers: usize,
) -> Result<(Vec<Row>, ParquetOlapStats)> {
    let early_cap = if order_by.is_none() {
        limit.map(|l| offset.unwrap_or(0).saturating_add(l))
    } else {
        None
    };

    let mut rows = Vec::new();
    let mut agg_stats = ParquetOlapStats::default();

    if early_cap.is_some() || max_workers <= 1 || files.len() <= 1 {
        agg_stats.parallel_workers = 1;
        for f in files {
            let rem = early_cap.map(|cap| cap.saturating_sub(rows.len()));
            if rem == Some(0) {
                break;
            }
            let (mut chunk, fs) = scan_single_projected_file(
                f,
                select_all,
                proj_plans,
                referenced_cols,
                where_expr,
                order_by.map(|o| o.column.as_str()),
                rem,
            )?;
            rows.append(&mut chunk);
            merge_stats(&mut agg_stats, &fs);
        }
    } else {
        agg_stats.parallel_workers = max_workers;
        let chunk_size = files.len().div_ceil(max_workers);
        let results: Vec<Result<(Vec<Row>, ParquetOlapStats)>> = std::thread::scope(|s| {
            let handles: Vec<_> = files
                .chunks(chunk_size)
                .map(|chunk| {
                    s.spawn(move || {
                        let mut local_rows = Vec::new();
                        let mut local_stats = ParquetOlapStats::default();
                        for f in chunk {
                            let (mut r, fs) = scan_single_projected_file(
                                f,
                                select_all,
                                proj_plans,
                                referenced_cols,
                                where_expr,
                                order_by.map(|o| o.column.as_str()),
                                None,
                            )?;
                            local_rows.append(&mut r);
                            merge_stats(&mut local_stats, &fs);
                        }
                        Ok((local_rows, local_stats))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        for res in results {
            let (mut r, fs) = res?;
            rows.append(&mut r);
            merge_stats(&mut agg_stats, &fs);
        }
    }

    if let Some(ord) = order_by {
        rows.sort_by(|a, b| {
            let va = a.get_value(&ord.column).unwrap_or(&Value::Null);
            let vb = b.get_value(&ord.column).unwrap_or(&Value::Null);
            if ord.is_ascending {
                va.compare(vb)
            } else {
                vb.compare(va)
            }
        });
    }

    // If an extra order_by column was appended only for sorting, strip back to projected columns.
    if !select_all
        && !proj_plans.is_empty()
        && let Some(first) = rows.first()
        && first.columns().len() > proj_plans.len()
    {
        let final_cols = Arc::new(
            proj_plans
                .iter()
                .map(|p| p.output_name.clone())
                .collect::<Vec<_>>(),
        );
        for row in &mut rows {
            let trimmed = row.values()[..proj_plans.len()].to_vec();
            *row = Row::with_shared_columns(final_cols.clone(), trimmed);
        }
    }

    let skip = offset.unwrap_or(0);
    if skip > 0 {
        if skip >= rows.len() {
            rows.clear();
        } else {
            rows.drain(..skip);
        }
    }
    if let Some(lim) = limit {
        rows.truncate(lim);
    }

    Ok((rows, agg_stats))
}

fn scan_single_projected_file(
    lake_file: &ResolvedLakeFile,
    select_all: bool,
    proj_plans: &[ProjectedColPlan],
    referenced_cols: &[String],
    where_expr: Option<&ResolvedWhereExpr>,
    order_col: Option<&str>,
    limit: Option<usize>,
) -> Result<(Vec<Row>, ParquetOlapStats)> {
    let opened = open_parquet_with_pushdown(lake_file, select_all, referenced_cols, where_expr)?;
    let mut stats = ParquetOlapStats {
        files_scanned: 1,
        row_groups_total: opened.row_groups_total,
        row_groups_pruned: opened.row_groups_pruned,
        columns_projected: opened.root_indices.len(),
        ..Default::default()
    };
    if opened.kept_row_groups.is_empty() {
        return Ok((Vec::new(), stats));
    }

    let mut builder = ParquetRecordBatchReaderBuilder::try_new(opened.file)
        .map_err(|e| Error::Corrupted(format!("Parquet open error: {e}")))?
        .with_batch_size(4096);

    if !select_all && !opened.root_indices.is_empty() {
        let mask = ProjectionMask::roots(builder.parquet_schema(), opened.root_indices.clone());
        builder = builder.with_projection(mask);
    }
    if opened.kept_row_groups.len() < opened.row_groups_total {
        builder = builder.with_row_groups(opened.kept_row_groups);
    }

    let reader = builder
        .build()
        .map_err(|e| Error::Corrupted(format!("Parquet reader build error: {e}")))?;

    let mut out_col_names = Vec::new();
    let mut source_col_names = Vec::new();
    if select_all || proj_plans.is_empty() {
        for c in &opened.clean_cols {
            out_col_names.push(c.clone());
            source_col_names.push(c.clone());
        }
        for (pk, _) in &opened.hive_vals {
            if !out_col_names.iter().any(|c| c.eq_ignore_ascii_case(pk)) {
                out_col_names.push(pk.clone());
                source_col_names.push(pk.clone());
            }
        }
    } else {
        for p in proj_plans {
            out_col_names.push(p.output_name.clone());
            source_col_names.push(p.source_name.clone());
        }
        if let Some(oc) = order_col
            && !out_col_names.iter().any(|c| c.eq_ignore_ascii_case(oc))
        {
            out_col_names.push(oc.to_string());
            source_col_names.push(oc.to_string());
        }
    }

    let out_cols_arc = Arc::new(out_col_names);
    let col_refs: Vec<ArrowColRef<'_>> = source_col_names
        .iter()
        .map(|s| resolve_batch_col(s, &opened.projected_clean_cols, &opened.hive_vals))
        .collect();

    let mut rows = Vec::new();
    'outer: for batch_res in reader {
        let batch =
            batch_res.map_err(|e| Error::Corrupted(format!("Parquet batch read error: {e}")))?;
        let num_rows = batch.num_rows();
        for r in 0..num_rows {
            if let Some(expr) = where_expr
                && !eval_batch_where_at(
                    &batch,
                    &opened.projected_clean_cols,
                    &opened.hive_vals,
                    expr,
                    r,
                )
            {
                continue;
            }
            stats.rows_scanned += 1;
            let mut vals = Vec::with_capacity(col_refs.len());
            for cref in &col_refs {
                vals.push(read_arrow_cell_fast(&batch, cref, r).unwrap_or(Value::Null));
            }
            rows.push(Row::with_shared_columns(out_cols_arc.clone(), vals));
            if let Some(lim) = limit
                && rows.len() >= lim
            {
                break 'outer;
            }
        }
    }

    Ok((rows, stats))
}

fn merge_stats(dst: &mut ParquetOlapStats, src: &ParquetOlapStats) {
    dst.files_scanned += src.files_scanned;
    dst.files_pruned_hive += src.files_pruned_hive;
    dst.row_groups_total += src.row_groups_total;
    dst.row_groups_pruned += src.row_groups_pruned;
    if src.columns_projected > dst.columns_projected {
        dst.columns_projected = src.columns_projected;
    }
    dst.rows_scanned += src.rows_scanned;
}
