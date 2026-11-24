//! SQL Statement Executor connecting the AST, Catalog, B+Tree, Vector, and Graph engines.

use crate::btree::{BTreeStorage, engine};
use crate::error::{Error, Result};
use crate::graph::GraphEngine;
use crate::memory::bm25::Bm25Index;
use crate::pager::Pager;
use crate::sql::catalog::{
    Catalog, ColumnDef, DataType, DefaultValue, ForeignKeyAction, IndexDef, TableDef, TriggerDef,
    TriggerEvent, TriggerTiming,
};
use crate::sql::codec::{
    decode_row, decode_row_shared, decode_row_shared_projected, decode_row_values_into_projected,
    encode_row, encode_row_into,
};
use crate::sql::lexer::tokenize;
use crate::sql::parser::{
    BinaryOp, JoinType, OnConflict, Statement, WhereCondition, WhereExpr, parse_tokens,
};
use crate::traits::{HashableValue, Row, Value, VectorIndexEngine, col_name_matches};
use crate::vector::{DistanceMetric, HnswIndex};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

mod constraints;
mod cte_explain;
mod dml;
mod eval;
mod persistence;
mod query;
mod types;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub(crate) use eval::*;
#[allow(unused_imports)]
pub use eval::{
    matches_condition, project_row, project_row_with_udf, row_matches_conditions,
    row_matches_resolved, row_matches_resolved_with_udf, sql_like_match, sql_match_bm25,
    try_eval_scalar_expr,
};
#[allow(unused_imports)]
pub(crate) use types::*;
#[allow(unused_imports)]
pub use types::{
    FastIntBuildHasher, FastIntHasher, INDEX_BUCKET_MAGIC, INDEX_BUCKET_VERSION, InValueLookup,
    IndexBucket, IndexPostingEntry, IntEntry, JoinOutputCol, ResolvedWhereExpr,
    SimdJoinBloomFilter, TemporalRecord, VectorSnapshotRecord, plan_cbo_inner_join_order,
    resolve_table_col_index, value_to_index_key,
};

/// The SQL and multi-model execution engine for TapirusDB
#[derive(Debug, Clone)]
pub struct SQLExecutor {
    catalog: Catalog,
    btree: BTreeStorage,
    vector_indexes: HashMap<String, HnswIndex>,
    fts_indexes: HashMap<String, Bm25Index>,
    graph: GraphEngine,
    last_temporal_timestamp: u64,
    /// Default system versioning flag for new tables or session-level time travel
    pub default_system_versioning: bool,
    /// Whether FOREIGN KEY constraints are enforced (`PRAGMA foreign_keys = ON|OFF`).
    pub foreign_keys_enabled: bool,
    trigger_depth: usize,
    fk_cascade_depth: usize,
    last_insert_rowid: i64,
    statement_cache: HashMap<String, Statement>,
    statement_cache_order: std::collections::VecDeque<String>,
    predicate_cache: HashMap<String, WhereExpr>,
    active_ctes: HashMap<String, Vec<Row>>,
    insert_scratch_buf: Vec<u8>,
    insert_scratch_vals: Vec<Value>,
    /// Thread-safe registry for user-defined functions (UDF) and standard scalar extensions
    pub udf_registry: crate::sql::udf::UdfRegistry,
}

impl SQLExecutor {
    /// Initialize a new SQLExecutor by loading the catalog from the Pager
    pub fn new(pager: &mut Pager) -> Result<Self> {
        let catalog = Catalog::load(pager)?;
        let btree = BTreeStorage::new();
        let vector_indexes = HashMap::new();

        Ok(Self {
            catalog,
            btree,
            vector_indexes,
            fts_indexes: HashMap::new(),
            graph: GraphEngine::new(),
            last_temporal_timestamp: 0,
            default_system_versioning: false,
            foreign_keys_enabled: true,
            trigger_depth: 0,
            fk_cascade_depth: 0,
            last_insert_rowid: 0,
            statement_cache: HashMap::new(),
            statement_cache_order: std::collections::VecDeque::new(),
            predicate_cache: HashMap::new(),
            active_ctes: HashMap::new(),
            insert_scratch_buf: Vec::with_capacity(256),
            insert_scratch_vals: Vec::with_capacity(16),
            udf_registry: crate::sql::udf::UdfRegistry::new(),
        })
    }

    /// Register a custom scalar UDF
    pub fn register_scalar_function<F>(&mut self, name: &str, func: F)
    where
        F: Fn(&[Value]) -> Result<Value> + Send + Sync + 'static,
    {
        self.udf_registry.register(name, func);
    }

    /// Access reference to the UDF registry
    pub fn udf_registry(&self) -> &crate::sql::udf::UdfRegistry {
        &self.udf_registry
    }

    /// Return the row ID assigned by the most recent successful `INSERT`.
    pub fn last_insert_rowid(&self) -> i64 {
        self.last_insert_rowid
    }

    /// Reload catalog, graph, and indexes from disk/pager into executor
    pub fn reload_from_pager(&mut self, pager: &mut Pager) -> Result<()> {
        self.catalog = Catalog::load(pager)?;
        self.fts_indexes.clear();
        self.load_graph_from_disk(pager)?;
        self.load_vector_indexes_from_disk(pager)?;
        Ok(())
    }

    /// Capture a lightweight read-only snapshot clone skipping write scratch buffers and parser cache.
    pub(crate) fn clone_for_snapshot(&self) -> Self {
        Self {
            catalog: self.catalog.clone(),
            btree: self.btree.clone(),
            vector_indexes: self.vector_indexes.clone(),
            fts_indexes: self.fts_indexes.clone(),
            graph: self.graph.clone(),
            last_temporal_timestamp: self.last_temporal_timestamp,
            default_system_versioning: self.default_system_versioning,
            foreign_keys_enabled: self.foreign_keys_enabled,
            trigger_depth: 0,
            fk_cascade_depth: 0,
            last_insert_rowid: 0,
            statement_cache: HashMap::new(),
            statement_cache_order: std::collections::VecDeque::new(),
            predicate_cache: HashMap::new(),
            active_ctes: self.active_ctes.clone(),
            insert_scratch_buf: Vec::new(),
            insert_scratch_vals: Vec::new(),
            udf_registry: self.udf_registry.clone(),
        }
    }

