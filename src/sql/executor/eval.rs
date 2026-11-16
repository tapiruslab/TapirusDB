use super::SQLExecutor;
use super::types::ResolvedWhereExpr;
use crate::error::{Error, Result};
use crate::sql::parser::{BinaryOp, WhereCondition};
use crate::traits::{HashableValue, Row, Value};
use std::collections::HashMap;

/// Match a SQL LIKE pattern with '%' (wildcard multi-char) and '_' (wildcard single-char), case-insensitively
#[inline]
pub fn sql_like_match(pattern: &str, text: &str) -> bool {
    if pattern.is_ascii() && text.is_ascii() {
        let p = pattern.as_bytes();
        let t = text.as_bytes();
        let (p_len, t_len) = (p.len(), t.len());
        let mut p_idx = 0;
        let mut t_idx = 0;
        let mut star_idx = None;
        let mut match_idx = 0;

        while t_idx < t_len {
            if p_idx < p_len && (p[p_idx] == b'_' || p[p_idx].eq_ignore_ascii_case(&t[t_idx])) {
                p_idx += 1;
                t_idx += 1;
            } else if p_idx < p_len && p[p_idx] == b'%' {
                star_idx = Some(p_idx);
                match_idx = t_idx;
                p_idx += 1;
            } else if let Some(star) = star_idx {
                p_idx = star + 1;
                match_idx += 1;
                t_idx = match_idx;
            } else {
                return false;
            }
        }

        while p_idx < p_len && p[p_idx] == b'%' {
            p_idx += 1;
        }
        return p_idx == p_len;
    }

    let p_chars: Vec<char> = pattern.to_lowercase().chars().collect();
    let t_chars: Vec<char> = text.to_lowercase().chars().collect();
    let (p_len, t_len) = (p_chars.len(), t_chars.len());
    let mut p_idx = 0;
    let mut t_idx = 0;
    let mut star_idx = None;
    let mut match_idx = 0;

    while t_idx < t_len {
        if p_idx < p_len && (p_chars[p_idx] == '_' || p_chars[p_idx] == t_chars[t_idx]) {
            p_idx += 1;
            t_idx += 1;
        } else if p_idx < p_len && p_chars[p_idx] == '%' {
            star_idx = Some(p_idx);
            match_idx = t_idx;
            p_idx += 1;
        } else if let Some(star) = star_idx {
            p_idx = star + 1;
            match_idx += 1;
            t_idx = match_idx;
        } else {
            return false;
        }
    }

    while p_idx < p_len && p_chars[p_idx] == '%' {
        p_idx += 1;
    }

    p_idx == p_len
}

/// Token-based text match (BM25 term containment) for `WHERE col MATCH 'terms'`
pub fn sql_match_bm25(query: &str, text: &str) -> bool {
    let query_tokens = crate::memory::bm25::tokenize(query);
    if query_tokens.is_empty() {
        return false;
    }
    let text_tokens: std::collections::HashSet<String> =
        crate::memory::bm25::tokenize(text).into_iter().collect();
    query_tokens.iter().any(|token| text_tokens.contains(token))
}

/// Evaluate a single WhereCondition against a row value
pub fn matches_condition(actual: &Value, op: &BinaryOp, target: &Value) -> bool {
    // ANSI SQL Three-Valued Logic: In WHERE clauses, any comparison where either
    // operand is NULL evaluates to UNKNOWN (false in boolean filtering).
    // EXCEPTION: IS NULL and IS NOT NULL are specifically designed to test for
    // nullness and must NOT be short-circuited by this guard.
    match op {
        BinaryOp::IsNull => return actual.is_null(),
        BinaryOp::IsNotNull => return !actual.is_null(),
        _ => {}
    }

    if actual.is_null() || target.is_null() {
        return false;
    }

    match op {
        BinaryOp::Equals => actual == target,
        BinaryOp::NotEquals => actual != target,
        BinaryOp::GreaterThan => actual.compare(target) == std::cmp::Ordering::Greater,
        BinaryOp::LessThan => actual.compare(target) == std::cmp::Ordering::Less,
        BinaryOp::GreaterOrEqual => {
            let ord = actual.compare(target);
            ord == std::cmp::Ordering::Greater || ord == std::cmp::Ordering::Equal
        }
        BinaryOp::LessOrEqual => {
            let ord = actual.compare(target);
            ord == std::cmp::Ordering::Less || ord == std::cmp::Ordering::Equal
        }
        BinaryOp::Like => match (actual, target) {
            (Value::Text(txt), Value::Text(pat)) => sql_like_match(pat, txt),
            _ => false,
        },
        BinaryOp::NotLike => match (actual, target) {
            (Value::Text(txt), Value::Text(pat)) => !sql_like_match(pat, txt),
            _ => false,
        },
        BinaryOp::Match => match (actual, target) {
            (Value::Text(txt), Value::Text(q)) => sql_match_bm25(q, txt),
            _ => false,
        },
        // Already handled above — unreachable, but Rust requires exhaustiveness.
        BinaryOp::IsNull | BinaryOp::IsNotNull => unreachable!(),
    }
}

/// Check if a row satisfies a resolved where expression
pub fn row_matches_resolved(row: &Row, expr: &ResolvedWhereExpr) -> bool {
    row_matches_resolved_with_udf(row, expr, None)
}