    /// Project a row applying scalar functions and registered UDFs
    pub fn project_row(&self, row: &Row, requested_cols: &[String]) -> Result<Row> {
        project_row_with_udf(row, requested_cols, Some(&self.udf_registry))
    }

    /// Parse SQL query string into Statement using an internal LRU statement cache
    pub fn get_or_parse_statement(&mut self, sql: &str) -> Result<Option<Statement>> {
        if let Some(stmt) = self.statement_cache.get(sql) {
            return Ok(Some(stmt.clone()));
        }
        let tokens = crate::sql::lexer::tokenize(sql)?;
        if tokens.is_empty() {
            return Ok(None);
        }
        let stmt = crate::sql::parser::parse_tokens(&tokens)?;
        if self.statement_cache.len() >= 256
            && let Some(oldest) = self.statement_cache_order.pop_front()
        {
            self.statement_cache.remove(&oldest);
        }
        self.statement_cache.insert(sql.to_string(), stmt.clone());
        self.statement_cache_order.push_back(sql.to_string());
        Ok(Some(stmt))
    }

    /// Parse and cache SQL WHERE/CHECK/partial-index predicate ASTs so bulk DML skips per-row lex/parse.
    pub(crate) fn parse_cached_where_expr(&mut self, sql: &str) -> Result<WhereExpr> {
        if let Some(expr) = self.predicate_cache.get(sql) {
            return Ok(expr.clone());
        }
        let tokens = tokenize(sql)?;
        let mut cursor = 0usize;
        let expr = crate::sql::parser::parse_where_expr_public(&tokens, &mut cursor)?;
        if self.predicate_cache.len() >= 256 {
            self.predicate_cache.clear();
        }
        self.predicate_cache.insert(sql.to_string(), expr.clone());
        Ok(expr)
    }

    /// Return reference to graph engine
    pub fn graph(&self) -> &GraphEngine {
        &self.graph
    }

    /// Return mutable reference to graph engine
    pub fn graph_mut(&mut self) -> &mut GraphEngine {
        &mut self.graph
    }

    /// Recursively resolve a WhereExpr into a ResolvedWhereExpr, executing uncorrelated subqueries
    /// and marking correlated subqueries for per-outer-row memoized execution.
    pub fn resolve_where_expr(
        &mut self,
        pager: &mut Pager,
        expr: &WhereExpr,
    ) -> Result<ResolvedWhereExpr> {
        self.resolve_where_expr_for_tables(pager, expr, &[])
    }

    pub(crate) fn resolve_where_expr_for_tables(
        &mut self,
        pager: &mut Pager,
        expr: &WhereExpr,
        outer_tables: &[&TableDef],
    ) -> Result<ResolvedWhereExpr> {
        match expr {
            WhereExpr::Condition(cond) => Ok(ResolvedWhereExpr::Condition(cond.clone())),
            WhereExpr::ColumnComparison {
                left_col,
                op,
                right_col,
            } => Ok(ResolvedWhereExpr::ColumnComparison {
                left_col: left_col.clone(),
                op: op.clone(),
                right_col: right_col.clone(),
            }),
            WhereExpr::And(left, right) => {
                let l = self.resolve_where_expr_for_tables(pager, left, outer_tables)?;
                let r = self.resolve_where_expr_for_tables(pager, right, outer_tables)?;
                Ok(ResolvedWhereExpr::And(Box::new(l), Box::new(r)))
            }
            WhereExpr::Or(left, right) => {
                let l = self.resolve_where_expr_for_tables(pager, left, outer_tables)?;
                let r = self.resolve_where_expr_for_tables(pager, right, outer_tables)?;
                Ok(ResolvedWhereExpr::Or(Box::new(l), Box::new(r)))
            }
            WhereExpr::InList {
                column,
                values,
                negated,
            } => Ok(ResolvedWhereExpr::InSet {
                column: column.clone(),
                values: values.clone(),
                lookup: Arc::new(InValueLookup::from_values(values)),
                negated: *negated,
            }),
            WhereExpr::InSubquery {
                column,
                subquery,
                negated,
            } => {
                if !outer_tables.is_empty() {
                    let mut outer_refs = Vec::new();
                    collect_statement_outer_refs(
                        subquery,
                        outer_tables,
                        &self.catalog,
                        &mut outer_refs,
                    );
                    if !outer_refs.is_empty() {
                        return Ok(ResolvedWhereExpr::CorrelatedInSubquery {
                            column: column.clone(),
                            subquery: subquery.clone(),
                            negated: *negated,
                            outer_refs,
                        });
                    }
                }
                let sub_rows = self.query(pager, *subquery.clone())?;
                let mut values = Vec::with_capacity(sub_rows.len());
                for r in sub_rows {
                    if let Some(v) = r.values().first() {
                        values.push(v.clone());
                    }
                }
                let lookup = Arc::new(InValueLookup::from_values(&values));
                Ok(ResolvedWhereExpr::InSet {
                    column: column.clone(),
                    values,
                    lookup,
                    negated: *negated,
                })
            }
            WhereExpr::Exists { subquery, negated } => {
                if !outer_tables.is_empty() {
                    let mut outer_refs = Vec::new();
                    collect_statement_outer_refs(
                        subquery,
                        outer_tables,
                        &self.catalog,
                        &mut outer_refs,
                    );
                    if !outer_refs.is_empty() {
                        return Ok(ResolvedWhereExpr::CorrelatedExists {
                            subquery: subquery.clone(),
                            negated: *negated,
                            outer_refs,
                        });
                    }
                }
                let mut sub = *subquery.clone();
                inject_exists_limit(&mut sub);
                let sub_rows = self.query(pager, sub)?;
                let exists = !sub_rows.is_empty();
                let matches = if *negated { !exists } else { exists };
                Ok(ResolvedWhereExpr::Boolean(matches))
            }
            WhereExpr::ScalarSubqueryComparison {
                column,
                op,
                subquery,
            } => {
                if !outer_tables.is_empty() {
                    let mut outer_refs = Vec::new();
                    collect_statement_outer_refs(
                        subquery,
                        outer_tables,
                        &self.catalog,
                        &mut outer_refs,
                    );
                    if !outer_refs.is_empty() {
                        return Ok(ResolvedWhereExpr::CorrelatedScalarSubquery {
                            column: Some(column.clone()),
                            op: op.clone(),
                            subquery: subquery.clone(),
                            literal: None,
                            outer_refs,
                        });
                    }
                }
                let val = self.eval_scalar_subquery_value(pager, subquery.as_ref().clone())?;
                Ok(ResolvedWhereExpr::Condition(WhereCondition {
                    column: column.clone(),
                    op: op.clone(),
                    value: val,
                }))
            }
            WhereExpr::SubqueryComparison {
                subquery,
                op,
                value,
            } => {
                if !outer_tables.is_empty() {
                    let mut outer_refs = Vec::new();
                    collect_statement_outer_refs(
                        subquery,
                        outer_tables,
                        &self.catalog,
                        &mut outer_refs,
                    );
                    if !outer_refs.is_empty() {
                        return Ok(ResolvedWhereExpr::CorrelatedScalarSubquery {
                            column: None,
                            op: op.clone(),
                            subquery: subquery.clone(),
                            literal: Some(value.clone()),
                            outer_refs,
                        });
                    }
                }
                let sub_val = self.eval_scalar_subquery_value(pager, subquery.as_ref().clone())?;
                Ok(ResolvedWhereExpr::Boolean(matches_condition(
                    &sub_val, op, value,
                )))
            }
        }
    }

    pub(crate) fn eval_scalar_subquery_value(
        &mut self,
        pager: &mut Pager,
        mut subquery: Statement,
    ) -> Result<Value> {
        inject_scalar_limit(&mut subquery);
        let sub_rows = self.query(pager, subquery)?;
        if sub_rows.len() > 1 {
            return Err(Error::SqlSyntax(
                "Scalar subquery returned more than 1 row".into(),
            ));
        }
        if let Some(first) = sub_rows.first() {
            if first.values().len() > 1 {
                return Err(Error::SqlSyntax(
                    "Scalar subquery must return a single column".into(),
                ));
            }
            Ok(first.values().first().cloned().unwrap_or(Value::Null))
        } else {
            Ok(Value::Null)
        }
    }

    pub(crate) fn eval_resolved_where_with_subqueries(
        &mut self,
        pager: &mut Pager,
        row: &Row,
        expr: &ResolvedWhereExpr,
        outer_tables: &[&TableDef],
        memo: &mut CorrelatedSubqueryMemo,
    ) -> Result<bool> {
        if !resolved_where_has_correlated(expr) {
            return Ok(row_matches_resolved(row, expr));
        }
        match expr {
            ResolvedWhereExpr::And(l, r) => {
                if !self.eval_resolved_where_with_subqueries(pager, row, l, outer_tables, memo)? {
                    return Ok(false);
                }
                self.eval_resolved_where_with_subqueries(pager, row, r, outer_tables, memo)
            }
            ResolvedWhereExpr::Or(l, r) => {
                if self.eval_resolved_where_with_subqueries(pager, row, l, outer_tables, memo)? {
                    return Ok(true);
                }
                self.eval_resolved_where_with_subqueries(pager, row, r, outer_tables, memo)
            }
            ResolvedWhereExpr::CorrelatedExists {
                subquery,
                negated,
                outer_refs,
            } => {
                let cache_id = (subquery.as_ref() as *const Statement) as usize;
                let key = build_outer_ref_key(row, outer_refs);
                let exists = if let Some(&cached) = memo.exists_cache.get(&(cache_id, key.clone()))
                {
                    cached
                } else {
                    let sub_opt =
                        substitute_outer_statement(subquery, row, outer_tables, &self.catalog);
                    let res = match sub_opt {
                        None => false,
                        Some(mut sub) => {
                            inject_exists_limit(&mut sub);
                            !self.query(pager, sub)?.is_empty()
                        }
                    };
                    memo.exists_cache.insert((cache_id, key), res);
                    res
                };
                Ok(if *negated { !exists } else { exists })
            }
            ResolvedWhereExpr::CorrelatedInSubquery {
                column,
                subquery,
                negated,
                outer_refs,
            } => {
                let lhs_val = match row.get_field_or_json_path(column) {
                    Some(v) => v,
                    None => match try_eval_scalar_expr(row, column, None) {
                        Ok(Some(v)) => v,
                        _ => return Ok(false),
                    },
                };
                if lhs_val.is_null() {
                    return Ok(false);
                }
                let cache_id = (subquery.as_ref() as *const Statement) as usize;
                let key = build_outer_ref_key(row, outer_refs);
                let lookup = if let Some(cached) = memo.in_cache.get(&(cache_id, key.clone())) {
                    cached.clone()
                } else {
                    let sub_opt =
                        substitute_outer_statement(subquery, row, outer_tables, &self.catalog);
                    let computed = match sub_opt {
                        None => Arc::new(InValueLookup::from_values(&[])),
                        Some(sub) => {
                            let sub_rows = self.query(pager, sub)?;
                            let mut values = Vec::with_capacity(sub_rows.len());
                            for r in sub_rows {
                                if let Some(v) = r.values().first() {
                                    values.push(v.clone());
                                }
                            }
                            Arc::new(InValueLookup::from_values(&values))
                        }
                    };
                    memo.in_cache.insert((cache_id, key), computed.clone());
                    computed
                };
                let in_set = lookup.contains(&lhs_val);
                Ok(if *negated {
                    !lookup.has_null && !in_set
                } else {
                    in_set
                })
            }
            ResolvedWhereExpr::CorrelatedScalarSubquery {
                column,
                op,
                subquery,
                literal,
                outer_refs,
            } => {
                let cache_id = (subquery.as_ref() as *const Statement) as usize;
                let key = build_outer_ref_key(row, outer_refs);
                let sub_val = if let Some(cached) = memo.scalar_cache.get(&(cache_id, key.clone()))
                {
                    cached.clone()
                } else {
                    let sub_opt =
                        substitute_outer_statement(subquery, row, outer_tables, &self.catalog);
                    let computed = match sub_opt {
                        None => Value::Null,
                        Some(sub) => self.eval_scalar_subquery_value(pager, sub)?,
                    };
                    memo.scalar_cache.insert((cache_id, key), computed.clone());
                    computed
                };
                if let Some(col) = column {
                    let lhs_val = match row.get_field_or_json_path(col) {
                        Some(v) => v,
                        None => match try_eval_scalar_expr(row, col, Some(&self.udf_registry)) {
                            Ok(Some(v)) => v,
                            _ => return Ok(false),
                        },
                    };
                    Ok(matches_condition(&lhs_val, op, &sub_val))
                } else {
                    let rhs_val = literal.as_ref().unwrap_or(&Value::Null);
                    Ok(matches_condition(&sub_val, op, rhs_val))
                }
            }
            _ => Ok(row_matches_resolved(row, expr)),
        }
    }