/// Evaluate a simple resolved `WHERE` tree (whose column names were pre-verified by
/// `mark_resolved_where_columns`) directly against a projected `&[Value]` slice with zero allocations.
#[inline]
pub(crate) fn slice_matches_resolved(
    vals: &[Value],
    table_def: &crate::sql::catalog::TableDef,
    expr: &ResolvedWhereExpr,
) -> bool {
    match expr {
        ResolvedWhereExpr::Boolean(b) => *b,
        ResolvedWhereExpr::Condition(cond) => {
            let Some(idx) = super::types::resolve_direct_table_col(table_def, &cond.column) else {
                return false;
            };
            let val = vals.get(idx).unwrap_or(&Value::Null);
            matches_condition(val, &cond.op, &cond.value)
        }
        ResolvedWhereExpr::And(left, right) => {
            slice_matches_resolved(vals, table_def, left)
                && slice_matches_resolved(vals, table_def, right)
        }
        ResolvedWhereExpr::Or(left, right) => {
            slice_matches_resolved(vals, table_def, left)
                || slice_matches_resolved(vals, table_def, right)
        }
        ResolvedWhereExpr::InSet {
            column,
            lookup,
            negated,
            ..
        } => {
            let Some(idx) = super::types::resolve_direct_table_col(table_def, column) else {
                return false;
            };
            let val = vals.get(idx).unwrap_or(&Value::Null);
            if val.is_null() {
                return false;
            }
            let in_set = lookup.contains(val);
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
            let Some(idx) = super::types::resolve_direct_table_col(table_def, column) else {
                return false;
            };
            let val = vals.get(idx).unwrap_or(&Value::Null);
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
        ResolvedWhereExpr::ColumnComparison {
            left_col,
            op,
            right_col,
        } => {
            let (Some(l_idx), Some(r_idx)) = (
                super::types::resolve_direct_table_col(table_def, left_col),
                super::types::resolve_direct_table_col(table_def, right_col),
            ) else {
                return false;
            };
            let l_val = vals.get(l_idx).unwrap_or(&Value::Null);
            let r_val = vals.get(r_idx).unwrap_or(&Value::Null);
            matches_condition(l_val, op, r_val)
        }
        ResolvedWhereExpr::CorrelatedExists { .. }
        | ResolvedWhereExpr::CorrelatedInSubquery { .. }
        | ResolvedWhereExpr::CorrelatedScalarSubquery { .. } => false,
    }
}

/// Check if a row satisfies a resolved where expression, with optional UDF registry support
pub fn row_matches_resolved_with_udf(
    row: &Row,
    expr: &ResolvedWhereExpr,
    udfs: Option<&crate::sql::udf::UdfRegistry>,
) -> bool {
    match expr {
        ResolvedWhereExpr::Boolean(b) => *b,
        ResolvedWhereExpr::Condition(cond) => {
            let val = match row.get_field_or_json_path(&cond.column) {
                Some(v) => v.clone(),
                None => {
                    if let Ok(Some(fn_v)) = try_eval_scalar_expr(row, &cond.column, udfs) {
                        fn_v
                    } else {
                        return false;
                    }
                }
            };
            matches_condition(&val, &cond.op, &cond.value)
        }
        ResolvedWhereExpr::And(left, right) => {
            row_matches_resolved_with_udf(row, left, udfs)
                && row_matches_resolved_with_udf(row, right, udfs)
        }
        ResolvedWhereExpr::Or(left, right) => {
            row_matches_resolved_with_udf(row, left, udfs)
                || row_matches_resolved_with_udf(row, right, udfs)
        }
        ResolvedWhereExpr::InList {
            column,
            values,
            negated,
        } => {
            let val = match row.get_field_or_json_path(column) {
                Some(v) => v.clone(),
                None => {
                    if let Ok(Some(fn_v)) = try_eval_scalar_expr(row, column, udfs) {
                        fn_v
                    } else {
                        return false;
                    }
                }
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
        ResolvedWhereExpr::InSet {
            column,
            lookup,
            negated,
            ..
        } => {
            let val = match row.get_field_or_json_path(column) {
                Some(v) => v,
                None => {
                    if let Ok(Some(fn_v)) = try_eval_scalar_expr(row, column, udfs) {
                        if fn_v.is_null() {
                            return false;
                        }
                        let in_set = lookup.contains(&fn_v);
                        return if *negated {
                            !lookup.has_null && !in_set
                        } else {
                            in_set
                        };
                    } else {
                        return false;
                    }
                }
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
        ResolvedWhereExpr::ColumnComparison {
            left_col,
            op,
            right_col,
        } => {
            let l_val = match row.get_field_or_json_path(left_col) {
                Some(v) => v,
                None => match try_eval_scalar_expr(row, left_col, udfs) {
                    Ok(Some(v)) => v,
                    _ => return false,
                },
            };
            let r_val = match row.get_field_or_json_path(right_col) {
                Some(v) => v,
                None => match try_eval_scalar_expr(row, right_col, udfs) {
                    Ok(Some(v)) => v,
                    _ => return false,
                },
            };
            matches_condition(&l_val, op, &r_val)
        }
        ResolvedWhereExpr::CorrelatedExists { .. }
        | ResolvedWhereExpr::CorrelatedInSubquery { .. }
        | ResolvedWhereExpr::CorrelatedScalarSubquery { .. } => false,
    }
}

/// Check if a row matches all conditions (conjunction AND)
pub fn row_matches_conditions(row: &Row, conditions: &[WhereCondition]) -> bool {
    for cond in conditions {
        let val = match row.get_field_or_json_path(&cond.column) {
            Some(v) => v,
            None => return false,
        };
        if !matches_condition(&val, &cond.op, &cond.value) {
            return false;
        }
    }
    true
}

pub(crate) fn parse_col_and_alias(col: &str) -> (&str, Option<&str>) {
    let trimmed = col.trim();
    if let Some(pos) = crate::sql::parser::find_top_level_as(trimmed) {
        let expr = trimmed[..pos].trim();
        let alias = trimmed[pos + 4..].trim();
        if !alias.is_empty() {
            return (expr, Some(alias));
        }
    }
    (trimmed, None)
}

pub(crate) fn has_window_function(col: &str) -> bool {
    let u = col.to_ascii_uppercase();
    u.contains(" OVER ") || u.contains(" OVER(")
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedWindowCol {
    raw: String,
    expr: String,
    alias: Option<String>,
    func: String,
    arg: String,
    offset: usize,
    partition_by: Vec<String>,
    order_by: Option<(String, bool)>,
}

pub(crate) fn parse_window_function(col_str: &str) -> Option<ParsedWindowCol> {
    let (expr, alias) = parse_col_and_alias(col_str);
    let u = expr.to_ascii_uppercase();
    let over_pos = u.find(" OVER ").or_else(|| u.find(" OVER("))?;

    let func_part = expr[..over_pos].trim();
    let open_p = func_part.find('(')?;
    let close_p = func_part.rfind(')')?;
    let func = func_part[..open_p].trim().to_ascii_uppercase();
    let arg_inner = func_part[open_p + 1..close_p].trim();

    let mut arg = arg_inner.to_string();
    let mut offset = 1usize;

    if func == "LAG" || func == "LEAD" {
        if let Some(comma) = arg_inner.find(',') {
            arg = arg_inner[..comma].trim().to_string();
            let off_str = arg_inner[comma + 1..].trim();
            if let Ok(n) = off_str.parse::<usize>() {
                offset = n;
            }
        }
    } else if func == "NTILE"
        && let Ok(n) = arg_inner.parse::<usize>()
    {
        offset = n.max(1);
    }

    let spec_part = &expr[over_pos..];
    let spec_open = spec_part.find('(')?;
    let spec_close = spec_part.rfind(')')?;
    let spec_inner = spec_part[spec_open + 1..spec_close].trim();
    let spec_u = spec_inner.to_ascii_uppercase();

    let mut partition_by = Vec::new();
    let mut order_by = None;

    if let Some(p_idx) = spec_u.find("PARTITION BY") {
        let after_p = spec_inner[p_idx + 12..].trim();
        let end_p = after_p
            .to_ascii_uppercase()
            .find("ORDER BY")
            .unwrap_or(after_p.len());
        let p_cols_str = after_p[..end_p].trim();
        for c in p_cols_str.split(',') {
            let trimmed = c.trim();
            if !trimmed.is_empty() {
                partition_by.push(trimmed.to_string());
            }
        }
    }

    if let Some(o_idx) = spec_u.find("ORDER BY") {
        let after_o = spec_inner[o_idx + 8..].trim();
        let parts: Vec<&str> = after_o.split_whitespace().collect();
        if !parts.is_empty() {
            let col = parts[0].trim().to_string();
            let is_asc = !(parts.len() > 1 && parts[1].eq_ignore_ascii_case("DESC"));
            order_by = Some((col, is_asc));
        }
    }

    Some(ParsedWindowCol {
        raw: col_str.to_string(),
        expr: expr.to_string(),
        alias: alias.map(|s| s.to_string()),
        func,
        arg,
        offset,
        partition_by,
        order_by,
    })
}

pub(crate) fn evaluate_window_functions(rows: &mut [Row], columns: &[String]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }

    let n_rows = rows.len();

    for col in columns {
        if let Some(w) = parse_window_function(col) {
            let mut partitions: HashMap<Vec<String>, Vec<usize>> = HashMap::new();
            for (i, r) in rows.iter().enumerate() {
                let mut key = Vec::new();
                for p_col in &w.partition_by {
                    let v = format!("{:?}", r.get_value(p_col).unwrap_or(&Value::Null));
                    key.push(v);
                }
                partitions.entry(key).or_default().push(i);
            }

            let mut computed_values: Vec<(usize, Value)> = Vec::with_capacity(n_rows);

            for (_key, mut p_indices) in partitions {
                if let Some((ref o_col, is_asc)) = w.order_by {
                    p_indices.sort_by(|&a_idx, &b_idx| {
                        let val_a = rows[a_idx].get_value(o_col).unwrap_or(&Value::Null);
                        let val_b = rows[b_idx].get_value(o_col).unwrap_or(&Value::Null);
                        if is_asc {
                            val_a.compare(val_b)
                        } else {
                            val_b.compare(val_a)
                        }
                    });
                }

                let p_len = p_indices.len();
                let mut prev_order_val: Option<Value> = None;
                let mut current_rank = 1usize;
                let mut current_dense_rank = 1usize;

                for (pos, &row_idx) in p_indices.iter().enumerate() {
                    let computed_val = match w.func.as_str() {
                        "ROW_NUMBER" => Value::Integer((pos + 1) as i64),
                        "RANK" => {
                            if let Some((ref o_col, _)) = w.order_by {
                                let cur_val = rows[row_idx]
                                    .get_value(o_col)
                                    .cloned()
                                    .unwrap_or(Value::Null);
                                if pos > 0
                                    && let Some(ref prev) = prev_order_val
                                    && prev != &cur_val
                                {
                                    current_rank = pos + 1;
                                }
                                prev_order_val = Some(cur_val);
                            } else {
                                current_rank = pos + 1;
                            }
                            Value::Integer(current_rank as i64)
                        }
                        "DENSE_RANK" => {
                            if let Some((ref o_col, _)) = w.order_by {
                                let cur_val = rows[row_idx]
                                    .get_value(o_col)
                                    .cloned()
                                    .unwrap_or(Value::Null);
                                if pos > 0
                                    && let Some(ref prev) = prev_order_val
                                    && prev != &cur_val
                                {
                                    current_dense_rank += 1;
                                }
                                prev_order_val = Some(cur_val);
                            } else {
                                current_dense_rank = pos + 1;
                            }
                            Value::Integer(current_dense_rank as i64)
                        }
                        "NTILE" => {
                            let k = w.offset.max(1);
                            let bucket = (pos * k) / p_len.max(1) + 1;
                            Value::Integer(bucket as i64)
                        }
                        "LAG" => {
                            if pos >= w.offset {
                                let target_row_idx = p_indices[pos - w.offset];
                                rows[target_row_idx]
                                    .get_value(&w.arg)
                                    .cloned()
                                    .unwrap_or(Value::Null)
                            } else {
                                Value::Null
                            }
                        }
                        "LEAD" => {
                            if pos + w.offset < p_len {
                                let target_row_idx = p_indices[pos + w.offset];
                                rows[target_row_idx]
                                    .get_value(&w.arg)
                                    .cloned()
                                    .unwrap_or(Value::Null)
                            } else {
                                Value::Null
                            }
                        }
                        "COUNT" => {
                            let is_count_all = w.arg == "*" || w.arg == "1" || w.arg.is_empty();
                            if is_count_all {
                                Value::Integer(p_len as i64)
                            } else {
                                let mut count = 0i64;
                                for &idx in &p_indices {
                                    if let Some(v) = rows[idx].get_value(&w.arg)
                                        && v != &Value::Null
                                    {
                                        count += 1;
                                    }
                                }
                                Value::Integer(count)
                            }
                        }
                        "SUM" => {
                            let mut sum = 0.0f64;
                            let mut is_int = true;
                            let mut int_sum = 0i64;
                            for &idx in &p_indices {
                                match rows[idx].get_value(&w.arg) {
                                    Some(Value::Integer(i)) => {
                                        int_sum += *i;
                                        sum += *i as f64;
                                    }
                                    Some(Value::Real(f)) => {
                                        is_int = false;
                                        sum += *f;
                                    }
                                    _ => {}
                                }
                            }
                            if is_int {
                                Value::Integer(int_sum)
                            } else {
                                Value::Real(sum)
                            }
                        }
                        "AVG" => {
                            let mut sum = 0.0f64;
                            let mut count = 0usize;
                            for &idx in &p_indices {
                                match rows[idx].get_value(&w.arg) {
                                    Some(Value::Integer(i)) => {
                                        sum += *i as f64;
                                        count += 1;
                                    }
                                    Some(Value::Real(f)) => {
                                        sum += *f;
                                        count += 1;
                                    }
                                    _ => {}
                                }
                            }
                            if count > 0 {
                                Value::Real(sum / count as f64)
                            } else {
                                Value::Null
                            }
                        }
                        "MIN" => {
                            let mut min_val: Option<Value> = None;
                            for &idx in &p_indices {
                                if let Some(v) = rows[idx].get_value(&w.arg)
                                    && v != &Value::Null
                                    && (min_val.is_none()
                                        || min_val
                                            .as_ref()
                                            .map(|m| v.compare(m).is_lt())
                                            .unwrap_or(false))
                                {
                                    min_val = Some(v.clone());
                                }
                            }
                            min_val.unwrap_or(Value::Null)
                        }
                        "MAX" => {
                            let mut max_val: Option<Value> = None;
                            for &idx in &p_indices {
                                if let Some(v) = rows[idx].get_value(&w.arg)
                                    && v != &Value::Null
                                    && (max_val.is_none()
                                        || max_val
                                            .as_ref()
                                            .map(|m| v.compare(m).is_gt())
                                            .unwrap_or(false))
                                {
                                    max_val = Some(v.clone());
                                }
                            }
                            max_val.unwrap_or(Value::Null)
                        }
                        _ => Value::Null,
                    };

                    computed_values.push((row_idx, computed_val));
                }
            }

            for (row_idx, val) in computed_values {
                rows[row_idx].push_column(w.expr.clone(), val.clone());
                rows[row_idx].push_column(w.raw.clone(), val.clone());
                if let Some(ref al) = w.alias {
                    rows[row_idx].push_column(al.clone(), val);
                }
            }
        }
    }

    Ok(())
}

/// Project a row without custom UDF registry (built-ins only)
pub fn project_row(row: &Row, requested_cols: &[String]) -> Result<Row> {
    project_row_with_udf(row, requested_cols, None)
}

pub(crate) fn eval_projected_expr(
    row: &Row,
    expr: &str,
    raw_col: &str,
    udfs: Option<&crate::sql::udf::UdfRegistry>,
) -> Result<Value> {
    if let Some(fn_val) = try_eval_scalar_expr(row, expr, udfs)? {
        Ok(fn_val)
    } else if (expr.starts_with('\'') && expr.ends_with('\''))
        || expr.parse::<f64>().is_ok()
        || expr.eq_ignore_ascii_case("NULL")
    {
        eval_func_arg(row, expr, udfs)
    } else {
        Ok(row
            .get_field_or_json_path(expr)
            .or_else(|| row.get_field_or_json_path(raw_col))
            .or_else(|| row.get_value(expr).cloned())
            .unwrap_or(Value::Null))
    }
}

/// Project a row with custom scalar UDF registry and standard SQL scalar functions
pub fn project_row_with_udf(
    row: &Row,
    requested_cols: &[String],
    udfs: Option<&crate::sql::udf::UdfRegistry>,
) -> Result<Row> {
    let mut names = Vec::with_capacity(requested_cols.len());
    let mut vals = Vec::with_capacity(requested_cols.len());
    for col in requested_cols {
        let (expr, alias) = parse_col_and_alias(col);
        let val = eval_projected_expr(row, expr, col, udfs)?;
        names.push(alias.unwrap_or(expr).to_string());
        vals.push(val);
    }
    Ok(Row::new(names, vals))
}

#[inline]
pub(crate) fn keyword_at(bytes: &[u8], idx: usize, kw: &[u8]) -> bool {
    let end = idx + kw.len();
    if end > bytes.len() || !bytes[idx..end].eq_ignore_ascii_case(kw) {
        return false;
    }
    let left_ok = idx == 0 || !(bytes[idx - 1].is_ascii_alphanumeric() || bytes[idx - 1] == b'_');
    let right_ok =
        end == bytes.len() || !(bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_');
    left_ok && right_ok
}

pub(crate) fn split_func_args(inner: &str) -> Vec<&str> {
    if inner.trim().is_empty() {
        return Vec::new();
    }
    let mut args = Vec::new();
    let mut cur_start = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut paren_depth: u32 = 0;
    let mut bracket_depth: u32 = 0;
    let mut case_depth: u32 = 0;
    let bytes = inner.as_bytes();

    for (idx, ch) in inner.char_indices() {
        match ch {
            '\'' if !in_double_quote => in_single_quote = !in_single_quote,
            '"' if !in_single_quote => in_double_quote = !in_double_quote,
            '(' if !in_single_quote && !in_double_quote => paren_depth += 1,
            ')' if !in_single_quote && !in_double_quote => {
                paren_depth = paren_depth.saturating_sub(1);
            }
            '[' if !in_single_quote && !in_double_quote => bracket_depth += 1,
            ']' if !in_single_quote && !in_double_quote => {
                bracket_depth = bracket_depth.saturating_sub(1)
            }
            c if !in_single_quote
                && !in_double_quote
                && (c.eq_ignore_ascii_case(&'c') || c.eq_ignore_ascii_case(&'e')) =>
            {
                if keyword_at(bytes, idx, b"CASE") {
                    case_depth += 1;
                } else if keyword_at(bytes, idx, b"END") {
                    case_depth = case_depth.saturating_sub(1);
                }
            }
            ',' if !in_single_quote
                && !in_double_quote
                && paren_depth == 0
                && bracket_depth == 0
                && case_depth == 0 =>
            {
                args.push(inner[cur_start..idx].trim());
                cur_start = idx + 1;
            }
            _ => {}
        }
    }
    args.push(inner[cur_start..].trim());
    args
}

pub(crate) fn eval_func_arg(
    row: &Row,
    raw: &str,
    udfs: Option<&crate::sql::udf::UdfRegistry>,
) -> Result<Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Value::Null);
    }
    if (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        || (trimmed.starts_with('"') && trimmed.ends_with('"'))
    {
        return Ok(Value::Text(strip_quotes(trimmed).replace("''", "'")));
    }
    let upper = trimmed.to_ascii_uppercase();
    if upper == "NULL" {
        return Ok(Value::Null);
    }
    if upper == "TRUE" {
        return Ok(Value::Integer(1));
    }
    if upper == "FALSE" {
        return Ok(Value::Integer(0));
    }
    if let Ok(i) = trimmed.parse::<i64>() {
        return Ok(Value::Integer(i));
    }
    if let Ok(f) = trimmed.parse::<f64>() {
        return Ok(Value::Real(f));
    }
    if let Some(val) = try_eval_scalar_expr(row, trimmed, udfs)? {
        return Ok(val);
    }
    if let Some(val) = row
        .get_field_or_json_path(trimmed)
        .or_else(|| row.get_value(trimmed).cloned())
    {
        return Ok(val);
    }
    Ok(Value::Text(trimmed.to_string()))
}

pub(crate) fn strip_outer_parens(expr: &str) -> &str {
    let mut s = expr.trim();
    while s.starts_with('(') && s.ends_with(')') {
        let mut depth = 0usize;
        let mut in_sq = false;
        let mut in_dq = false;
        let mut wraps_entire = true;
        for (idx, ch) in s.char_indices() {
            match ch {
                '\'' if !in_dq => in_sq = !in_sq,
                '"' if !in_sq => in_dq = !in_dq,
                '(' if !in_sq && !in_dq => depth += 1,
                ')' if !in_sq && !in_dq => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && idx + 1 < s.len() {
                        wraps_entire = false;
                        break;
                    }
                }
                _ => {}
            }
        }
        if wraps_entire && depth == 0 {
            s = s[1..s.len() - 1].trim();
        } else {
            break;
        }
    }
    s
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum InfixOp {
    Concat,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

pub(crate) fn find_top_level_infix(expr: &str) -> Option<(usize, usize, InfixOp)> {
    let bytes = expr.as_bytes();
    let mut depth_paren = 0usize;
    let mut depth_bracket = 0usize;
    let mut depth_case = 0usize;
    let mut in_sq = false;
    let mut in_dq = false;

    let mut tier1: Option<(usize, usize, InfixOp)> = None; // ||
    let mut tier2: Option<(usize, usize, InfixOp)> = None; // +, -
    let mut tier3: Option<(usize, usize, InfixOp)> = None; // *, /, %

    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'\'' if !in_dq => in_sq = !in_sq,
            b'"' if !in_sq => in_dq = !in_dq,
            b'(' if !in_sq && !in_dq => depth_paren += 1,
            b')' if !in_sq && !in_dq => depth_paren = depth_paren.saturating_sub(1),
            b'[' if !in_sq && !in_dq => depth_bracket += 1,
            b']' if !in_sq && !in_dq => depth_bracket = depth_bracket.saturating_sub(1),
            _ if !in_sq && !in_dq => {
                if keyword_at(bytes, i, b"CASE") {
                    depth_case += 1;
                    i += 4;
                    continue;
                } else if keyword_at(bytes, i, b"END") {
                    depth_case = depth_case.saturating_sub(1);
                    i += 3;
                    continue;
                } else if depth_paren == 0 && depth_bracket == 0 && depth_case == 0 {
                    if b == b'|' && i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                        tier1 = Some((i, 2, InfixOp::Concat));
                        i += 2;
                        continue;
                    } else if b == b'+' {
                        tier2 = Some((i, 1, InfixOp::Add));
                    } else if b == b'-' {
                        let prev_non_ws = expr[..i].trim_end().as_bytes().last().copied();
                        let is_binary = matches!(
                            prev_non_ws,
                            Some(c) if c.is_ascii_alphanumeric() || matches!(c, b'_' | b')' | b'\'' | b'"')
                        );
                        if is_binary {
                            tier2 = Some((i, 1, InfixOp::Sub));
                        }
                    } else if b == b'*' && !expr[..i].trim().is_empty() {
                        tier3 = Some((i, 1, InfixOp::Mul));
                    } else if b == b'/' {
                        tier3 = Some((i, 1, InfixOp::Div));
                    } else if b == b'%' {
                        tier3 = Some((i, 1, InfixOp::Mod));
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }

    tier1.or(tier2).or(tier3)
}

pub(crate) fn eval_infix_expr(
    row: &Row,
    expr: &str,
    udfs: Option<&crate::sql::udf::UdfRegistry>,
) -> Result<Option<Value>> {
    let trimmed = strip_outer_parens(expr);
    let Some((pos, len, op)) = find_top_level_infix(trimmed) else {
        return Ok(None);
    };
    let lhs_raw = trimmed[..pos].trim();
    let rhs_raw = trimmed[pos + len..].trim();
    if lhs_raw.is_empty() || rhs_raw.is_empty() {
        return Ok(None);
    }
    let lhs = eval_func_arg(row, lhs_raw, udfs)?;
    let rhs = eval_func_arg(row, rhs_raw, udfs)?;

    if matches!(lhs, Value::Null) || matches!(rhs, Value::Null) {
        return Ok(Some(Value::Null));
    }

    let result = match op {
        InfixOp::Concat => Value::Text(format!(
            "{}{}",
            value_to_concat_str(&lhs),
            value_to_concat_str(&rhs)
        )),
        InfixOp::Add => match (lhs, rhs) {
            (Value::Integer(a), Value::Integer(b)) => a
                .checked_add(b)
                .map(Value::Integer)
                .unwrap_or_else(|| Value::Real(a as f64 + b as f64)),
            (a, b) => match (as_f64(&a), as_f64(&b)) {
                (Some(x), Some(y)) => Value::Real(x + y),
                _ => Value::Null,
            },
        },
        InfixOp::Sub => match (lhs, rhs) {
            (Value::Integer(a), Value::Integer(b)) => a
                .checked_sub(b)
                .map(Value::Integer)
                .unwrap_or_else(|| Value::Real(a as f64 - b as f64)),
            (a, b) => match (as_f64(&a), as_f64(&b)) {
                (Some(x), Some(y)) => Value::Real(x - y),
                _ => Value::Null,
            },
        },
        InfixOp::Mul => match (lhs, rhs) {
            (Value::Integer(a), Value::Integer(b)) => a
                .checked_mul(b)
                .map(Value::Integer)
                .unwrap_or_else(|| Value::Real(a as f64 * b as f64)),
            (a, b) => match (as_f64(&a), as_f64(&b)) {
                (Some(x), Some(y)) => Value::Real(x * y),
                _ => Value::Null,
            },
        },
        InfixOp::Div => match (lhs, rhs) {
            (Value::Integer(_), Value::Integer(0)) => Value::Null,
            (Value::Integer(a), Value::Integer(b)) => Value::Integer(a / b),
            (a, b) => match (as_f64(&a), as_f64(&b)) {
                (Some(_), Some(0.0)) => Value::Null,
                (Some(x), Some(y)) => Value::Real(x / y),
                _ => Value::Null,
            },
        },
        InfixOp::Mod => match (lhs, rhs) {
            (Value::Integer(_), Value::Integer(0)) => Value::Null,
            (Value::Integer(a), Value::Integer(b)) => Value::Integer(a % b),
            (a, b) => match (as_f64(&a), as_f64(&b)) {
                (Some(_), Some(0.0)) => Value::Null,
                (Some(x), Some(y)) => Value::Real(x % y),
                _ => Value::Null,
            },
        },
    };
    Ok(Some(result))
}

pub(crate) fn as_f64(val: &Value) -> Option<f64> {
    match val {
        Value::Integer(i) => Some(*i as f64),
        Value::Real(f) => Some(*f),
        Value::Text(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

pub(crate) fn value_to_concat_str(val: &Value) -> String {
    match val {
        Value::Text(s) => s.clone(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => f.to_string(),
        other => other.to_string(),
    }
}

/// Try evaluating a scalar expression (infix arithmetic/concat, built-in SQL function, custom UDF, or TAP function)
pub fn try_eval_scalar_expr(
    row: &Row,
    expr: &str,
    udfs: Option<&crate::sql::udf::UdfRegistry>,
) -> Result<Option<Value>> {
    let trimmed = strip_outer_parens(expr);
    if let Some(infix_val) = eval_infix_expr(row, trimmed, udfs)? {
        return Ok(Some(infix_val));
    }
    if let Some(case_val) = try_eval_case_expr(row, trimmed, udfs)? {
        return Ok(Some(case_val));
    }
    if !trimmed.contains('(') || !trimmed.ends_with(')') {
        return Ok(None);
    }

    if let Some(cast_val) = try_eval_cast_expr(row, trimmed, udfs)? {
        return Ok(Some(cast_val));
    }

    if let Some(tap_val) = try_eval_tap_function(row, trimmed)? {
        return Ok(Some(tap_val));
    }

    let paren_idx = match trimmed.find('(') {
        Some(idx) => idx,
        None => return Ok(None),
    };
    let func_name = trimmed[..paren_idx].trim();
    let inner = trimmed[paren_idx + 1..trimmed.len() - 1].trim();

    let raw_args = split_func_args(inner);
    let mut evaluated_args = Vec::with_capacity(raw_args.len());
    for raw_arg in raw_args {
        evaluated_args.push(eval_func_arg(row, raw_arg, udfs)?);
    }

    if let Some(registry) = udfs
        && let Some(res) = registry.try_call(func_name, &evaluated_args)?
    {
        return Ok(Some(res));
    }

    let standard = crate::sql::udf::UdfRegistry::new();
    if let Some(res) = standard.try_call(func_name, &evaluated_args)? {
        return Ok(Some(res));
    }

    Ok(None)
}

pub(crate) fn try_eval_tap_function(row: &Row, expr: &str) -> Result<Option<Value>> {
    let trimmed = expr.trim();
    let upper = trimmed.to_ascii_uppercase();

    let (func_type, rest) = if upper.starts_with("TAP_CLASSIFY_GROUNDED(") {
        ("CLASSIFY_GROUNDED", &trimmed[22..])
    } else if upper.starts_with("TAP_VERIFY_GROUNDED(") {
        ("VERIFY_GROUNDED", &trimmed[20..])
    } else if upper.starts_with("TAP_CLASSIFY(") {
        ("CLASSIFY", &trimmed[13..])
    } else if upper.starts_with("TAP_SCORE(") {
        ("SCORE", &trimmed[10..])
    } else if upper.starts_with("TAP_VERIFY(") {
        ("VERIFY", &trimmed[11..])
    } else if upper.starts_with("TAP_ROUTE(") {
        ("ROUTE", &trimmed[10..])
    } else if upper.starts_with("VECTOR_COSINE(") {
        ("VECTOR_COSINE", &trimmed[14..])
    } else if upper.starts_with("VECTOR_L2(") {
        ("VECTOR_L2", &trimmed[10..])
    } else if upper.starts_with("VECTOR_DISTANCE(") {
        ("VECTOR_L2", &trimmed[16..])
    } else {
        return Ok(None);
    };

    if !rest.ends_with(')') {
        return Ok(None);
    }
    let inner = rest[..rest.len() - 1].trim();

    // Split by commas not inside quotes or brackets
    let mut args = Vec::new();
    let mut cur_start = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut bracket_depth: u32 = 0;

    for (idx, ch) in inner.char_indices() {
        match ch {
            '\'' if !in_double_quote => in_single_quote = !in_single_quote,
            '"' if !in_single_quote => in_double_quote = !in_double_quote,
            '[' if !in_single_quote && !in_double_quote => bracket_depth += 1,
            ']' if !in_single_quote && !in_double_quote => {
                bracket_depth = bracket_depth.saturating_sub(1)
            }
            ',' if !in_single_quote && !in_double_quote && bracket_depth == 0 => {
                args.push(inner[cur_start..idx].trim());
                cur_start = idx + 1;
            }
            _ => {}
        }
    }
    args.push(inner[cur_start..].trim());

    let raw_arg1 = args.first().copied().unwrap_or("");
    let raw_arg2 = args.get(1).copied().unwrap_or("");

    let arg1_val = if let Some(v) = row
        .get_field_or_json_path(raw_arg1)
        .or_else(|| row.get_value(raw_arg1).cloned())
    {
        match v {
            Value::Text(s) => s.clone(),
            other => other.to_string(),
        }
    } else {
        strip_quotes(raw_arg1).to_string()
    };

    let arg2_val = strip_quotes(raw_arg2);

    match func_type {
        "VECTOR_COSINE" => {
            let vec1 = resolve_vector_arg(row, raw_arg1)?;
            let vec2 = resolve_vector_arg(row, raw_arg2)?;
            let sim = crate::vector::cosine_similarity(&vec1, &vec2);
            Ok(Some(Value::Real(sim as f64)))
        }
        "VECTOR_L2" => {
            let vec1 = resolve_vector_arg(row, raw_arg1)?;
            let vec2 = resolve_vector_arg(row, raw_arg2)?;
            let dist = crate::vector::euclidean_distance(&vec1, &vec2);
            Ok(Some(Value::Real(dist as f64)))
        }
        "CLASSIFY_GROUNDED" => {
            let index_name = args.get(2).map(|s| strip_quotes(s)).unwrap_or("default");
            let top_k = args
                .get(3)
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(3);
            let label = crate::tap::sql_bridge::eval_tap_classify_grounded(
                &arg1_val, arg2_val, index_name, top_k,
            )?;
            Ok(Some(Value::Text(label)))
        }
        "VERIFY_GROUNDED" => {
            let index_name = args.get(2).map(|s| strip_quotes(s)).unwrap_or("default");
            let top_k = args
                .get(3)
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(3);
            let verified = crate::tap::sql_bridge::eval_tap_verify_grounded(
                &arg1_val, arg2_val, index_name, top_k,
            )?;
            Ok(Some(Value::Integer(if verified { 1 } else { 0 })))
        }
        "CLASSIFY" => {
            let label = crate::tap::sql_bridge::eval_tap_classify(&arg1_val, arg2_val)?;
            Ok(Some(Value::Text(label)))
        }
        "SCORE" => {
            let score = crate::tap::sql_bridge::eval_tap_score(&arg1_val, arg2_val)?;
            Ok(Some(Value::Real(score as f64)))
        }
        "VERIFY" => {
            let verified = crate::tap::sql_bridge::eval_tap_verify(&arg1_val, arg2_val)?;
            Ok(Some(Value::Integer(if verified { 1 } else { 0 })))
        }
        "ROUTE" => {
            let route = crate::tap::sql_bridge::eval_tap_route(&arg1_val, arg2_val)?;
            Ok(Some(Value::Text(route)))
        }
        _ => Ok(None),
    }
}

pub(crate) fn resolve_vector_arg(row: &Row, raw_arg: &str) -> Result<Vec<f32>> {
    let trimmed = raw_arg.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        let inner = &trimmed[1..trimmed.len() - 1];
        let mut vec = Vec::new();
        for item in inner.split(',') {
            let item_t = item.trim();
            if !item_t.is_empty()
                && let Ok(f) = item_t.parse::<f32>()
            {
                vec.push(f);
            }
        }
        Ok(vec)
    } else if let Some(val) = row
        .get_field_or_json_path(trimmed)
        .or_else(|| row.get_value(trimmed).cloned())
    {
        match val {
            Value::Vector(v) => Ok(v),
            _ => Err(Error::ConstraintViolation(format!(
                "Argument '{trimmed}' is not a vector"
            ))),
        }
    } else {
        Err(Error::ConstraintViolation(format!(
            "Could not resolve vector argument '{trimmed}'"
        )))
    }
}

pub(crate) fn strip_quotes(s: &str) -> &str {
    let trimmed = s.trim();
    if ((trimmed.starts_with('\'') && trimmed.ends_with('\''))
        || (trimmed.starts_with('"') && trimmed.ends_with('"')))
        && trimmed.len() >= 2
    {
        return &trimmed[1..trimmed.len() - 1];
    }
    trimmed
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedAggSpec {
    pub(crate) output_name: String,
    pub(crate) func: String,
    pub(crate) arg: String,
    pub(crate) percentile_q: Option<f64>,
    pub(crate) is_star: bool,
    pub(crate) is_distinct: bool,
}

impl ParsedAggSpec {
    pub(crate) fn needs_samples(&self) -> bool {
        matches!(
            self.func.as_str(),
            "MEDIAN" | "PERCENTILE" | "PERCENTILE_CONT"
        )
    }

    pub(crate) fn needs_distinct(&self) -> bool {
        self.is_distinct
    }

    pub(crate) fn finalize(
        &self,
        acc: &mut crate::sql::vectorized::VectorizedAccumulator,
    ) -> Value {
        match self.func.as_str() {
            "COUNT" if self.is_distinct => acc.finalize_count_distinct(),
            "COUNT" => acc.finalize_count(self.is_star),
            "SUM" => acc.finalize_sum(),
            "AVG" => acc.finalize_avg(),
            "MIN" => acc.finalize_min(),
            "MAX" => acc.finalize_max(),
            "STDDEV" | "STDDEV_SAMP" | "STDEV" => acc.finalize_stddev(true),
            "STDDEV_POP" => acc.finalize_stddev(false),
            "VARIANCE" | "VAR_SAMP" => acc.finalize_variance(true),
            "VAR_POP" => acc.finalize_variance(false),
            "MEDIAN" => acc.finalize_median(),
            "PERCENTILE" | "PERCENTILE_CONT" => {
                acc.finalize_percentile(self.percentile_q.unwrap_or(0.5))
            }
            _ => Value::Null,
        }
    }
}

pub(crate) fn parse_aggregate_spec(col: &str) -> Option<ParsedAggSpec> {
    let (expr, alias_opt) = parse_col_and_alias(col);
    let expr_upper = expr.to_ascii_uppercase();
    if expr_upper.contains(" OVER ") || expr_upper.contains(" OVER(") {
        return None;
    }
    let open_p = expr.find('(')?;
    let close_p = expr.rfind(')')?;
    if close_p <= open_p {
        return None;
    }
    let func = expr[..open_p].trim().to_ascii_uppercase();
    if !matches!(
        func.as_str(),
        "COUNT"
            | "SUM"
            | "AVG"
            | "MIN"
            | "MAX"
            | "STDDEV"
            | "STDDEV_SAMP"
            | "STDDEV_POP"
            | "STDEV"
            | "VARIANCE"
            | "VAR_SAMP"
            | "VAR_POP"
            | "MEDIAN"
            | "PERCENTILE"
            | "PERCENTILE_CONT"
    ) {
        return None;
    }

    let raw_arg = expr[open_p + 1..close_p].trim();
    let (arg_part, percentile_q) = if matches!(func.as_str(), "PERCENTILE" | "PERCENTILE_CONT")
        && let Some((col_part, q_part)) = raw_arg.split_once(',')
    {
        (col_part.trim(), q_part.trim().parse::<f64>().ok())
    } else {
        (raw_arg, None)
    };

    let mut is_distinct = false;
    let cleaned_arg =
        if func == "COUNT" && arg_part.len() > 9 && arg_part[..9].eq_ignore_ascii_case("DISTINCT ")
        {
            is_distinct = true;
            arg_part[9..].trim()
        } else {
            arg_part
        };

    let is_star = !is_distinct
        && (cleaned_arg == "*"
            || cleaned_arg == "1"
            || (!cleaned_arg.is_empty() && cleaned_arg.chars().all(|c| c.is_ascii_digit())));

    let output_name = alias_opt.unwrap_or(expr).to_string();
    Some(ParsedAggSpec {
        output_name,
        func,
        arg: cleaned_arg.to_string(),
        percentile_q,
        is_star,
        is_distinct,
    })
}

pub(crate) fn is_aggregate_query(columns: &[String]) -> bool {
    columns.iter().any(|c| parse_aggregate_spec(c).is_some())
}

pub(crate) fn compute_aggregate_from_spec(spec: &ParsedAggSpec, rows: &[Row]) -> Value {
    if spec.func == "COUNT" && spec.is_star {
        return Value::Integer(rows.len() as i64);
    }

    let mut acc = crate::sql::vectorized::VectorizedAccumulator::with_tracking(
        spec.needs_samples(),
        spec.needs_distinct(),
    );
    for r in rows {
        if let Some(v) = r.get_value(&spec.arg) {
            acc.accumulate_value(v);
        }
    }
    spec.finalize(&mut acc)
}

pub(crate) fn aggregate_ephemeral_rows(
    rows: &[Row],
    columns: &[String],
    gb_cols: &[String],
    resolved_having: Option<&ResolvedWhereExpr>,
) -> Vec<Row> {
    let mut group_map: HashMap<Vec<HashableValue>, usize> = HashMap::new();
    let mut groups: Vec<(Vec<Value>, Vec<&Row>)> = Vec::new();
    for row in rows {
        let key: Vec<Value> = gb_cols
            .iter()
            .map(|c| row.get_value(c).cloned().unwrap_or(Value::Null))
            .collect();
        let hash_key: Vec<HashableValue> = key.iter().map(HashableValue::from).collect();
        if let Some(&pos) = group_map.get(&hash_key) {
            groups[pos].1.push(row);
        } else {
            let pos = groups.len();
            group_map.insert(hash_key, pos);
            groups.push((key, vec![row]));
        }
    }

    let output_cols = if columns.is_empty() {
        gb_cols.to_vec()
    } else {
        columns.to_vec()
    };

    let mut grouped_rows = Vec::with_capacity(groups.len());
    for (key, g_rows) in groups {
        let mut g_cols = Vec::with_capacity(output_cols.len());
        let mut g_vals = Vec::with_capacity(output_cols.len());

        for col in &output_cols {
            if let Some(spec) = parse_aggregate_spec(col) {
                if spec.func == "COUNT" && spec.is_star {
                    g_cols.push(spec.output_name);
                    g_vals.push(Value::Integer(g_rows.len() as i64));
                } else {
                    let mut acc = crate::sql::vectorized::VectorizedAccumulator::with_tracking(
                        spec.needs_samples(),
                        spec.needs_distinct(),
                    );
                    for r in &g_rows {
                        if let Some(v) = r.get_value(&spec.arg) {
                            acc.accumulate_value(v);
                        }
                    }
                    let val = spec.finalize(&mut acc);
                    g_cols.push(spec.output_name);
                    g_vals.push(val);
                }
                continue;
            }
            let (raw_col, alias_opt) = parse_col_and_alias(col);
            let out_name = alias_opt.unwrap_or(raw_col).to_string();
            if let Some(pos) = gb_cols.iter().position(|c| c.eq_ignore_ascii_case(raw_col)) {
                g_cols.push(out_name);
                g_vals.push(key[pos].clone());
            } else {
                let val = g_rows
                    .first()
                    .and_then(|r| r.get_value(raw_col))
                    .cloned()
                    .unwrap_or(Value::Null);
                g_cols.push(out_name);
                g_vals.push(val);
            }
        }

        let g_row = Row::new(g_cols, g_vals);
        if let Some(h_expr) = resolved_having
            && !row_matches_resolved(&g_row, h_expr)
        {
            continue;
        }
        grouped_rows.push(g_row);
    }

    grouped_rows
}

pub(crate) fn compare_rows_by_order(
    a: &Row,
    b: &Row,
    order_by: &[crate::sql::parser::OrderByClause],
) -> std::cmp::Ordering {
    for ord in order_by {
        let va = a.get_value(&ord.column).unwrap_or(&Value::Null);
        let vb = b.get_value(&ord.column).unwrap_or(&Value::Null);
        let cmp = if ord.is_ascending {
            va.compare(vb)
        } else {
            vb.compare(va)
        };
        if cmp != std::cmp::Ordering::Equal {
            return cmp;
        }
    }
    std::cmp::Ordering::Equal
}

/// Append projected SELECT alias columns referenced by `ORDER BY` when raw rows lack them,
/// so multi-column `ORDER BY` works uniformly across base columns and computed aliases.
pub(crate) fn enrich_rows_with_order_aliases(
    executor: &SQLExecutor,
    rows: &mut [Row],
    columns: &[String],
    order_by: &[crate::sql::parser::OrderByClause],
) -> Result<()> {
    if rows.is_empty() || order_by.is_empty() || columns.is_empty() {
        return Ok(());
    }
    let mut missing_aliases: Vec<(String, String, String)> = Vec::new();
    for ord in order_by {
        if rows[0].get_value(&ord.column).is_some()
            || missing_aliases
                .iter()
                .any(|(_, _, alias)| alias.eq_ignore_ascii_case(&ord.column))
        {
            continue;
        }
        for col in columns {
            let (expr, alias_opt) = parse_col_and_alias(col);
            if let Some(alias) = alias_opt
                && alias.eq_ignore_ascii_case(&ord.column)
            {
                missing_aliases.push((col.clone(), expr.to_string(), alias.to_string()));
                break;
            }
        }
    }
    if missing_aliases.is_empty() {
        return Ok(());
    }
    for row in rows.iter_mut() {
        let mut new_cols = row.columns().to_vec();
        let mut new_vals = row.values().to_vec();
        for (raw_col, expr, alias) in &missing_aliases {
            let val = eval_projected_expr(row, expr, raw_col, Some(&executor.udf_registry))?;
            new_cols.push(alias.clone());
            new_vals.push(val);
        }
        *row = Row::new(new_cols, new_vals);
    }
    Ok(())
}

fn try_eval_cast_expr(
    row: &Row,
    trimmed: &str,
    udfs: Option<&crate::sql::udf::UdfRegistry>,
) -> Result<Option<Value>> {
    if !keyword_at(trimmed.as_bytes(), 0, b"CAST") {
        return Ok(None);
    }
    let after_kw = trimmed[4..].trim_start();
    if !after_kw.starts_with('(') || !after_kw.ends_with(')') {
        return Ok(None);
    }
    let inner = &after_kw[1..after_kw.len() - 1];
    let bytes = inner.as_bytes();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut case_depth = 0usize;
    let mut in_sq = false;
    let mut in_dq = false;
    let mut as_pos = None;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_dq => in_sq = !in_sq,
            b'"' if !in_sq => in_dq = !in_dq,
            b'(' if !in_sq && !in_dq => paren_depth += 1,
            b')' if !in_sq && !in_dq => paren_depth = paren_depth.saturating_sub(1),
            b'[' if !in_sq && !in_dq => bracket_depth += 1,
            b']' if !in_sq && !in_dq => bracket_depth = bracket_depth.saturating_sub(1),
            _ if !in_sq && !in_dq => {
                if keyword_at(bytes, i, b"CASE") {
                    case_depth += 1;
                    i += 4;
                    continue;
                } else if keyword_at(bytes, i, b"END") {
                    case_depth = case_depth.saturating_sub(1);
                    i += 3;
                    continue;
                } else if paren_depth == 0
                    && bracket_depth == 0
                    && case_depth == 0
                    && keyword_at(bytes, i, b"AS")
                {
                    as_pos = Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    let Some(pos) = as_pos else {
        return Ok(None);
    };
    let lhs_raw = inner[..pos].trim();
    let target_raw = inner[pos + 2..].trim().to_ascii_uppercase();
    let val = eval_func_arg(row, lhs_raw, udfs)?;
    Ok(Some(cast_value_as(val, &target_raw)))
}

fn cast_value_as(val: Value, target_ty: &str) -> Value {
    if val.is_null() {
        return Value::Null;
    }
    let base_ty = target_ty
        .split('(')
        .next()
        .unwrap_or(target_ty)
        .trim()
        .to_ascii_uppercase();
    match base_ty.as_str() {
        "INTEGER" | "INT" | "BIGINT" | "SMALLINT" => match val {
            Value::Integer(i) => Value::Integer(i),
            Value::Real(f) => Value::Integer(f as i64),
            Value::Text(s) => {
                let t = s.trim();
                if let Ok(i) = t.parse::<i64>() {
                    Value::Integer(i)
                } else if let Ok(f) = t.parse::<f64>() {
                    Value::Integer(f as i64)
                } else {
                    Value::Integer(0)
                }
            }
            Value::Blob(_) | Value::Vector(_) => Value::Integer(0),
            Value::Null => Value::Null,
        },
        "REAL" | "FLOAT" | "DOUBLE" | "NUMERIC" => match val {
            Value::Real(f) => Value::Real(f),
            Value::Integer(i) => Value::Real(i as f64),
            Value::Text(s) => Value::Real(s.trim().parse::<f64>().unwrap_or(0.0)),
            Value::Blob(_) | Value::Vector(_) => Value::Real(0.0),
            Value::Null => Value::Null,
        },
        "BOOLEAN" | "BOOL" => match val {
            Value::Integer(i) => Value::Integer(i64::from(i != 0)),
            Value::Real(f) => Value::Integer(i64::from(f != 0.0)),
            Value::Text(s) => Value::Integer(i64::from(matches!(
                s.trim().to_ascii_lowercase().as_str(),
                "true" | "1" | "t" | "yes" | "y"
            ))),
            Value::Blob(b) => Value::Integer(i64::from(!b.is_empty())),
            Value::Vector(v) => Value::Integer(i64::from(!v.is_empty())),
            Value::Null => Value::Null,
        },
        "BLOB" => match val {
            Value::Blob(b) => Value::Blob(b),
            Value::Text(s) => Value::Blob(s.into_bytes()),
            other => Value::Blob(other.to_string().into_bytes()),
        },
        _ => match val {
            Value::Text(s) => Value::Text(s),
            other => Value::Text(other.to_string()),
        },
    }
}

fn try_eval_case_expr(
    row: &Row,
    trimmed: &str,
    udfs: Option<&crate::sql::udf::UdfRegistry>,
) -> Result<Option<Value>> {
    let bytes = trimmed.as_bytes();
    if bytes.len() < 7
        || !keyword_at(bytes, 0, b"CASE")
        || !keyword_at(bytes, bytes.len() - 3, b"END")
    {
        return Ok(None);
    }

    // Verify outer CASE ... END wraps the entire expression (case_depth stays > 0 until final END).
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut case_depth = 0usize;
    let mut in_sq = false;
    let mut in_dq = false;
    let mut when_positions = Vec::new();
    let mut then_positions = Vec::new();
    let mut else_pos = None;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_dq => in_sq = !in_sq,
            b'"' if !in_sq => in_dq = !in_dq,
            b'(' if !in_sq && !in_dq => paren_depth += 1,
            b')' if !in_sq && !in_dq => paren_depth = paren_depth.saturating_sub(1),
            b'[' if !in_sq && !in_dq => bracket_depth += 1,
            b']' if !in_sq && !in_dq => bracket_depth = bracket_depth.saturating_sub(1),
            _ if !in_sq && !in_dq => {
                if keyword_at(bytes, i, b"CASE") {
                    case_depth += 1;
                    i += 4;
                    continue;
                } else if keyword_at(bytes, i, b"END") {
                    case_depth = case_depth.saturating_sub(1);
                    if case_depth == 0 {
                        if i + 3 != bytes.len() {
                            return Ok(None);
                        }
                        break;
                    }
                    i += 3;
                    continue;
                } else if case_depth == 1 && paren_depth == 0 && bracket_depth == 0 {
                    if keyword_at(bytes, i, b"WHEN") {
                        when_positions.push(i);
                        i += 4;
                        continue;
                    } else if keyword_at(bytes, i, b"THEN") {
                        then_positions.push(i);
                        i += 4;
                        continue;
                    } else if keyword_at(bytes, i, b"ELSE") {
                        else_pos = Some(i);
                        i += 4;
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }

    if when_positions.is_empty() || when_positions.len() != then_positions.len() {
        return Ok(None);
    }

    let operand_raw = trimmed[4..when_positions[0]].trim();
    let operand_val = if operand_raw.is_empty() {
        None
    } else {
        Some(eval_func_arg(row, operand_raw, udfs)?)
    };

    let end_kw = bytes.len() - 3;
    for idx in 0..when_positions.len() {
        let w_pos = when_positions[idx];
        let t_pos = then_positions[idx];
        if t_pos < w_pos + 4 {
            return Ok(None);
        }
        let cond_sql = trimmed[w_pos + 4..t_pos].trim();
        let next_bound = if idx + 1 < when_positions.len() {
            when_positions[idx + 1]
        } else {
            else_pos.unwrap_or(end_kw)
        };
        if next_bound < t_pos + 4 {
            return Ok(None);
        }
        let res_sql = trimmed[t_pos + 4..next_bound].trim();

        let matched = if let Some(ref op_v) = operand_val {
            let cmp_v = eval_func_arg(row, cond_sql, udfs)?;
            !op_v.is_null() && matches_condition(op_v, &BinaryOp::Equals, &cmp_v)
        } else {
            let toks = crate::sql::lexer::tokenize(cond_sql)?;
            let mut cur = 0usize;
            let where_ast = crate::sql::parser::parse_where_expr_public(&toks, &mut cur)?;
            if cur != toks.len() {
                return Err(Error::SqlSyntax(format!(
                    "Unexpected token in CASE WHEN condition: '{cond_sql}'"
                )));
            }
            let resolved = where_expr_to_simple_resolved(&where_ast);
            row_matches_resolved_with_udf(row, &resolved, udfs)
        };

        if matched {
            return eval_func_arg(row, res_sql, udfs).map(Some);
        }
    }

    if let Some(ep) = else_pos {
        let else_sql = trimmed[ep + 4..end_kw].trim();
        return eval_func_arg(row, else_sql, udfs).map(Some);
    }

    Ok(Some(Value::Null))
}

fn where_expr_to_simple_resolved(expr: &crate::sql::parser::WhereExpr) -> ResolvedWhereExpr {
    use crate::sql::parser::WhereExpr;
    match expr {
        WhereExpr::Condition(c) => ResolvedWhereExpr::Condition(c.clone()),
        WhereExpr::ColumnComparison {
            left_col,
            op,
            right_col,
        } => ResolvedWhereExpr::ColumnComparison {
            left_col: left_col.clone(),
            op: op.clone(),
            right_col: right_col.clone(),
        },
        WhereExpr::InList {
            column,
            values,
            negated,
        } => ResolvedWhereExpr::InList {
            column: column.clone(),
            values: values.clone(),
            negated: *negated,
        },
        WhereExpr::And(l, r) => ResolvedWhereExpr::And(
            Box::new(where_expr_to_simple_resolved(l)),
            Box::new(where_expr_to_simple_resolved(r)),
        ),
        WhereExpr::Or(l, r) => ResolvedWhereExpr::Or(
            Box::new(where_expr_to_simple_resolved(l)),
            Box::new(where_expr_to_simple_resolved(r)),
        ),
        WhereExpr::InSubquery { .. }
        | WhereExpr::Exists { .. }
        | WhereExpr::ScalarSubqueryComparison { .. }
        | WhereExpr::SubqueryComparison { .. } => ResolvedWhereExpr::Boolean(false),
    }
}