    pub(crate) fn retain_rows_with_correlated_where(
        &mut self,
        pager: &mut Pager,
        rows: &mut Vec<Row>,
        expr: &ResolvedWhereExpr,
        outer_tables: &[&TableDef],
    ) -> Result<()> {
        if !resolved_where_has_correlated(expr) {
            rows.retain(|r| row_matches_resolved(r, expr));
            return Ok(());
        }
        let mut memo = CorrelatedSubqueryMemo::default();
        let old = std::mem::take(rows);
        rows.reserve(old.len());
        for row in old {
            if self.eval_resolved_where_with_subqueries(
                pager,
                &row,
                expr,
                outer_tables,
                &mut memo,
            )? {
                rows.push(row);
            }
        }
        Ok(())
    }

    pub(crate) fn prepare_projection_plan(
        &mut self,
        pager: &mut Pager,
        columns: &[String],
        outer_tables: &[&TableDef],
    ) -> Result<Vec<PreparedProjColumn>> {
        let mut plan = Vec::with_capacity(columns.len());
        for col in columns {
            let (expr, alias) = parse_col_and_alias(col);
            let out_name = alias.unwrap_or(expr).to_string();
            let raw_pieces = crate::sql::parser::parse_proj_subquery_pieces(expr);
            let mut pieces = Vec::with_capacity(raw_pieces.len());
            for piece in raw_pieces {
                match piece {
                    crate::sql::parser::ProjSubqueryPiece::Raw(s) => {
                        pieces.push(PreparedProjPiece::Raw(s));
                    }
                    crate::sql::parser::ProjSubqueryPiece::Subquery(stmt) => {
                        let mut outer_refs = Vec::new();
                        if !outer_tables.is_empty() {
                            collect_statement_outer_refs(
                                &stmt,
                                outer_tables,
                                &self.catalog,
                                &mut outer_refs,
                            );
                        }
                        if outer_refs.is_empty() {
                            let val = self.eval_scalar_subquery_value(pager, *stmt)?;
                            pieces.push(PreparedProjPiece::Uncorrelated(val));
                        } else {
                            pieces.push(PreparedProjPiece::Correlated { stmt, outer_refs });
                        }
                    }
                }
            }
            let is_pure_subquery =
                pieces.len() == 1 && !matches!(pieces[0], PreparedProjPiece::Raw(_));
            plan.push(PreparedProjColumn {
                raw_col: col.clone(),
                expr: expr.to_string(),
                out_name,
                pieces,
                is_pure_subquery,
            });
        }
        Ok(plan)
    }

    pub(crate) fn eval_prepared_proj_piece(
        &mut self,
        pager: &mut Pager,
        row: &Row,
        piece: &PreparedProjPiece,
        outer_tables: &[&TableDef],
        memo: &mut CorrelatedSubqueryMemo,
    ) -> Result<Option<Value>> {
        match piece {
            PreparedProjPiece::Raw(_) => Ok(None),
            PreparedProjPiece::Uncorrelated(v) => Ok(Some(v.clone())),
            PreparedProjPiece::Correlated { stmt, outer_refs } => {
                let cache_id = (stmt.as_ref() as *const Statement) as usize;
                let key = build_outer_ref_key(row, outer_refs);
                if let Some(cached) = memo.scalar_cache.get(&(cache_id, key.clone())) {
                    return Ok(Some(cached.clone()));
                }
                let sub_opt = substitute_outer_statement(stmt, row, outer_tables, &self.catalog);
                let computed = match sub_opt {
                    None => Value::Null,
                    Some(sub) => self.eval_scalar_subquery_value(pager, sub)?,
                };
                memo.scalar_cache.insert((cache_id, key), computed.clone());
                Ok(Some(computed))
            }
        }
    }

    pub(crate) fn eval_prepared_projection_col(
        &mut self,
        pager: &mut Pager,
        row: &Row,
        col_plan: &PreparedProjColumn,
        outer_tables: &[&TableDef],
        memo: &mut CorrelatedSubqueryMemo,
    ) -> Result<Value> {
        if col_plan.pieces.is_empty() {
            return eval_projected_expr(
                row,
                &col_plan.expr,
                &col_plan.raw_col,
                Some(&self.udf_registry),
            );
        }
        if col_plan.is_pure_subquery {
            return Ok(self
                .eval_prepared_proj_piece(pager, row, &col_plan.pieces[0], outer_tables, memo)?
                .unwrap_or(Value::Null));
        }
        let mut substituted = String::new();
        for piece in &col_plan.pieces {
            if !substituted.is_empty() {
                substituted.push(' ');
            }
            match piece {
                PreparedProjPiece::Raw(s) => substituted.push_str(s),
                _ => {
                    let v = self
                        .eval_prepared_proj_piece(pager, row, piece, outer_tables, memo)?
                        .unwrap_or(Value::Null);
                    match v {
                        Value::Null => substituted.push_str("NULL"),
                        Value::Integer(i) => substituted.push_str(&i.to_string()),
                        Value::Real(f) => substituted.push_str(&f.to_string()),
                        Value::Text(s) => {
                            substituted.push('\'');
                            substituted.push_str(&s.replace('\'', "''"));
                            substituted.push('\'');
                        }
                        other => substituted.push_str(&other.to_string()),
                    }
                }
            }
        }
        eval_projected_expr(
            row,
            &substituted,
            &col_plan.raw_col,
            Some(&self.udf_registry),
        )
    }

    pub(crate) fn project_row_with_plan(
        &mut self,
        pager: &mut Pager,
        row: &Row,
        plan: &[PreparedProjColumn],
        outer_tables: &[&TableDef],
        memo: &mut CorrelatedSubqueryMemo,
    ) -> Result<Row> {
        let mut names = Vec::with_capacity(plan.len());
        let mut vals = Vec::with_capacity(plan.len());
        for col_plan in plan {
            names.push(col_plan.out_name.clone());
            vals.push(self.eval_prepared_projection_col(
                pager,
                row,
                col_plan,
                outer_tables,
                memo,
            )?);
        }
        Ok(Row::new(names, vals))
    }

    pub(crate) fn enrich_rows_with_order_plan(
        &mut self,
        pager: &mut Pager,
        rows: &mut [Row],
        plan: &[PreparedProjColumn],
        order_by: &[crate::sql::parser::OrderByClause],
        outer_tables: &[&TableDef],
        memo: &mut CorrelatedSubqueryMemo,
    ) -> Result<()> {
        if rows.is_empty() || order_by.is_empty() {
            return Ok(());
        }
        let mut missing_indices = Vec::new();
        for ord in order_by {
            if rows[0].get_value(&ord.column).is_some() {
                continue;
            }
            if let Some((idx, _)) = plan
                .iter()
                .enumerate()
                .find(|(_, p)| p.out_name.eq_ignore_ascii_case(&ord.column))
                && !missing_indices.contains(&idx)
            {
                missing_indices.push(idx);
            }
        }
        if missing_indices.is_empty() {
            return Ok(());
        }
        for row in rows.iter_mut() {
            let mut new_cols = row.columns().to_vec();
            let mut new_vals = row.values().to_vec();
            for &idx in &missing_indices {
                let val =
                    self.eval_prepared_projection_col(pager, row, &plan[idx], outer_tables, memo)?;
                new_cols.push(plan[idx].out_name.clone());
                new_vals.push(val);
            }
            *row = Row::new(new_cols, new_vals);
        }
        Ok(())
    }

    /// Execute a non-query SQL or Graph command (CREATE, INSERT, etc.)
    pub fn execute(&mut self, pager: &mut Pager, stmt: Statement) -> Result<usize> {
        if Self::is_mutation(&stmt) {
            self.atomic_statement(pager, |executor, pager| executor.execute_inner(pager, stmt))
        } else {
            self.execute_inner(pager, stmt)
        }
    }

    fn is_mutation(stmt: &Statement) -> bool {
        matches!(
            stmt,
            Statement::CreateTable { .. }
                | Statement::Insert { .. }
                | Statement::InsertMany { .. }
                | Statement::Update { .. }
                | Statement::Delete { .. }
                | Statement::GraphInsertNode { .. }
                | Statement::GraphInsertEdge { .. }
                | Statement::CreateIndex { .. }
                | Statement::DropTable { .. }
                | Statement::DropIndex { .. }
                | Statement::AlterTableAddColumn { .. }
                | Statement::AlterTableDropColumn { .. }
                | Statement::AlterTableRenameColumn { .. }
                | Statement::AlterTableRenameTable { .. }
                | Statement::CreateView { .. }
                | Statement::DropView { .. }
                | Statement::CreateTrigger { .. }
                | Statement::DropTrigger { .. }
                | Statement::CopyFrom { .. }
        )
    }

    pub(crate) fn atomic_statement<T>(
        &mut self,
        pager: &mut Pager,
        operation: impl FnOnce(&mut Self, &mut Pager) -> Result<T>,
    ) -> Result<T> {
        let own = !pager.is_in_transaction();
        let savepoint = format!("__tapirus_statement_{}", pager.savepoint_depth());
        if own {
            pager.begin_transaction()?;
        } else {
            pager.create_savepoint(&savepoint)?;
        }
        match operation(self, pager) {
            Ok(value) => {
                let commit = if own {
                    pager.commit_transaction()
                } else {
                    pager.release_savepoint(&savepoint)
                };
                if let Err(error) = commit {
                    if pager.is_in_transaction() {
                        let _ = pager.rollback_transaction();
                    }
                    self.reload_from_pager(pager)?;
                    return Err(error);
                }
                Ok(value)
            }
            Err(error) => {
                if own {
                    pager.rollback_transaction()?;
                } else {
                    pager.rollback_to_savepoint(&savepoint)?;
                    pager.release_savepoint(&savepoint)?;
                }
                self.reload_from_pager(pager)?;
                Err(error)
            }
        }
    }

    fn execute_inner(&mut self, pager: &mut Pager, stmt: Statement) -> Result<usize> {
        match stmt {
            Statement::InsertMany {
                table,
                columns,
                rows,
                conflict_action,
                returning,
            } => {
                let mut affected = 0;
                let mut returned = Vec::new();
                for values in rows {
                    let (count, result) = self.execute_insert(
                        pager,
                        table.clone(),
                        columns.clone(),
                        values,
                        conflict_action.clone(),
                        returning.clone(),
                    )?;
                    affected += count;
                    returned.extend(result);
                }
                Ok(affected)
            }
            Statement::Pragma { key, value } => {
                let k = key.to_lowercase();
                if k == "time_travel" || k == "system_versioning" || k == "temporal" {
                    if let Some(v) = value {
                        let val_lower = v.trim().to_lowercase();
                        self.default_system_versioning =
                            val_lower == "1" || val_lower == "true" || val_lower == "on";
                    }
                    Ok(if self.default_system_versioning { 1 } else { 0 })
                } else if k == "foreign_keys" {
                    if let Some(v) = value {
                        let val_lower = v.trim().to_lowercase();
                        self.foreign_keys_enabled =
                            val_lower == "1" || val_lower == "true" || val_lower == "on";
                    }
                    Ok(if self.foreign_keys_enabled { 1 } else { 0 })
                } else {
                    Ok(0)
                }
            }

            Statement::CreateTable {
                name,
                if_not_exists,
                columns,
                system_versioning,
                foreign_keys,
                check_constraints,
                unique_constraints,
                primary_key_columns,
                fts_config,
            } => {
                if self.catalog.table_exists(&name) {
                    if if_not_exists {
                        return Ok(0);
                    } else {
                        return Err(Error::TableExists(name));
                    }
                }

                // Allocate table root page
                let root_page = pager.allocate_page()?;
                let mut root_buf = vec![0u8; pager.page_size()];
                engine::init_leaf_page(&mut root_buf, root_page);
                pager.write_page(root_page, &root_buf)?;

                let mut table_def = TableDef::new(name.clone(), root_page, columns);
                table_def.system_versioning = system_versioning || self.default_system_versioning;
                table_def.foreign_keys = foreign_keys;
                table_def.check_constraints = check_constraints;
                table_def.unique_constraints = unique_constraints;
                table_def.primary_key_columns = primary_key_columns;
                table_def.fts_config = fts_config.clone();

                // If table has a vector column, prepare vector index
                if let Some((_, dims)) = table_def.vector_column() {
                    let index = HnswIndex::new(dims, DistanceMetric::Cosine);
                    self.vector_indexes.insert(name.to_lowercase(), index);
                }

                if fts_config.is_some() {
                    self.fts_indexes
                        .insert(name.to_lowercase(), Bm25Index::new());
                }

                self.catalog.create_table(pager, table_def)?;
                Ok(1)
            }

            Statement::Insert {
                table,
                columns,
                values,
                conflict_action,
                returning,
            } => {
                let (affected, _) =
                    self.execute_insert(pager, table, columns, values, conflict_action, returning)?;
                Ok(affected)
            }

            Statement::GraphInsertNode {
                id,
                label,
                properties,
            } => {
                let node = crate::graph::Node {
                    id,
                    label,
                    properties,
                    vector: None,
                };
                self.graph.restore_node(node.clone());
                self.persist_graph_node(pager, &node)?;
                Ok(1)
            }

            Statement::GraphInsertEdge {
                from_id,
                to_id,
                label,
                weight,
                properties,
            } => {
                let edge_id = self
                    .graph
                    .add_edge(from_id, to_id, label, weight, properties)?;
                if let Some(edge) = self.graph.get_edge(edge_id) {
                    let edge_clone = edge.clone();
                    self.persist_graph_edge(pager, &edge_clone)?;
                }
                Ok(1)
            }

            Statement::Update {
                table,
                assignments,
                assignment_exprs,
                where_clause,
                returning,
            } => {
                let (affected, _) = self.execute_update(
                    pager,
                    table,
                    assignments,
                    assignment_exprs,
                    where_clause,
                    returning,
                )?;
                Ok(affected)
            }

            Statement::Delete {
                table,
                where_clause,
                returning,
            } => {
                let (affected, _) = self.execute_delete(pager, table, where_clause, returning)?;
                Ok(affected)
            }

            Statement::BeginTransaction => {
                pager.begin_transaction()?;
                Ok(0)
            }

            Statement::CommitTransaction => {
                self.persist_vector_indexes_to_disk(pager)?;
                pager.commit_transaction()?;
                Ok(0)
            }

            Statement::RollbackTransaction => {
                pager.rollback_transaction()?;
                self.catalog = Catalog::load(pager)?;
                self.vector_indexes.clear();
                self.fts_indexes.clear();
                self.graph = GraphEngine::new();
                self.load_vector_indexes_from_disk(pager)?;
                self.load_graph_from_disk(pager)?;
                Ok(0)
            }

            Statement::CreateIndex {
                name,
                if_not_exists,
                table,
                column,
                columns,
                unique,
                where_clause,
                predicate_sql,
            } => {
                if if_not_exists && self.catalog.index_exists(&name) {
                    return Ok(0);
                }

                if self.catalog.index_exists(&name) {
                    return Err(Error::TableExists(format!("Index '{name}' already exists")));
                }

                let table_def = self
                    .catalog
                    .get_table(&table)
                    .cloned()
                    .ok_or_else(|| Error::TableNotFound(table.clone()))?;

                let target_cols = if columns.is_empty() {
                    vec![column.clone()]
                } else {
                    columns
                };
                validate_index_targets(&table_def, &target_cols, &table, &name)?;

                let resolved_predicate = match where_clause {
                    Some(ref expr) => Some(self.resolve_where_expr(pager, expr)?),
                    None => None,
                };

                let index_root = pager.allocate_page()?;
                let mut index_buf = vec![0u8; pager.page_size()];
                engine::init_leaf_page(&mut index_buf, index_root);
                pager.write_page(index_root, &index_buf)?;

                // Populate inverted index posting list with existing rows matching partial predicate (if any)
                let all_col_names = table_def.column_names();
                let mut buckets: HashMap<u64, IndexBucket> = HashMap::new();

                self.btree
                    .scan_with_ref(pager, table_def.root_page, |row_id, payload| {
                        let full_row = decode_row(payload, &all_col_names)?;
                        if let Some(ref pred) = resolved_predicate
                            && !row_matches_resolved(&full_row, pred)
                        {
                            return Ok(true);
                        }
                        if let Some((col_val, all_non_null)) =
                            extract_index_key_value(&full_row, &target_cols)
                        {
                            let key = value_to_index_key(&col_val);
                            let bucket = buckets.entry(key).or_default();
                            if unique
                                && all_non_null
                                && bucket.get_row_ids(&col_val).is_some_and(|r| !r.is_empty())
                            {
                                return Err(Error::ConstraintViolation(format!(
                                    "UNIQUE index '{name}' failed: duplicate key in {table}({})",
                                    target_cols.join(", ")
                                )));
                            }
                            bucket.add_row_id(col_val, row_id);
                        }
                        Ok(true)
                    })?;

                for (key, bucket) in buckets {
                    let idx_payload = bucket.to_bytes();
                    self.btree.insert(pager, index_root, key, &idx_payload)?;
                }

                let index_def = IndexDef::composite(name, table, target_cols, index_root, unique)
                    .with_predicate(predicate_sql);
                self.catalog.create_index(pager, index_def)?;

                Ok(1)
            }

            Statement::Vacuum { into } => {
                if let Some(dest_file) = into {
                    let pages_written = pager.backup_to(std::path::Path::new(&dest_file))?;
                    Ok(pages_written as usize)
                } else {
                    let cp = pager.checkpoint()?;
                    for index in self.vector_indexes.values_mut() {
                        index.vacuum();
                    }
                    let _ = self.persist_vector_indexes_to_disk(pager);
                    Ok(cp)
                }
            }

            Statement::DropTable { table, if_exists } => {
                let name_lower = table.to_lowercase();
                if let Some(table_def) = self.catalog.get_table(&name_lower).cloned() {
                    if self.foreign_keys_enabled {
                        for child_def in self.catalog.tables() {
                            if child_def.name.eq_ignore_ascii_case(&table) {
                                continue;
                            }
                            for fk in collect_table_foreign_keys(&child_def) {
                                if fk.ref_table.eq_ignore_ascii_case(&table) {
                                    let child_cols = child_def.column_names();
                                    self.btree.scan_with_ref(
                                        pager,
                                        child_def.root_page,
                                        |_row_id, payload| {
                                            let row = decode_row(payload, &child_cols)?;
                                            let has_non_null_ref = fk.columns.iter().all(|c| {
                                                !row.get_field_or_json_path(c)
                                                    .unwrap_or(Value::Null)
                                                    .is_null()
                                            });
                                            if has_non_null_ref {
                                                return Err(Error::ConstraintViolation(format!(
                                                    "FOREIGN KEY constraint failed on DROP TABLE '{table}': referenced by '{}'",
                                                    child_def.name
                                                )));
                                            }
                                            Ok(true)
                                        },
                                    )?;
                                }
                            }
                        }
                    }

                    let root_page = table_def.root_page;
                    let mut page_buf = pager.read_page(root_page)?;
                    engine::init_leaf_page(&mut page_buf, root_page);
                    pager.write_page(root_page, &page_buf)?;

                    // Drop any secondary indexes on this table
                    let index_names: Vec<String> = self
                        .catalog
                        .indexes()
                        .into_iter()
                        .filter(|i| i.table.eq_ignore_ascii_case(&table))
                        .map(|i| i.name)
                        .collect();
                    for idx_name in index_names {
                        let _ = self.catalog.drop_index(pager, &idx_name);
                    }

                    // Drop any triggers attached to this table
                    let trigger_names: Vec<String> = self
                        .catalog
                        .triggers()
                        .into_iter()
                        .filter(|t| t.table.eq_ignore_ascii_case(&table))
                        .map(|t| t.name)
                        .collect();
                    for trg_name in trigger_names {
                        let _ = self.catalog.drop_trigger(pager, &trg_name);
                    }

                    self.catalog.drop_table(pager, &table)?;
                    self.vector_indexes.remove(&name_lower);
                    self.fts_indexes.remove(&name_lower);
                    Ok(1)
                } else if if_exists {
                    Ok(0)
                } else {
                    Err(Error::TableNotFound(table))
                }
            }

            Statement::AlterTableAddColumn { table, column } => {
                let default_fill = column.default_value.as_ref().map(DefaultValue::materialize);
                let has_gen = column.generated_expr.is_some();
                self.catalog.add_column(pager, &table, column)?;
                if (default_fill.is_some() || has_gen)
                    && let Some(tdef) = self.catalog.get_table(&table).cloned()
                {
                    let col_count = tdef.columns.len();
                    let prev_cols: Vec<String> = tdef
                        .columns
                        .iter()
                        .take(col_count.saturating_sub(1))
                        .map(|c| c.name.clone())
                        .collect();
                    let cells = self.btree.scan(pager, tdef.root_page)?;
                    for cell in cells {
                        let row = decode_row(&cell.payload, &prev_cols)?;
                        let mut vals = row.values().to_vec();
                        if vals.len() < col_count {
                            vals.resize(col_count - 1, Value::Null);
                            vals.push(default_fill.clone().unwrap_or(Value::Null));
                            if has_gen {
                                self.compute_generated_columns(&table, &tdef.columns, &mut vals)?;
                            }
                            let new_payload = encode_row(&vals);
                            self.btree
                                .insert(pager, tdef.root_page, cell.row_id, &new_payload)?;
                        }
                    }
                }
                Ok(1)
            }

            Statement::AlterTableRenameTable { table, new_name } => {
                self.catalog.rename_table(pager, &table, &new_name)?;
                Ok(1)
            }

            Statement::AlterTableRenameColumn {
                table,
                old_name,
                new_name,
            } => {
                self.catalog
                    .rename_column(pager, &table, &old_name, &new_name)?;
                Ok(1)
            }

            Statement::AlterTableDropColumn { table, column } => {
                self.catalog.drop_column(pager, &table, &column)?;
                Ok(1)
            }

            Statement::Savepoint { name } => {
                pager.create_savepoint(&name)?;
                Ok(0)
            }

            Statement::RollbackToSavepoint { name } => {
                pager.rollback_to_savepoint(&name)?;
                self.catalog = Catalog::load(pager)?;
                self.vector_indexes.clear();
                self.fts_indexes.clear();
                self.graph = GraphEngine::new();
                self.load_vector_indexes_from_disk(pager)?;
                self.load_graph_from_disk(pager)?;
                Ok(0)
            }

            Statement::CreateTrigger {
                name,
                if_not_exists,
                table,
                timing,
                event,
                when_sql,
                body_sql,
            } => {
                if if_not_exists && self.catalog.trigger_exists(&name) {
                    return Ok(0);
                }
                if !self.catalog.table_exists(&table) {
                    return Err(Error::TableNotFound(table));
                }
                let trg = TriggerDef {
                    name,
                    table,
                    timing,
                    event,
                    when_sql,
                    body_sql,
                };
                self.catalog.create_trigger(pager, trg)?;
                Ok(1)
            }

            Statement::DropTrigger { name, if_exists } => {
                let dropped = self.catalog.drop_trigger(pager, &name)?;
                if dropped {
                    Ok(1)
                } else if if_exists {
                    Ok(0)
                } else {
                    Err(Error::TableNotFound(format!("Trigger '{name}' not found")))
                }
            }

            Statement::Raise { action, message } => Err(Error::ConstraintViolation(format!(
                "RAISE({action}): {message}"
            ))),

            Statement::ReleaseSavepoint { name } => {
                pager.release_savepoint(&name)?;
                Ok(0)
            }

            Statement::CreateView {
                name,
                if_not_exists,
                query_sql,
                ..
            } => {
                let name_lower = name.to_lowercase();
                if self.catalog.view_exists(&name_lower) {
                    if if_not_exists {
                        return Ok(0);
                    } else {
                        return Err(Error::TableExists(format!("View '{name}' already exists")));
                    }
                }
                if self.catalog.table_exists(&name_lower) {
                    return Err(Error::TableExists(format!(
                        "Table with name '{name}' already exists"
                    )));
                }

                let view_def = crate::sql::catalog::ViewDef::new(name, query_sql);
                self.catalog.create_view(pager, view_def)?;
                Ok(1)
            }

            Statement::DropView { name, if_exists } => {
                let dropped = self.catalog.drop_view(pager, &name)?;
                if dropped {
                    Ok(1)
                } else if if_exists {
                    Ok(0)
                } else {
                    Err(Error::TableNotFound(format!("View '{name}' not found")))
                }
            }

            Statement::DropIndex { name, if_exists } => {
                if let Some(idx_def) = self.catalog.get_index(&name).cloned() {
                    let root_page = idx_def.root_page;
                    if root_page > 0 {
                        let mut page_buf = pager.read_page(root_page)?;
                        engine::init_leaf_page(&mut page_buf, root_page);
                        pager.write_page(root_page, &page_buf)?;
                    }
                    self.catalog.drop_index(pager, &name)?;
                    Ok(1)
                } else if if_exists {
                    Ok(0)
                } else {
                    Err(Error::TableNotFound(format!("Index '{name}' not found")))
                }
            }

            Statement::Analyze { table } => {
                let tables_to_analyze: Vec<String> = if let Some(t) = table {
                    vec![t]
                } else {
                    self.catalog.tables().into_iter().map(|t| t.name).collect()
                };

                let mut analyzed_count = 0;
                for tname in tables_to_analyze {
                    if let Some(tdef) = self.catalog.get_table(&tname).cloned() {
                        let mut total_rows = 0usize;
                        let all_cols = tdef.column_names();
                        let mut distinct_map = HashMap::new();
                        let mut null_map = HashMap::new();
                        let mut total_bytes = 0usize;

                        for col in &all_cols {
                            distinct_map.insert(col.clone(), std::collections::HashSet::new());
                            null_map.insert(col.clone(), 0usize);
                        }

                        self.btree
                            .scan_with_ref(pager, tdef.root_page, |_row_id, payload| {
                                total_rows += 1;
                                total_bytes += payload.len();
                                if let Ok(row) = decode_row(payload, &all_cols) {
                                    for col in &all_cols {
                                        if let Some(v) = row.get_field_or_json_path(col) {
                                            if v.is_null() {
                                                if let Some(n) = null_map.get_mut(col) {
                                                    *n += 1;
                                                }
                                            } else if let Some(set) = distinct_map.get_mut(col) {
                                                let key = format!("{v:?}");
                                                set.insert(key);
                                            }
                                        }
                                    }
                                }
                                Ok(true)
                            })?;

                        let mut column_distinct = HashMap::new();
                        for (k, v) in distinct_map {
                            column_distinct.insert(k, v.len());
                        }

                        let page_count = (total_bytes / pager.page_size()).max(1);
                        let mut stats =
                            crate::sql::planner::TableStats::new(&tname, total_rows, page_count);
                        stats.column_distinct = column_distinct;
                        stats.column_nulls = null_map;
                        self.catalog.set_table_stats(&tname, stats);
                        analyzed_count += 1;
                    }
                }
                Ok(analyzed_count)
            }

            Statement::Select { .. }
            | Statement::ExternalScan { .. }
            | Statement::VectorSearch { .. }
            | Statement::Explain { .. }
            | Statement::GraphTraverse { .. }
            | Statement::GraphShortestPath { .. }
            | Statement::GraphMatch { .. }
            | Statement::GraphAlgorithm { .. }
            | Statement::WithCte { .. }
            | Statement::Compound { .. } => Err(Error::SqlSyntax(
                "Query statement passed to execute(); use query() instead".into(),
            )),
            Statement::CopyFrom {
                table,
                source,
                format,
                options,
            } => self.execute_copy_from(pager, &table, &source, format.as_deref(), &options),
            Statement::CopyTo {
                source_table,
                source_query,
                dest,
                format,
                partition_by,
            } => self.execute_copy_to(
                pager,
                source_table.as_deref(),
                source_query.as_deref(),
                &dest,
                format.as_deref(),
                partition_by.as_deref(),
            ),
        }
    }

    pub(crate) fn compute_generated_columns(
        &self,
        table: &str,
        table_cols: &[ColumnDef],
        values: &mut [Value],
    ) -> Result<()> {
        if !table_cols.iter().any(|c| c.generated_expr.is_some()) {
            return Ok(());
        }
        let col_names: Vec<String> = table_cols.iter().map(|c| c.name.clone()).collect();
        for (col_idx, col_def) in table_cols.iter().enumerate() {
            let Some(ref gen_expr) = col_def.generated_expr else {
                continue;
            };
            let snapshot_row = Row::new(col_names.clone(), values.to_vec());
            let computed =
                eval_projected_expr(&snapshot_row, gen_expr, gen_expr, Some(&self.udf_registry))?;
            let coerced = col_def
                .data_type
                .coerce_value(computed)
                .map_err(|e| match e {
                    Error::DimensionMismatch(exp, got) => Error::DimensionMismatch(exp, got),
                    Error::ConstraintViolation(msg) => {
                        Error::ConstraintViolation(format!("{table}.{}: {msg}", col_def.name))
                    }
                    other => other,
                })?;
            if col_idx < values.len() {
                values[col_idx] = coerced;
            }
        }
        Ok(())
    }
}

fn validate_index_targets(
    table_def: &TableDef,
    target_cols: &[String],
    table: &str,
    name: &str,
) -> Result<()> {
    use crate::sql::lexer::Token;

    for col in target_cols {
        let base_col = col.split('.').next().unwrap_or(col);
        if resolve_table_col_index(table_def, col).is_some()
            || table_def.column_index(base_col).is_some()
        {
            continue;
        }
        let Ok(toks) = tokenize(col) else {
            return Err(Error::Corrupted(format!(
                "Invalid index target '{col}' on table '{table}' for index '{name}'"
            )));
        };
        if toks.is_empty() {
            return Err(Error::Corrupted(format!(
                "Empty index expression on table '{table}' for index '{name}'"
            )));
        }
        for i in 0..toks.len() {
            let Token::Ident(ref id) = toks[i] else {
                continue;
            };
            if i > 0 && toks[i - 1] == Token::Dot {
                continue;
            }
            if toks.get(i + 1) == Some(&Token::OpenParen) {
                continue;
            }
            if toks.get(i + 1) == Some(&Token::Dot) && id.eq_ignore_ascii_case(&table_def.name) {
                continue;
            }
            if id.eq_ignore_ascii_case("TRUE") || id.eq_ignore_ascii_case("FALSE") {
                continue;
            }
            if table_def.column_index(id).is_none() {
                return Err(Error::Corrupted(format!(
                    "Column '{id}' not found in table '{table}' for index '{name}'"
                )));
            }
        }
    }
    Ok(())
}
