#[allow(unused_imports)]
use super::*;

impl SQLExecutor {
    /// Execute a query returning rows (SELECT, VECTOR NEAR, EXPLAIN, RETURNING, etc.)
    pub fn query(&mut self, pager: &mut Pager, stmt: Statement) -> Result<Vec<Row>> {
        if Self::is_mutation(&stmt) {
            self.atomic_statement(pager, |executor, pager| executor.query_inner(pager, stmt))
        } else {
            self.query_inner(pager, stmt)
        }
    }

    pub(super) fn query_inner(&mut self, pager: &mut Pager, stmt: Statement) -> Result<Vec<Row>> {
        match stmt {
            Statement::InsertMany {
                table,
                columns,
                rows,
                conflict_action,
                returning,
            } => {
                let mut returned = Vec::new();
                for values in rows {
                    let (_, result) = self.execute_insert(
                        pager,
                        table.clone(),
                        columns.clone(),
                        values,
                        conflict_action.clone(),
                        returning.clone(),
                    )?;
                    returned.extend(result);
                }
                Ok(returned)
            }
            Statement::Insert {
                table,
                columns,
                values,
                conflict_action,
                returning,
            } => {
                let (_, rows) =
                    self.execute_insert(pager, table, columns, values, conflict_action, returning)?;
                Ok(rows)
            }
            Statement::Update {
                table,
                assignments,
                where_clause,
                returning,
            } => {
                let (_, rows) =
                    self.execute_update(pager, table, assignments, where_clause, returning)?;
                Ok(rows)
            }
            Statement::Delete {
                table,
                where_clause,
                returning,
            } => {
                let (_, rows) = self.execute_delete(pager, table, where_clause, returning)?;
                Ok(rows)
            }
            Statement::WithCte { ctes, main_query } => {
                self.query_with_cte(pager, ctes, *main_query)
            }
            Statement::Compound { left, op, right } => {
                let mut left_rows = self.query(pager, *left)?;
                let right_rows = self.query(pager, *right)?;
                combine_compound_rows(&mut left_rows, right_rows, op);
                Ok(left_rows)
            }
            Statement::ExternalScan { func, path, arg } => {
                #[cfg(feature = "std")]
                {
                    crate::lakehouse::scan_external_rows(&func, &path, arg.as_deref(), None)
                }
                #[cfg(not(feature = "std"))]
                {
                    let _ = (func, path, arg);
                    Err(Error::SqlSyntax(
                        "External file scans require the 'std' feature".into(),
                    ))
                }
            }
            Statement::CopyFrom {
                table,
                source,
                format,
                options,
            } => {
                let count =
                    self.execute_copy_from(pager, &table, &source, format.as_deref(), &options)?;
                Ok(vec![Row::new(
                    vec!["rows_copied".to_string()],
                    vec![Value::Integer(count as i64)],
                )])
            }
            Statement::CopyTo {
                source_table,
                source_query,
                dest,
                format,
                partition_by,
            } => {
                let count = self.execute_copy_to(
                    pager,
                    source_table.as_deref(),
                    source_query.as_deref(),
                    &dest,
                    format.as_deref(),
                    partition_by.as_deref(),
                )?;
                Ok(vec![Row::new(
                    vec!["rows_exported".to_string()],
                    vec![Value::Integer(count as i64)],
                )])
            }
            Statement::Explain {
                statement,
                query_plan,
            } => self.explain_query_plan(pager, *statement, query_plan),

            Statement::Select {
                distinct,
                columns,
                table,
                from_subquery,
                join,
                extra_joins,
                where_clause,
                group_by,
                having,
                order_by,
                limit,
                offset,
                as_of_timestamp,
            } => {
                if table.is_empty() {
                    let empty_row = Row::new(Vec::new(), Vec::new());
                    if has_projection_subquery(&columns) {
                        let plan = self.prepare_projection_plan(pager, &columns, &[])?;
                        let mut memo = CorrelatedSubqueryMemo::default();
                        return Ok(vec![self.project_row_with_plan(
                            pager,
                            &empty_row,
                            &plan,
                            &[],
                            &mut memo,
                        )?]);
                    }
                    return Ok(vec![self.project_row(&empty_row, &columns)?]);
                }
                if let Some((sub_stmt, alias)) = from_subquery {
                    #[cfg(all(feature = "lakehouse", feature = "olap"))]
                    if join.is_none()
                        && extra_joins.is_empty()
                        && !distinct
                        && as_of_timestamp.is_none()
                        && let Statement::ExternalScan {
                            ref func,
                            ref path,
                            arg: None,
                        } = *sub_stmt
                        && matches!(func.as_str(), "read_parquet" | "parquet")
                    {
                        let resolved_where = if let Some(ref expr) = where_clause {
                            Some(self.resolve_where_expr(pager, expr)?)
                        } else {
                            None
                        };
                        let resolved_having = if let Some(ref expr) = having {
                            Some(self.resolve_where_expr(pager, expr)?)
                        } else {
                            None
                        };
                        if let Some((rows, _olap_stats)) =
                            crate::lakehouse::olap::execute_parquet_sql_select(
                                path,
                                &columns,
                                resolved_where.as_ref(),
                                group_by.as_deref(),
                                resolved_having.as_ref(),
                                order_by.as_slice(),
                                limit,
                                offset,
                            )?
                        {
                            return Ok(rows);
                        }
                    }

                    let mut cte_store = HashMap::new();
                    let sub_rows = self.query(pager, *sub_stmt)?;
                    cte_store.insert(alias.clone(), sub_rows);
                    let outer_stmt = Statement::Select {
                        distinct,
                        columns,
                        table: alias,
                        from_subquery: None,
                        join,
                        extra_joins,
                        where_clause,
                        group_by,
                        having,
                        order_by,
                        limit,
                        offset,
                        as_of_timestamp,
                    };
                    return self.query_on_ephemeral_rows(pager, outer_stmt, &cte_store);
                }

                // If target table is a persistent View, evaluate view query and apply outer filters/order/limit
                if let Some(view_def) = self.catalog.get_view(&table).cloned() {
                    let mut view_stmt = crate::sql::parse_sql(&view_def.query_sql)?;
                    if let Statement::Select {
                        as_of_timestamp: ref mut view_ts,
                        ..
                    } = view_stmt
                    {
                        *view_ts = as_of_timestamp;
                    }
                    let mut rows = self.query(pager, view_stmt)?;

                    // Check outer WHERE filter
                    if let Some(ref expr) = where_clause {
                        let resolved_where = self.resolve_where_expr(pager, expr)?;
                        rows.retain(|r| row_matches_resolved(r, &resolved_where));
                    }

                    // Determine output column names before ORDER BY so scalar-subquery aliases can sort cleanly
                    let output_col_names = if columns.is_empty() {
                        if let Some(first) = rows.first() {
                            first.columns().to_vec()
                        } else {
                            Vec::new()
                        }
                    } else {
                        columns.clone()
                    };
                    let has_proj_sub = has_projection_subquery(&output_col_names);
                    let proj_plan = if has_proj_sub {
                        Some(self.prepare_projection_plan(pager, &output_col_names, &[])?)
                    } else {
                        None
                    };
                    let mut proj_memo = CorrelatedSubqueryMemo::default();

                    // Apply outer ORDER BY
                    if !order_by.is_empty() {
                        if let Some(ref plan) = proj_plan {
                            self.enrich_rows_with_order_plan(
                                pager,
                                &mut rows,
                                plan,
                                &order_by,
                                &[],
                                &mut proj_memo,
                            )?;
                        } else {
                            enrich_rows_with_order_aliases(self, &mut rows, &columns, &order_by)?;
                        }
                        rows.sort_by(|a, b| compare_rows_by_order(a, b, &order_by));
                    }

                    // Apply outer column projection and LIMIT
                    let mut result_rows = Vec::new();
                    let skip_count = offset.unwrap_or(0);
                    let mut skipped = 0;
                    let mut distinct_set = RowDistinctSet::new();
                    for row in rows {
                        let projected = if let Some(ref plan) = proj_plan {
                            self.project_row_with_plan(pager, &row, plan, &[], &mut proj_memo)?
                        } else {
                            self.project_row(&row, &output_col_names)?
                        };
                        if distinct {
                            if distinct_set.push_paginated(
                                &mut result_rows,
                                projected,
                                skip_count,
                                limit,
                            ) {
                                break;
                            }
                            continue;
                        }
                        if skipped < skip_count {
                            skipped += 1;
                            continue;
                        }
                        result_rows.push(projected);
                        if let Some(lim) = limit
                            && result_rows.len() >= lim
                        {
                            break;
                        }
                    }

                    return Ok(result_rows);
                }

                if !self.active_ctes.is_empty()
                    && (self.active_ctes.contains_key(&table.to_ascii_lowercase())
                        || join
                            .iter()
                            .chain(extra_joins.iter())
                            .any(|j| self.active_ctes.contains_key(&j.table.to_ascii_lowercase())))
                {
                    let cte_store = self.active_ctes.clone();
                    return self.query_on_ephemeral_rows(
                        pager,
                        Statement::Select {
                            distinct,
                            columns,
                            table,
                            from_subquery: None,
                            join,
                            extra_joins,
                            where_clause,
                            group_by,
                            having,
                            order_by,
                            limit,
                            offset,
                            as_of_timestamp,
                        },
                        &cte_store,
                    );
                }

                let table_def = self
                    .catalog
                    .get_table(&table)
                    .cloned()
                    .ok_or_else(|| Error::TableNotFound(table.clone()))?;

                let root_page = table_def.root_page;
                let all_col_names = table_def.column_names();

                let fts_query = if table_def.fts_config.is_some() {
                    where_clause
                        .as_ref()
                        .and_then(|e| extract_fts_match_query(e, &table))
                } else {
                    None
                };
                let fts_scores: Option<HashMap<u64, f64>> = if let Some(ref q) = fts_query {
                    self.ensure_fts_index(pager, &table_def)?;
                    let idx_key = table.to_lowercase();
                    let mut score_map = HashMap::new();
                    if let Some(idx) = self.fts_indexes.get(&idx_key) {
                        let total_docs = idx.total_docs().max(1);
                        for (doc_id, score) in idx.search(q, total_docs) {
                            score_map.insert(doc_id, -(score as f64));
                        }
                    }
                    Some(score_map)
                } else {
                    None
                };

                let all_joins: Vec<crate::sql::parser::JoinClause> =
                    join.into_iter().chain(extra_joins).collect();
                let has_joins = !all_joins.is_empty();

                let resolved_where = if let Some(ref expr) = where_clause {
                    let mut outer_defs = Vec::with_capacity(1 + all_joins.len());
                    outer_defs.push(table_def.clone());
                    for jc in &all_joins {
                        if let Some(rt) = self.catalog.get_table(&jc.table) {
                            outer_defs.push(rt.clone());
                        }
                    }
                    let outer_refs: Vec<&TableDef> = outer_defs.iter().collect();
                    Some(self.resolve_where_expr_for_tables(pager, expr, &outer_refs)?)
                } else {
                    None
                };

                let has_correlated = resolved_where
                    .as_ref()
                    .is_some_and(resolved_where_has_correlated);
                let (single_scan_where, single_post_correlated) = match resolved_where.as_ref() {
                    Some(r) if has_correlated && !has_joins => split_correlated_and_predicates(r),
                    other => (other.cloned(), None),
                };

                let resolved_having = if let Some(ref h_expr) = having {
                    Some(self.resolve_where_expr(pager, h_expr)?)
                } else {
                    None
                };

                let has_win = columns.iter().any(|c| has_window_function(c));
                let has_agg = is_aggregate_query(&columns);
                let has_proj_sub = has_projection_subquery(&columns);
                let wants_fts_scores = fts_scores.is_some()
                    && (columns.iter().any(|c| {
                        let cl = c.to_ascii_lowercase();
                        cl == "rank" || cl.starts_with("bm25(")
                    }) || order_by.iter().any(|o| {
                        let ol = o.column.to_ascii_lowercase();
                        ol == "rank" || ol.starts_with("bm25(")
                    }));

                // Universal Single-Table Projection Mask (enables projected decode & zero-alloc WHERE)
                let single_table_mask =
                    if !has_joins && group_by.is_none() && as_of_timestamp.is_none() && !has_win {
                        compute_single_table_needed_mask(
                            &table_def,
                            &columns,
                            resolved_where.as_ref(),
                            &order_by,
                            wants_fts_scores,
                        )
                    } else {
                        None
                    };

                // Fast Point Lookup Optimization if WHERE matches single Primary Key column equals
                let pk_idx = table_def.primary_key_index();
                let is_int_pk = pk_idx.is_some_and(|idx| {
                    table_def.columns[idx].data_type == crate::sql::catalog::DataType::Integer
                });
                let pk_range = if is_int_pk
                    && as_of_timestamp.is_none()
                    && let Some(pk_col_idx) = pk_idx
                    && let Some(ref w_expr) = where_clause
                {
                    let pk_name = &table_def.columns[pk_col_idx].name;
                    let (mn, mx, has) = extract_pk_signed_range(w_expr, pk_name);
                    if has { Some((mn, mx)) } else { None }
                } else {
                    None
                };

                let mut left_rows_from_index: Option<Vec<Row>> = None;

                if as_of_timestamp.is_none()
                    && !has_joins
                    && let Some((Some(lo), Some(hi))) = pk_range
                {
                    if lo > hi && group_by.is_none() && !has_agg {
                        return Ok(Vec::new());
                    }
                    if lo == hi
                        && group_by.is_none()
                        && !has_agg
                        && !has_win
                        && fts_scores.is_none()
                    {
                        if offset.unwrap_or(0) > 0 || limit == Some(0) {
                            return Ok(Vec::new());
                        }
                        let all_col_shared = Arc::new(all_col_names.clone());
                        if single_post_correlated.is_none() {
                            let output_col_names = if columns.is_empty() {
                                all_col_names.clone()
                            } else {
                                columns.clone()
                            };
                            return self.fetch_rows_by_row_ids(
                                pager,
                                root_page,
                                &table_def,
                                &all_col_shared,
                                vec![lo as u64],
                                single_scan_where.as_ref(),
                                single_table_mask.as_deref(),
                                IndexFetchMode::EarlyReturn {
                                    output_col_names,
                                    distinct,
                                    offset,
                                    limit,
                                },
                            );
                        } else {
                            left_rows_from_index = Some(self.fetch_rows_by_row_ids(
                                pager,
                                root_page,
                                &table_def,
                                &all_col_shared,
                                vec![lo as u64],
                                single_scan_where.as_ref(),
                                single_table_mask.as_deref(),
                                IndexFetchMode::CollectAll,
                            )?);
                        }
                    }
                }

                // Composite secondary index lookup for `WHERE a = X AND b = Y`
                if left_rows_from_index.is_none()
                    && as_of_timestamp.is_none()
                    && !has_joins
                    && order_by.is_empty()
                    && group_by.is_none()
                    && !has_agg
                    && !has_win
                    && fts_scores.is_none()
                    && let Some(ref expr) = where_clause
                {
                    let mut and_eqs = HashMap::new();
                    collect_and_eq_conditions(expr, &mut and_eqs);
                    if and_eqs.len() >= 2 {
                        let matched_comp_idx = self
                            .catalog
                            .indexes()
                            .into_iter()
                            .filter(|idx| {
                                idx.table.eq_ignore_ascii_case(&table)
                                    && partial_index_matches_query(idx, where_clause.as_ref())
                            })
                            .filter_map(|idx| {
                                let cols = idx.target_columns();
                                if cols.len() >= 2
                                    && cols.iter().all(|c| and_eqs.contains_key(&c.to_lowercase()))
                                {
                                    Some((cols.len(), idx))
                                } else {
                                    None
                                }
                            })
                            .max_by_key(|(len, _)| *len)
                            .map(|(_, idx)| idx);

                        if let Some(index_def) = matched_comp_idx {
                            let tcols = index_def.target_columns();
                            let dummy_cols: Vec<String> =
                                tcols.iter().map(|c| (*c).to_string()).collect();
                            let dummy_vals: Vec<Value> = tcols
                                .iter()
                                .map(|c| {
                                    and_eqs
                                        .get(&c.to_lowercase())
                                        .cloned()
                                        .unwrap_or(Value::Null)
                                })
                                .collect();
                            let dummy_row = Row::new(dummy_cols, dummy_vals);
                            if let Some((comp_val, _)) = extract_index_key_value(&dummy_row, &tcols)
                            {
                                let key = value_to_index_key(&comp_val);
                                let mut matched_row_ids = Vec::new();
                                if let Some(bucket) =
                                    IndexBucket::load(&self.btree, pager, index_def.root_page, key)?
                                    && let Some(rids) = bucket.get_row_ids(&comp_val)
                                {
                                    matched_row_ids.extend_from_slice(rids);
                                }
                                let all_col_shared = Arc::new(all_col_names.clone());
                                if single_post_correlated.is_none() {
                                    let output_col_names = if columns.is_empty() {
                                        all_col_names.clone()
                                    } else {
                                        columns.clone()
                                    };
                                    return self.fetch_rows_by_row_ids(
                                        pager,
                                        root_page,
                                        &table_def,
                                        &all_col_shared,
                                        matched_row_ids,
                                        single_scan_where.as_ref(),
                                        single_table_mask.as_deref(),
                                        IndexFetchMode::EarlyReturn {
                                            output_col_names,
                                            distinct,
                                            offset,
                                            limit,
                                        },
                                    );
                                } else {
                                    left_rows_from_index = Some(self.fetch_rows_by_row_ids(
                                        pager,
                                        root_page,
                                        &table_def,
                                        &all_col_shared,
                                        matched_row_ids,
                                        single_scan_where.as_ref(),
                                        single_table_mask.as_deref(),
                                        IndexFetchMode::CollectAll,
                                    )?);
                                }
                            }
                        }
                    }
                }

                // Cost-Based Optimizer (CBO) evaluation for Secondary Index / Multi-Key IN / OR Union vs Sequential Scan
                if left_rows_from_index.is_none()
                    && as_of_timestamp.is_none()
                    && !has_joins
                    && fts_scores.is_none()
                    && let Some(ref w_expr) = where_clause
                {
                    let optimizer = crate::sql::planner::CostOptimizer::new();
                    let default_stats = crate::sql::planner::TableStats::new(&table, 100, 10);
                    let stats = self
                        .catalog
                        .get_table_stats(&table)
                        .unwrap_or(&default_stats);
                    let can_stream_early_return = order_by.is_empty()
                        && group_by.is_none()
                        && !has_agg
                        && !has_win
                        && !has_proj_sub
                        && single_post_correlated.is_none();
                    let all_col_shared = Arc::new(all_col_names.clone());

                    let mut candidate_index_rids: Option<Vec<u64>> = None;

                    let mut eq_conds = HashMap::new();
                    collect_and_eq_conditions(w_expr, &mut eq_conds);
                    let chosen_single = eq_conds.iter().find_map(|(col_key, val)| {
                        self.find_usable_index_for_column(&table, col_key, Some(w_expr))
                            .map(|idx| (col_key.clone(), val.clone(), idx))
                    });
                    if let Some((cond_col, cond_val, index_def)) = chosen_single {
                        let sel = stats.estimate_selectivity(&cond_col, true);
                        let index_cost =
                            optimizer.estimate_index_scan(stats, &index_def.name, &cond_col, sel);
                        let seq_cost = optimizer.estimate_seq_scan(stats, sel);

                        if index_cost.total_cost <= seq_cost.total_cost {
                            let key = value_to_index_key(&cond_val);
                            let mut matched_row_ids = Vec::new();
                            if let Some(bucket) =
                                IndexBucket::load(&self.btree, pager, index_def.root_page, key)?
                                && let Some(rids) = bucket.get_row_ids(&cond_val)
                            {
                                matched_row_ids.extend_from_slice(rids);
                            }
                            candidate_index_rids = Some(matched_row_ids);
                        }
                    }

                    // Multi-key PK / Secondary Index `IN (...)` & `IN (SELECT ...)` pushdown
                    if candidate_index_rids.is_none()
                        && !has_agg
                        && group_by.is_none()
                        && !has_win
                        && let Some(ref r_expr) = single_scan_where
                    {
                        let mut in_sets = Vec::new();
                        extract_top_level_in_sets(r_expr, &mut in_sets);
                        let pk_name = pk_idx.map(|i| table_def.columns[i].name.as_str());

                        let mut matched_in_row_ids: Option<Vec<u64>> = None;
                        if is_int_pk
                            && let Some(pk_col) = pk_name
                            && let Some((_, lookup)) = in_sets.iter().find(|(col, _)| {
                                resolve_direct_table_col(&table_def, col) == pk_idx
                                    || col.eq_ignore_ascii_case(pk_col)
                            })
                        {
                            let keys = lookup.collect_sorted_int_keys();
                            if lookup.distinct_len() == 0 {
                                matched_in_row_ids = Some(Vec::new());
                            } else if !keys.is_empty()
                                && keys.len() <= 512
                                && (keys.len() <= 64 || stats.total_rows > keys.len() * 4)
                            {
                                matched_in_row_ids =
                                    Some(keys.into_iter().map(|k| k as u64).collect());
                            }
                        }

                        if matched_in_row_ids.is_none() {
                            for (col, lookup) in in_sets {
                                let n_keys = lookup.distinct_len();
                                if n_keys == 0 {
                                    matched_in_row_ids = Some(Vec::new());
                                    break;
                                }
                                if n_keys > 128 {
                                    continue;
                                }
                                let bare_col = col.split_once('.').map_or(col, |(_, c)| c);
                                if let Some(index_def) = self.find_usable_index_for_column(
                                    &table,
                                    bare_col,
                                    Some(w_expr),
                                ) {
                                    let sel = stats.estimate_selectivity(bare_col, true);
                                    let one_cost = optimizer
                                        .estimate_index_scan(stats, &index_def.name, bare_col, sel)
                                        .total_cost;
                                    let seq_cost =
                                        optimizer.estimate_seq_scan(stats, sel).total_cost;
                                    if n_keys <= 16 || (n_keys as f64) * one_cost <= seq_cost {
                                        let mut rids = Vec::new();
                                        for val in lookup.collect_probe_values() {
                                            let key = value_to_index_key(&val);
                                            if let Some(bucket) = IndexBucket::load(
                                                &self.btree,
                                                pager,
                                                index_def.root_page,
                                                key,
                                            )? && let Some(list) = bucket.get_row_ids(&val)
                                            {
                                                rids.extend_from_slice(list);
                                            }
                                        }
                                        matched_in_row_ids = Some(rids);
                                        break;
                                    }
                                }
                            }
                        }

                        candidate_index_rids = matched_in_row_ids;
                    }

                    // OR Index Union (`WHERE id = 1 OR id = 2` or indexed branches)
                    if candidate_index_rids.is_none()
                        && !has_agg
                        && group_by.is_none()
                        && !has_win
                        && matches!(w_expr, WhereExpr::Or(_, _))
                        && let Some(or_rids) = self.try_collect_or_union_row_ids(
                            pager, &table, &table_def, w_expr, stats,
                        )?
                    {
                        candidate_index_rids = Some(or_rids);
                    }

                    if let Some(matched_row_ids) = candidate_index_rids {
                        if can_stream_early_return {
                            let output_col_names = if columns.is_empty() {
                                all_col_names.clone()
                            } else {
                                columns.clone()
                            };
                            return self.fetch_rows_by_row_ids(
                                pager,
                                root_page,
                                &table_def,
                                &all_col_shared,
                                matched_row_ids,
                                single_scan_where.as_ref(),
                                single_table_mask.as_deref(),
                                IndexFetchMode::EarlyReturn {
                                    output_col_names,
                                    distinct,
                                    offset,
                                    limit,
                                },
                            );
                        } else {
                            left_rows_from_index = Some(self.fetch_rows_by_row_ids(
                                pager,
                                root_page,
                                &table_def,
                                &all_col_shared,
                                matched_row_ids,
                                single_scan_where.as_ref(),
                                single_table_mask.as_deref(),
                                IndexFetchMode::CollectAll,
                            )?);
                        }
                    }
                }

                // Single-Pass Pushdown Aggregation Optimization using VectorizedAccumulator
                if left_rows_from_index.is_none()
                    && !has_joins
                    && !has_correlated
                    && order_by.is_empty()
                    && group_by.is_none()
                    && as_of_timestamp.is_none()
                    && !distinct
                    && has_agg
                    && !has_proj_sub
                {
                    struct PushdownAgg {
                        spec: ParsedAggSpec,
                        col_idx: Option<usize>,
                        acc: crate::sql::vectorized::VectorizedAccumulator,
                    }

                    let mut aggs: Vec<PushdownAgg> = Vec::new();
                    for col in &columns {
                        if let Some(spec) = parse_aggregate_spec(col) {
                            let col_idx = if spec.is_star {
                                None
                            } else {
                                resolve_table_col_index(&table_def, &spec.arg)
                            };
                            let acc = crate::sql::vectorized::VectorizedAccumulator::with_tracking(
                                spec.needs_samples(),
                                spec.needs_distinct(),
                            );
                            aggs.push(PushdownAgg { spec, col_idx, acc });
                        } else {
                            let output_name = parse_col_and_alias(col).1.unwrap_or(col).to_string();
                            aggs.push(PushdownAgg {
                                spec: ParsedAggSpec {
                                    output_name,
                                    func: String::new(),
                                    arg: String::new(),
                                    percentile_q: None,
                                    is_star: false,
                                    is_distinct: false,
                                },
                                col_idx: resolve_table_col_index(&table_def, col),
                                acc: crate::sql::vectorized::VectorizedAccumulator::new(),
                            });
                        }
                    }

                    let mut needed_mask = vec![false; all_col_names.len()];
                    for agg in &aggs {
                        if let Some(idx) = agg.col_idx
                            && idx < needed_mask.len()
                        {
                            needed_mask[idx] = true;
                        }
                    }
                    let can_project_where = resolved_where.as_ref().is_none_or(|r| {
                        mark_resolved_where_columns(r, &table_def, &mut needed_mask)
                    });

                    let mut row_vals = Vec::with_capacity(all_col_names.len());
                    let left_col_shared = Arc::new(all_col_names.clone());
                    let agg_callback = |payload: &[u8]| -> Result<bool> {
                        let full_row;
                        let vals: &[Value] = if can_project_where {
                            decode_row_values_into_projected(payload, &needed_mask, &mut row_vals)?;
                            if let Some(ref r_expr) = resolved_where
                                && !slice_matches_resolved(&row_vals, &table_def, r_expr)
                            {
                                return Ok(true);
                            }
                            &row_vals
                        } else {
                            full_row = decode_row_shared(payload, &left_col_shared)?;
                            if let Some(ref r_expr) = resolved_where
                                && !row_matches_resolved(&full_row, r_expr)
                            {
                                return Ok(true);
                            }
                            full_row.values()
                        };
                        for agg in &mut aggs {
                            if agg.spec.is_star {
                                agg.acc.count_all += 1;
                                agg.acc.count += 1;
                            } else if let Some(idx) = agg.col_idx
                                && let Some(val) = vals.get(idx)
                            {
                                agg.acc.accumulate_value(val);
                            }
                        }
                        Ok(true)
                    };
                    if let Some((pk_min, pk_max)) = pk_range {
                        let mut cb = agg_callback;
                        self.btree.scan_signed_i64_with_ref(
                            pager,
                            root_page,
                            pk_min,
                            pk_max,
                            false,
                            |_rid, payload| cb(payload),
                        )?;
                    } else {
                        let mut cb = agg_callback;
                        self.btree
                            .scan_with_ref(pager, root_page, |_rid, payload| cb(payload))?;
                    }

                    let mut out_cols = Vec::with_capacity(aggs.len());
                    let mut out_vals = Vec::with_capacity(aggs.len());
                    for mut agg in aggs {
                        let out_name = agg.spec.output_name.clone();
                        let val = agg.spec.finalize(&mut agg.acc);
                        out_cols.push(out_name);
                        out_vals.push(val);
                    }
                    return Ok(vec![Row::new(out_cols, out_vals)]);
                }

                // Streaming Vectorized Hash Aggregation Pushdown for GROUP BY
                if let Some(ref gb_cols) = group_by
                    && !has_joins
                    && as_of_timestamp.is_none()
                    && fts_scores.is_none()
                    && !has_win
                {
                    let resolved_having = if let Some(ref h) = having {
                        Some(self.resolve_where_expr(pager, h)?)
                    } else {
                        None
                    };

                    let output_cols = if columns.is_empty() {
                        gb_cols.clone()
                    } else {
                        columns.clone()
                    };

                    enum GroupColPlan {
                        GroupKey {
                            out_name: String,
                            key_pos: usize,
                        },
                        Aggregate {
                            spec: ParsedAggSpec,
                        },
                        FirstValue {
                            out_name: String,
                            col_idx: Option<usize>,
                        },
                    }

                    let gb_indices: Vec<Option<usize>> = gb_cols
                        .iter()
                        .map(|c| resolve_table_col_index(&table_def, c))
                        .collect();
                    let mut plans = Vec::with_capacity(output_cols.len());
                    let mut agg_specs_template: Vec<(Option<usize>, bool, bool, bool)> = Vec::new();
                    let mut needed_mask = vec![false; all_col_names.len()];

                    for idx_opt in &gb_indices {
                        if let Some(i) = *idx_opt
                            && i < needed_mask.len()
                        {
                            needed_mask[i] = true;
                        }
                    }

                    for col in &output_cols {
                        if let Some(spec) = parse_aggregate_spec(col) {
                            let col_idx = if spec.is_star {
                                None
                            } else {
                                resolve_table_col_index(&table_def, &spec.arg)
                            };
                            if let Some(i) = col_idx
                                && i < needed_mask.len()
                            {
                                needed_mask[i] = true;
                            }
                            agg_specs_template.push((
                                col_idx,
                                spec.is_star,
                                spec.needs_samples(),
                                spec.needs_distinct(),
                            ));
                            plans.push(GroupColPlan::Aggregate { spec });
                        } else {
                            let (raw_expr, alias_opt) = parse_col_and_alias(col);
                            let out_name = alias_opt.unwrap_or(raw_expr).to_string();
                            if let Some(pos) = gb_cols
                                .iter()
                                .position(|g| g.eq_ignore_ascii_case(raw_expr))
                            {
                                plans.push(GroupColPlan::GroupKey {
                                    out_name,
                                    key_pos: pos,
                                });
                            } else {
                                let col_idx = resolve_table_col_index(&table_def, raw_expr);
                                if let Some(i) = col_idx
                                    && i < needed_mask.len()
                                {
                                    needed_mask[i] = true;
                                }
                                plans.push(GroupColPlan::FirstValue { out_name, col_idx });
                            }
                        }
                    }

                    struct StreamGroupState {
                        keys: Vec<Value>,
                        accs: Vec<crate::sql::vectorized::VectorizedAccumulator>,
                        first_vals: Vec<(usize, Value)>,
                    }

                    let can_project_where = resolved_where.as_ref().is_none_or(|r| {
                        mark_resolved_where_columns(r, &table_def, &mut needed_mask)
                    });
                    let left_col_shared = Arc::new(all_col_names.clone());
                    let mut group_map: HashMap<Vec<HashableValue>, usize> = HashMap::new();
                    let mut groups: Vec<StreamGroupState> = Vec::new();
                    let mut row_vals = Vec::with_capacity(all_col_names.len());
                    let mut hash_key_buf: Vec<HashableValue> = Vec::with_capacity(gb_cols.len());

                    let mut group_scan_cb = |_row_id: u64, payload: &[u8]| -> Result<bool> {
                        let full_row;
                        let vals: &[Value] = if can_project_where {
                            decode_row_values_into_projected(payload, &needed_mask, &mut row_vals)?;
                            if let Some(ref r_expr) = resolved_where
                                && !slice_matches_resolved(&row_vals, &table_def, r_expr)
                            {
                                return Ok(true);
                            }
                            &row_vals
                        } else {
                            full_row = decode_row_shared(payload, &left_col_shared)?;
                            if let Some(ref r_expr) = resolved_where
                                && !row_matches_resolved(&full_row, r_expr)
                            {
                                return Ok(true);
                            }
                            full_row.values()
                        };

                        hash_key_buf.clear();
                        for idx_opt in &gb_indices {
                            let v = idx_opt.and_then(|i| vals.get(i)).unwrap_or(&Value::Null);
                            hash_key_buf.push(HashableValue::from(v));
                        }

                        let g_pos = if let Some(&pos) = group_map.get(&hash_key_buf) {
                            pos
                        } else {
                            let pos = groups.len();
                            let key_vals: Vec<Value> = gb_indices
                                .iter()
                                .map(|idx_opt| {
                                    idx_opt
                                        .and_then(|i| vals.get(i).cloned())
                                        .unwrap_or(Value::Null)
                                })
                                .collect();
                            let accs = agg_specs_template
                                .iter()
                                .map(|&(_, _, s, d)| {
                                    crate::sql::vectorized::VectorizedAccumulator::with_tracking(
                                        s, d,
                                    )
                                })
                                .collect();
                            let mut fv = Vec::new();
                            for p in &plans {
                                if let GroupColPlan::FirstValue {
                                    col_idx: Some(ci), ..
                                } = p
                                {
                                    fv.push((*ci, vals.get(*ci).cloned().unwrap_or(Value::Null)));
                                }
                            }
                            group_map.insert(hash_key_buf.clone(), pos);
                            groups.push(StreamGroupState {
                                keys: key_vals,
                                accs,
                                first_vals: fv,
                            });
                            pos
                        };

                        let state = &mut groups[g_pos];
                        for (acc_idx, &(col_idx, is_star, _, _)) in
                            agg_specs_template.iter().enumerate()
                        {
                            if is_star {
                                state.accs[acc_idx].count_all += 1;
                                state.accs[acc_idx].count += 1;
                            } else if let Some(ci) = col_idx
                                && let Some(v) = vals.get(ci)
                            {
                                state.accs[acc_idx].accumulate_value(v);
                            }
                        }
                        Ok(true)
                    };

                    if let Some((pk_min, pk_max)) = pk_range {
                        self.btree.scan_signed_i64_with_ref(
                            pager,
                            root_page,
                            pk_min,
                            pk_max,
                            false,
                            |rid, payload| group_scan_cb(rid as u64, payload),
                        )?;
                    } else {
                        self.btree.scan_with_ref(pager, root_page, |rid, payload| {
                            group_scan_cb(rid, payload)
                        })?;
                    }

                    let mut result_rows = Vec::with_capacity(groups.len());
                    for mut state in groups {
                        let mut g_cols = Vec::with_capacity(plans.len());
                        let mut g_vals = Vec::with_capacity(plans.len());
                        let mut acc_cursor = 0usize;
                        for plan in &plans {
                            match plan {
                                GroupColPlan::GroupKey { out_name, key_pos } => {
                                    g_cols.push(out_name.clone());
                                    g_vals.push(state.keys[*key_pos].clone());
                                }
                                GroupColPlan::Aggregate { spec } => {
                                    g_cols.push(spec.output_name.clone());
                                    g_vals.push(spec.finalize(&mut state.accs[acc_cursor]));
                                    acc_cursor += 1;
                                }
                                GroupColPlan::FirstValue { out_name, col_idx } => {
                                    g_cols.push(out_name.clone());
                                    let v = col_idx
                                        .and_then(|ci| {
                                            state
                                                .first_vals
                                                .iter()
                                                .find(|(i, _)| *i == ci)
                                                .map(|(_, val)| val.clone())
                                        })
                                        .unwrap_or(Value::Null);
                                    g_vals.push(v);
                                }
                            }
                        }
                        let row = Row::new(g_cols, g_vals);
                        if let Some(ref h_expr) = resolved_having
                            && !row_matches_resolved(&row, h_expr)
                        {
                            continue;
                        }
                        result_rows.push(row);
                    }

                    if !order_by.is_empty() {
                        result_rows.sort_by(|a, b| compare_rows_by_order(a, b, &order_by));
                    }
                    if distinct {
                        let mut deduped = Vec::with_capacity(result_rows.len());
                        let mut distinct_set = RowDistinctSet::new();
                        for r in result_rows {
                            distinct_set.push_paginated(&mut deduped, r, 0, None);
                        }
                        result_rows = deduped;
                    }
                    if let Some(off) = offset {
                        if off < result_rows.len() {
                            result_rows = result_rows.split_off(off);
                        } else {
                            result_rows.clear();
                        }
                    }
                    if let Some(lim) = limit {
                        result_rows.truncate(lim);
                    }
                    return Ok(result_rows);
                }

                // Resolve joined tables and partition WHERE predicates (push single-table predicates; defer right/cross-table to post-join)
                let mut joined_table_defs: Vec<&TableDef> = Vec::with_capacity(all_joins.len());
                for jc in &all_joins {
                    let rt_def = self
                        .catalog
                        .get_table(&jc.table)
                        .ok_or_else(|| Error::TableNotFound(jc.table.clone()))?;
                    joined_table_defs.push(rt_def);
                }
                let (left_pushed_where, right_pushed_wheres, post_join_where) =
                    if has_joins && let Some(ref r_expr) = resolved_where {
                        let mut tables_chain = Vec::with_capacity(1 + joined_table_defs.len());
                        tables_chain.push(&table_def);
                        tables_chain.extend(joined_table_defs.iter().copied());
                        let join_refs: Vec<&crate::sql::parser::JoinClause> =
                            all_joins.iter().collect();
                        split_join_where_predicates(r_expr, &tables_chain, &join_refs)
                    } else {
                        (None, vec![None; all_joins.len()], None)
                    };
                let left_scan_where: Option<&ResolvedWhereExpr> = if has_joins {
                    left_pushed_where.as_ref()
                } else {
                    single_scan_where.as_ref()
                };

                // Table scan left table
                let mut left_rows = Vec::new();
                if let Some(ts) = as_of_timestamp {
                    let is_versioned =
                        table_def.system_versioning || self.default_system_versioning;
                    if !is_versioned && self.catalog.get_table("__sys_time_travel").is_none() {
                        return Err(Error::SqlSyntax(format!(
                            "Table '{}' does not have SYSTEM_VERSIONING enabled. Enable with PRAGMA time_travel = ON or CREATE TABLE ... WITH (SYSTEM_VERSIONING = TRUE)",
                            table
                        )));
                    }
                    if let Some(sys_table) = self.catalog.get_table("__sys_time_travel") {
                        self.btree.scan_with_ref(
                            pager,
                            sys_table.root_page,
                            |_row_id, payload| {
                                if let Ok(rec) = serde_json::from_slice::<TemporalRecord>(payload)
                                    && rec.table_name.eq_ignore_ascii_case(&table)
                                    && rec.valid_from <= ts
                                    && rec.valid_to > ts
                                {
                                    let full_row = decode_row(&rec.payload, &all_col_names)?;
                                    if let Some(r_expr) = left_scan_where
                                        && !row_matches_resolved(&full_row, r_expr)
                                    {
                                        return Ok(true);
                                    }
                                    left_rows.push(full_row);
                                }
                                Ok(true)
                            },
                        )?;
                    }
                }

                // ── Direct Join Projection Pre-resolution ──
                // Applies when there is a single JOIN without GROUP BY, aggregates, ORDER BY, DISTINCT, or post-join residual filter.
                let (direct_join_sources, left_needed_mask) = if all_joins.len() == 1
                    && post_join_where.is_none()
                {
                    let jc = &all_joins[0];
                    let right_table_def = joined_table_defs[0];
                    let can_direct = order_by.is_empty()
                        && group_by.is_none()
                        && !distinct
                        && !has_agg
                        && !has_proj_sub
                        && !columns.is_empty();
                    if can_direct {
                        let mut sources = Vec::with_capacity(columns.len());
                        let mut ok = true;
                        for col in &columns {
                            if col.contains('(') || col.contains(' ') {
                                ok = false;
                                break;
                            }
                            if let Some((tbl, c)) = col.split_once('.') {
                                if tbl.eq_ignore_ascii_case(&table) {
                                    if let Some(idx) = table_def.column_index(c) {
                                        sources.push(JoinOutputCol::Left(idx));
                                        continue;
                                    }
                                } else if tbl.eq_ignore_ascii_case(&jc.table)
                                    && let Some(idx) = right_table_def.column_index(c)
                                {
                                    sources.push(JoinOutputCol::Right(idx));
                                    continue;
                                }
                                ok = false;
                                break;
                            } else {
                                let in_l = table_def.column_index(col);
                                let in_r = right_table_def.column_index(col);
                                match (in_l, in_r) {
                                    (Some(idx), None) => sources.push(JoinOutputCol::Left(idx)),
                                    (None, Some(idx)) => sources.push(JoinOutputCol::Right(idx)),
                                    _ => {
                                        ok = false;
                                        break;
                                    }
                                }
                            }
                        }
                        if ok {
                            let mut mask = vec![false; all_col_names.len()];
                            if let (Some(l_idx), _) = resolve_join_key_indices(
                                &all_col_names,
                                Some(&table),
                                right_table_def,
                                &jc.left_col,
                                &jc.right_col,
                            ) && l_idx < mask.len()
                            {
                                mask[l_idx] = true;
                            }
                            for src in &sources {
                                if let JoinOutputCol::Left(idx) = src
                                    && *idx < mask.len()
                                {
                                    mask[*idx] = true;
                                }
                            }
                            if let Some(lw) = left_scan_where {
                                let _ = mark_resolved_where_columns(lw, &table_def, &mut mask);
                            }
                            (Some(sources), Some(mask))
                        } else {
                            (None, None)
                        }
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                };

                // ── Indexed Seek Join (O(M log N) B-Tree Primary Key Point Lookup, supporting single-table WHERE filters) ──
                let seek_left_where = left_scan_where;
                let seek_right_where = right_pushed_wheres.first().and_then(|w| w.as_ref());
                if all_joins.len() == 1
                    && let Some(ref sources) = direct_join_sources
                    && as_of_timestamp.is_none()
                    && post_join_where.is_none()
                    && !seek_left_where.is_some_and(resolved_where_has_correlated)
                    && !seek_right_where.is_some_and(resolved_where_has_correlated)
                {
                    let jc = &all_joins[0];
                    if jc.join_type == JoinType::Inner && !jc.is_left {
                        let right_table_def = joined_table_defs[0];
                        let (l_col_idx, r_col_idx) = resolve_join_key_indices(
                            &all_col_names,
                            Some(&table),
                            right_table_def,
                            &jc.left_col,
                            &jc.right_col,
                        );

                        if let (Some(l_idx), Some(r_idx)) = (l_col_idx, r_col_idx) {
                            let left_is_int_pk = table_def.primary_key_index() == Some(l_idx)
                                && table_def.columns[l_idx].data_type
                                    == crate::sql::catalog::DataType::Integer;
                            let right_is_int_pk = right_table_def.primary_key_index()
                                == Some(r_idx)
                                && right_table_def.columns[r_idx].data_type
                                    == crate::sql::catalog::DataType::Integer;

                            let output_col_shared = Arc::new(columns.clone());
                            let left_col_shared = Arc::new(all_col_names.clone());
                            let right_col_names = right_table_def.column_names();
                            let right_col_shared = Arc::new(right_col_names.clone());
                            let skip_count = offset.unwrap_or(0);

                            let can_slice_left = seek_left_where.is_none_or(|lw| {
                                let mut chk = vec![false; all_col_names.len()];
                                mark_resolved_where_columns(lw, &table_def, &mut chk)
                            });
                            let eff_left_mask: Vec<bool> = if can_slice_left {
                                left_needed_mask
                                    .clone()
                                    .unwrap_or_else(|| vec![true; all_col_names.len()])
                            } else {
                                vec![true; all_col_names.len()]
                            };

                            let mut right_mask = vec![false; right_col_names.len()];
                            right_mask[r_idx] = true;
                            for src in sources {
                                if let JoinOutputCol::Right(idx) = src
                                    && *idx < right_mask.len()
                                {
                                    right_mask[*idx] = true;
                                }
                            }
                            let can_slice_right = seek_right_where.is_none_or(|rw| {
                                mark_resolved_where_columns(rw, right_table_def, &mut right_mask)
                            });
                            if !can_slice_right {
                                right_mask.fill(true);
                            }

                            // Case A: Left table join key is its integer PRIMARY KEY
                            if left_is_int_pk {
                                let mut driving_tuples: Vec<(i64, Vec<Value>)> = Vec::new();
                                let mut scratch_right = Vec::with_capacity(right_col_names.len());
                                self.btree.scan_with_ref(
                                    pager,
                                    right_table_def.root_page,
                                    |_row_id, payload| {
                                        scratch_right.clear();
                                        decode_row_values_into_projected(
                                            payload,
                                            &right_mask,
                                            &mut scratch_right,
                                        )?;
                                        if let Some(Value::Integer(k)) = scratch_right.get(r_idx)
                                            && eval_projected_where(
                                                &scratch_right,
                                                &right_col_shared,
                                                right_table_def,
                                                seek_right_where,
                                                can_slice_right,
                                            )
                                        {
                                            driving_tuples
                                                .push((*k, std::mem::take(&mut scratch_right)));
                                        }
                                        Ok(true)
                                    },
                                )?;

                                let mut final_rows = Vec::with_capacity(
                                    driving_tuples.len().min(limit.unwrap_or(usize::MAX)),
                                );
                                let mut skipped = 0;
                                let mut probe_left = Vec::with_capacity(left_col_shared.len());

                                for (k, r_vals) in driving_tuples {
                                    if let Some(l_payload) =
                                        self.btree.search(pager, root_page, k as u64)?
                                    {
                                        probe_left.clear();
                                        decode_row_values_into_projected(
                                            &l_payload,
                                            &eff_left_mask,
                                            &mut probe_left,
                                        )?;
                                        if !eval_projected_where(
                                            &probe_left,
                                            &left_col_shared,
                                            &table_def,
                                            seek_left_where,
                                            can_slice_left,
                                        ) {
                                            continue;
                                        }
                                        if skipped < skip_count {
                                            skipped += 1;
                                            continue;
                                        }
                                        let mut vals = Vec::with_capacity(sources.len());
                                        for src in sources {
                                            match src {
                                                JoinOutputCol::Left(idx) => vals.push(
                                                    probe_left
                                                        .get(*idx)
                                                        .cloned()
                                                        .unwrap_or(Value::Null),
                                                ),
                                                JoinOutputCol::Right(idx) => vals.push(
                                                    r_vals
                                                        .get(*idx)
                                                        .cloned()
                                                        .unwrap_or(Value::Null),
                                                ),
                                            }
                                        }
                                        final_rows.push(Row::with_shared_columns(
                                            output_col_shared.clone(),
                                            vals,
                                        ));
                                        if let Some(lim) = limit
                                            && final_rows.len() >= lim
                                        {
                                            break;
                                        }
                                    }
                                }

                                return Ok(final_rows);
                            }

                            // Case B: Right table join key is its integer PRIMARY KEY
                            if right_is_int_pk {
                                let mut driving_tuples: Vec<(i64, Vec<Value>)> = Vec::new();
                                let mut scratch_left = Vec::with_capacity(left_col_shared.len());
                                self.btree.scan_with_ref(
                                    pager,
                                    root_page,
                                    |_row_id, payload| {
                                        scratch_left.clear();
                                        decode_row_values_into_projected(
                                            payload,
                                            &eff_left_mask,
                                            &mut scratch_left,
                                        )?;
                                        if let Some(Value::Integer(k)) = scratch_left.get(l_idx)
                                            && eval_projected_where(
                                                &scratch_left,
                                                &left_col_shared,
                                                &table_def,
                                                seek_left_where,
                                                can_slice_left,
                                            )
                                        {
                                            driving_tuples
                                                .push((*k, std::mem::take(&mut scratch_left)));
                                        }
                                        Ok(true)
                                    },
                                )?;

                                let mut final_rows = Vec::with_capacity(
                                    driving_tuples.len().min(limit.unwrap_or(usize::MAX)),
                                );
                                let mut skipped = 0;
                                let mut probe_right = Vec::with_capacity(right_col_shared.len());

                                for (k, l_vals) in driving_tuples {
                                    if let Some(r_payload) = self.btree.search(
                                        pager,
                                        right_table_def.root_page,
                                        k as u64,
                                    )? {
                                        probe_right.clear();
                                        decode_row_values_into_projected(
                                            &r_payload,
                                            &right_mask,
                                            &mut probe_right,
                                        )?;
                                        if !eval_projected_where(
                                            &probe_right,
                                            &right_col_shared,
                                            right_table_def,
                                            seek_right_where,
                                            can_slice_right,
                                        ) {
                                            continue;
                                        }
                                        if skipped < skip_count {
                                            skipped += 1;
                                            continue;
                                        }
                                        let mut vals = Vec::with_capacity(sources.len());
                                        for src in sources {
                                            match src {
                                                JoinOutputCol::Left(idx) => vals.push(
                                                    l_vals
                                                        .get(*idx)
                                                        .cloned()
                                                        .unwrap_or(Value::Null),
                                                ),
                                                JoinOutputCol::Right(idx) => vals.push(
                                                    probe_right
                                                        .get(*idx)
                                                        .cloned()
                                                        .unwrap_or(Value::Null),
                                                ),
                                            }
                                        }
                                        final_rows.push(Row::with_shared_columns(
                                            output_col_shared.clone(),
                                            vals,
                                        ));
                                        if let Some(lim) = limit
                                            && final_rows.len() >= lim
                                        {
                                            break;
                                        }
                                    }
                                }

                                return Ok(final_rows);
                            }
                        }
                    }
                }

                let mut fts_left_col_shared: Option<Arc<Vec<String>>> = None;

                let mut order_by_satisfied = false;
                if let Some(fetched) = left_rows_from_index {
                    left_rows = fetched;
                } else if as_of_timestamp.is_none() {
                    let pk_order_desc = if is_int_pk
                        && !has_joins
                        && group_by.is_none()
                        && !has_agg
                        && !has_win
                        && !wants_fts_scores
                        && order_by.len() == 1
                        && let Some(pk_col_idx) = pk_idx
                    {
                        let ord = &order_by[0];
                        let pk_name = &table_def.columns[pk_col_idx].name;
                        let ord_col = ord
                            .column
                            .split_once('.')
                            .and_then(|(tbl, c)| tbl.eq_ignore_ascii_case(&table).then_some(c))
                            .unwrap_or(ord.column.as_str());
                        let shadowed_by_alias = columns.iter().any(|c| {
                            let (expr_part, alias_opt) = parse_col_and_alias(c);
                            alias_opt.is_some_and(|a| {
                                a.eq_ignore_ascii_case(ord_col)
                                    && !col_name_matches(expr_part, pk_name)
                            })
                        });
                        if !shadowed_by_alias && pk_name.eq_ignore_ascii_case(ord_col) {
                            order_by_satisfied = true;
                            Some(!ord.is_ascending)
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    let can_early_terminate = !has_joins
                        && !has_correlated
                        && (order_by.is_empty() || order_by_satisfied)
                        && group_by.is_none()
                        && !distinct
                        && !has_win
                        && !has_agg;
                    let needed_rows = if can_early_terminate {
                        limit.map(|lim| offset.unwrap_or(0).saturating_add(lim))
                    } else {
                        None
                    };
                    if needed_rows == Some(0) {
                        return Ok(Vec::new());
                    }

                    let left_col_shared = Arc::new(all_col_names.clone());
                    if wants_fts_scores {
                        let mut extended = all_col_names.clone();
                        extended.push("rank".to_string());
                        extended.push(format!("bm25({table})"));
                        fts_left_col_shared = Some(Arc::new(extended));
                    }

                    let eff_scan_mask =
                        left_needed_mask.as_deref().or(single_table_mask.as_deref());
                    let can_slice_eval = eff_scan_mask.is_some()
                        && fts_scores.is_none()
                        && left_scan_where.is_none_or(|w| {
                            // `single_table_mask` already verified `mark_resolved_where_columns`.
                            if left_needed_mask.is_some() {
                                let mut check_mask = vec![false; all_col_names.len()];
                                mark_resolved_where_columns(w, &table_def, &mut check_mask)
                            } else {
                                true
                            }
                        });
                    let mut scratch_vals = Vec::with_capacity(all_col_names.len());

                    let mut handle_row = |row_id: u64, payload: &[u8]| -> Result<bool> {
                        let mut full_row = if let Some(mask) = eff_scan_mask
                            && can_slice_eval
                        {
                            decode_row_values_into_projected(payload, mask, &mut scratch_vals)?;
                            if let Some(r_expr) = left_scan_where
                                && !slice_matches_resolved(&scratch_vals, &table_def, r_expr)
                            {
                                return Ok(true);
                            }
                            Row::with_shared_columns(
                                left_col_shared.clone(),
                                std::mem::take(&mut scratch_vals),
                            )
                        } else {
                            let row = if let Some(mask) = eff_scan_mask {
                                decode_row_shared_projected(payload, &left_col_shared, mask)?
                            } else {
                                decode_row_shared(payload, &left_col_shared)?
                            };

                            if let Some(r_expr) = left_scan_where {
                                if let Some(ref scores) = fts_scores {
                                    let eff_expr = resolve_fts_match_predicate(
                                        r_expr,
                                        &table,
                                        scores.contains_key(&row_id),
                                    );
                                    if !row_matches_resolved(&row, &eff_expr) {
                                        return Ok(true);
                                    }
                                } else if !row_matches_resolved(&row, r_expr) {
                                    return Ok(true);
                                }
                            }
                            row
                        };

                        if let (Some(scores), Some(ext_cols)) =
                            (fts_scores.as_ref(), fts_left_col_shared.as_ref())
                        {
                            let rank_val = scores.get(&row_id).copied().unwrap_or(0.0);
                            let mut vals = full_row.values().to_vec();
                            vals.push(Value::Real(rank_val));
                            vals.push(Value::Real(rank_val));
                            full_row = Row::with_shared_columns(ext_cols.clone(), vals);
                        }

                        left_rows.push(full_row);
                        if let Some(needed) = needed_rows
                            && left_rows.len() >= needed
                        {
                            return Ok(false);
                        }
                        Ok(true)
                    };

                    if pk_range.is_some() || pk_order_desc.is_some() {
                        let pk_min = pk_range.and_then(|r| r.0);
                        let pk_max = pk_range.and_then(|r| r.1);
                        let desc = pk_order_desc.unwrap_or(false);
                        self.btree.scan_signed_i64_with_ref(
                            pager,
                            root_page,
                            pk_min,
                            pk_max,
                            desc,
                            |rid, payload| handle_row(rid as u64, payload),
                        )?;
                    } else {
                        self.btree
                            .scan_with_ref(pager, root_page, |row_id, payload| {
                                handle_row(row_id, payload)
                            })?;
                    }
                }

                // Execute chained JOINs — optimized O(N+M) Hash Joins with predicate pushdown
                if !all_joins.is_empty() {
                    let mut current_left_cols: Vec<String> = all_col_names
                        .iter()
                        .map(|c| format!("{table}.{c}"))
                        .collect();

                    for (join_step_idx, join_clause) in all_joins.iter().enumerate() {
                        let right_table_def = joined_table_defs[join_step_idx];
                        let right_col_names = right_table_def.column_names();
                        let right_col_shared = Arc::new(right_col_names.clone());
                        let right_pushed = right_pushed_wheres
                            .get(join_step_idx)
                            .and_then(|w| w.as_ref());

                        // Precompute merged qualified column names for this join step
                        let mut merged_cols_template =
                            Vec::with_capacity(current_left_cols.len() + right_col_names.len());
                        merged_cols_template.extend(current_left_cols.iter().cloned());
                        for c in &right_col_names {
                            merged_cols_template.push(format!("{}.{}", join_clause.table, c));
                        }
                        let merged_cols_shared = Arc::new(merged_cols_template);

                        // Resolve join key indices against accumulated left columns & right table
                        let (l_col_idx, r_col_idx) = resolve_join_key_indices(
                            &current_left_cols,
                            None,
                            right_table_def,
                            &join_clause.left_col,
                            &join_clause.right_col,
                        );

                        // Build hash table on right table (with pushed single-table WHERE when INNER)
                        let mut right_rows = Vec::new();
                        if let Some(ts) = as_of_timestamp {
                            if let Some(sys_table) = self.catalog.get_table("__sys_time_travel") {
                                self.btree.scan_with(pager, sys_table.root_page, |cell| {
                                    if let Ok(rec) =
                                        serde_json::from_slice::<TemporalRecord>(&cell.payload)
                                        && rec.table_name.eq_ignore_ascii_case(&join_clause.table)
                                        && rec.valid_from <= ts
                                        && rec.valid_to > ts
                                    {
                                        let r_row =
                                            decode_row_shared(&rec.payload, &right_col_shared)?;
                                        if let Some(rw) = right_pushed
                                            && !row_matches_resolved(&r_row, rw)
                                        {
                                            return Ok(true);
                                        }
                                        right_rows.push(r_row);
                                    }
                                    Ok(true)
                                })?;
                            }
                        } else {
                            let right_mask = if let Some(ref sources) = direct_join_sources {
                                let mut mask = vec![false; right_col_names.len()];
                                if let Some(r_idx) = r_col_idx
                                    && r_idx < mask.len()
                                {
                                    mask[r_idx] = true;
                                }
                                for src in sources {
                                    if let JoinOutputCol::Right(idx) = src
                                        && *idx < mask.len()
                                    {
                                        mask[*idx] = true;
                                    }
                                }
                                if let Some(rw) = right_pushed {
                                    let _ =
                                        mark_resolved_where_columns(rw, right_table_def, &mut mask);
                                }
                                Some(mask)
                            } else {
                                None
                            };

                            let can_slice_right = right_mask.is_some()
                                && right_pushed.is_none_or(|rw| {
                                    let mut check_m = vec![false; right_col_names.len()];
                                    mark_resolved_where_columns(rw, right_table_def, &mut check_m)
                                });
                            let mut scratch_right = Vec::with_capacity(right_col_names.len());

                            self.btree.scan_with_ref(
                                pager,
                                right_table_def.root_page,
                                |_rid, payload| {
                                    if let Some(ref mask) = right_mask
                                        && can_slice_right
                                    {
                                        decode_row_values_into_projected(
                                            payload,
                                            mask,
                                            &mut scratch_right,
                                        )?;
                                        if let Some(rw) = right_pushed
                                            && !slice_matches_resolved(
                                                &scratch_right,
                                                right_table_def,
                                                rw,
                                            )
                                        {
                                            return Ok(true);
                                        }
                                        right_rows.push(Row::with_shared_columns(
                                            right_col_shared.clone(),
                                            std::mem::take(&mut scratch_right),
                                        ));
                                    } else {
                                        let r_row = if let Some(ref mask) = right_mask {
                                            decode_row_shared_projected(
                                                payload,
                                                &right_col_shared,
                                                mask,
                                            )?
                                        } else {
                                            decode_row_shared(payload, &right_col_shared)?
                                        };
                                        if let Some(rw) = right_pushed
                                            && !row_matches_resolved(&r_row, rw)
                                        {
                                            return Ok(true);
                                        }
                                        right_rows.push(r_row);
                                    }
                                    Ok(true)
                                },
                            )?;
                        }

                        // Check whether join key is an integer column for fast u64-keyed HashMap
                        let all_int_keys = r_col_idx.is_some_and(|idx| {
                            right_rows
                                .first()
                                .and_then(|r| r.values().get(idx))
                                .is_some_and(|v| matches!(v, Value::Integer(_)))
                        }) && l_col_idx.is_some_and(|idx| {
                            left_rows
                                .first()
                                .and_then(|l| l.values().get(idx))
                                .is_some_and(|v| matches!(v, Value::Integer(_)))
                        });

                        let is_right_or_full = join_clause.join_type == JoinType::Right
                            || join_clause.join_type == JoinType::Full;
                        let mut joined_rows = Vec::with_capacity(left_rows.len());

                        if all_int_keys && !is_right_or_full {
                            // ── Fast path: FastIntHasher + IntEntry ──
                            let r_idx_col = r_col_idx.unwrap();
                            let l_idx_col = l_col_idx.unwrap();
                            let mut int_hash: HashMap<i64, IntEntry, FastIntBuildHasher> =
                                HashMap::with_capacity_and_hasher(
                                    right_rows.len(),
                                    FastIntBuildHasher::default(),
                                );

                            for (r_idx, r_row) in right_rows.iter().enumerate() {
                                if let Some(Value::Integer(k)) = r_row.values().get(r_idx_col) {
                                    match int_hash.get_mut(k) {
                                        Some(IntEntry::One(prev)) => {
                                            let first = *prev;
                                            int_hash.insert(*k, IntEntry::Many(vec![first, r_idx]));
                                        }
                                        Some(IntEntry::Many(list)) => {
                                            list.push(r_idx);
                                        }
                                        None => {
                                            int_hash.insert(*k, IntEntry::One(r_idx));
                                        }
                                    }
                                }
                            }

                            if let Some(ref direct_sources) = direct_join_sources {
                                // ── Direct Join Projection: Zero intermediate Row allocations ──
                                let output_col_shared = Arc::new(columns.clone());
                                let mut final_rows =
                                    Vec::with_capacity(left_rows.len().min(right_rows.len() * 2));

                                for l_row in &left_rows {
                                    let mut matched = false;
                                    if let Some(Value::Integer(lk)) = l_row.values().get(l_idx_col)
                                        && let Some(entry) = int_hash.get(lk)
                                    {
                                        matched = true;
                                        let r_slice = match entry {
                                            IntEntry::One(idx) => std::slice::from_ref(idx),
                                            IntEntry::Many(v) => v.as_slice(),
                                        };
                                        for &r_idx in r_slice {
                                            let r_row = &right_rows[r_idx];
                                            let mut vals = Vec::with_capacity(direct_sources.len());
                                            for src in direct_sources {
                                                match src {
                                                    JoinOutputCol::Left(idx) => vals.push(
                                                        l_row
                                                            .values()
                                                            .get(*idx)
                                                            .cloned()
                                                            .unwrap_or(Value::Null),
                                                    ),
                                                    JoinOutputCol::Right(idx) => vals.push(
                                                        r_row
                                                            .values()
                                                            .get(*idx)
                                                            .cloned()
                                                            .unwrap_or(Value::Null),
                                                    ),
                                                }
                                            }
                                            final_rows.push(Row::with_shared_columns(
                                                output_col_shared.clone(),
                                                vals,
                                            ));
                                        }
                                    }
                                    if !matched
                                        && (join_clause.join_type == JoinType::Left
                                            || join_clause.is_left)
                                    {
                                        let mut vals = Vec::with_capacity(direct_sources.len());
                                        for src in direct_sources {
                                            match src {
                                                JoinOutputCol::Left(idx) => vals.push(
                                                    l_row
                                                        .values()
                                                        .get(*idx)
                                                        .cloned()
                                                        .unwrap_or(Value::Null),
                                                ),
                                                JoinOutputCol::Right(_) => vals.push(Value::Null),
                                            }
                                        }
                                        final_rows.push(Row::with_shared_columns(
                                            output_col_shared.clone(),
                                            vals,
                                        ));
                                    }
                                }

                                let skip_count = offset.unwrap_or(0);
                                if skip_count > 0 || limit.is_some() {
                                    let mut sliced = Vec::new();
                                    let mut skipped = 0;
                                    for r in final_rows {
                                        if skipped < skip_count {
                                            skipped += 1;
                                            continue;
                                        }
                                        sliced.push(r);
                                        if let Some(lim) = limit
                                            && sliced.len() >= lim
                                        {
                                            break;
                                        }
                                    }
                                    return Ok(sliced);
                                }
                                return Ok(final_rows);
                            }

                            for l_row in &left_rows {
                                let mut matched = false;
                                if let Some(Value::Integer(lk)) = l_row.values().get(l_idx_col)
                                    && let Some(entry) = int_hash.get(lk)
                                {
                                    matched = true;
                                    let r_slice = match entry {
                                        IntEntry::One(idx) => std::slice::from_ref(idx),
                                        IntEntry::Many(v) => v.as_slice(),
                                    };
                                    for &r_idx in r_slice {
                                        let r_row = &right_rows[r_idx];
                                        let mut merged_vals = Vec::with_capacity(
                                            l_row.values().len() + r_row.values().len(),
                                        );
                                        merged_vals.extend_from_slice(l_row.values());
                                        merged_vals.extend_from_slice(r_row.values());
                                        joined_rows.push(Row::with_shared_columns(
                                            merged_cols_shared.clone(),
                                            merged_vals,
                                        ));
                                    }
                                }
                                if !matched
                                    && (join_clause.join_type == JoinType::Left
                                        || join_clause.is_left)
                                {
                                    let mut merged_vals = Vec::with_capacity(
                                        l_row.values().len() + right_col_names.len(),
                                    );
                                    merged_vals.extend_from_slice(l_row.values());
                                    merged_vals.resize(
                                        merged_vals.len() + right_col_names.len(),
                                        Value::Null,
                                    );
                                    joined_rows.push(Row::with_shared_columns(
                                        merged_cols_shared.clone(),
                                        merged_vals,
                                    ));
                                }
                            }
                        } else {
                            // ── General path: HashableValue HashMap ──
                            let mut right_hash_table: HashMap<HashableValue, Vec<usize>> =
                                HashMap::with_capacity(right_rows.len());
                            for (r_idx, r_row) in right_rows.iter().enumerate() {
                                let r_val = if let Some(idx) = r_col_idx {
                                    r_row.values().get(idx)
                                } else {
                                    r_row.get_value(&join_clause.right_col)
                                };
                                if let Some(r_val) = r_val
                                    && !r_val.is_null()
                                {
                                    right_hash_table
                                        .entry(HashableValue::from(r_val))
                                        .or_default()
                                        .push(r_idx);
                                }
                            }

                            let mut matched_right_indices = if is_right_or_full {
                                HashSet::with_capacity(right_rows.len())
                            } else {
                                HashSet::new()
                            };

                            for l_row in &left_rows {
                                let l_val = if let Some(idx) = l_col_idx {
                                    l_row.values().get(idx)
                                } else {
                                    l_row.get_value(&join_clause.left_col)
                                };
                                let mut matched = false;

                                if let Some(lv) = l_val
                                    && !lv.is_null()
                                    && let Some(matching_r_indices) =
                                        right_hash_table.get(&HashableValue::from(lv))
                                {
                                    matched = true;
                                    for &r_idx in matching_r_indices {
                                        let r_row = &right_rows[r_idx];
                                        if is_right_or_full {
                                            matched_right_indices.insert(r_idx);
                                        }
                                        let mut merged_vals = Vec::with_capacity(
                                            l_row.values().len() + r_row.values().len(),
                                        );
                                        merged_vals.extend_from_slice(l_row.values());
                                        merged_vals.extend_from_slice(r_row.values());
                                        joined_rows.push(Row::with_shared_columns(
                                            merged_cols_shared.clone(),
                                            merged_vals,
                                        ));
                                    }
                                }

                                if !matched
                                    && (join_clause.join_type == JoinType::Left
                                        || join_clause.join_type == JoinType::Full)
                                {
                                    let mut merged_vals = Vec::with_capacity(
                                        l_row.values().len() + right_col_names.len(),
                                    );
                                    merged_vals.extend_from_slice(l_row.values());
                                    merged_vals.resize(
                                        merged_vals.len() + right_col_names.len(),
                                        Value::Null,
                                    );
                                    joined_rows.push(Row::with_shared_columns(
                                        merged_cols_shared.clone(),
                                        merged_vals,
                                    ));
                                }
                            }

                            if is_right_or_full {
                                let left_width = current_left_cols.len();
                                for (r_idx, r_row) in right_rows.iter().enumerate() {
                                    if !matched_right_indices.contains(&r_idx) {
                                        let mut merged_vals =
                                            Vec::with_capacity(left_width + right_col_names.len());
                                        merged_vals.resize(left_width, Value::Null);
                                        merged_vals.extend_from_slice(r_row.values());
                                        joined_rows.push(Row::with_shared_columns(
                                            merged_cols_shared.clone(),
                                            merged_vals,
                                        ));
                                    }
                                }
                            }
                        }

                        left_rows = joined_rows;
                        current_left_cols = Arc::unwrap_or_clone(merged_cols_shared);
                    }

                    if let Some(ref post_w) = post_join_where {
                        let outer_tables: Vec<TableDef> = std::iter::once(table_def.clone())
                            .chain(joined_table_defs.iter().map(|t| (*t).clone()))
                            .collect();
                        let outer_refs: Vec<&TableDef> = outer_tables.iter().collect();
                        self.retain_rows_with_correlated_where(
                            pager,
                            &mut left_rows,
                            post_w,
                            &outer_refs,
                        )?;
                    }
                } else if let Some(ref corr_w) = single_post_correlated {
                    let outer_td = table_def.clone();
                    self.retain_rows_with_correlated_where(
                        pager,
                        &mut left_rows,
                        corr_w,
                        &[&outer_td],
                    )?;
                }

                // Check for GROUP BY (High-Performance O(N) Hash Aggregation fallback for joins/temporal)
                if let Some(ref gb_cols) = group_by {
                    left_rows = aggregate_ephemeral_rows(
                        &left_rows,
                        &columns,
                        gb_cols,
                        resolved_having.as_ref(),
                    );
                } else if is_aggregate_query(&columns) {
                    let mut agg_cols = Vec::new();
                    let mut agg_vals = Vec::new();
                    let empty_row = Row::new(Vec::new(), Vec::new());

                    for col in &columns {
                        if let Some(spec) = parse_aggregate_spec(col) {
                            let computed = compute_aggregate_from_spec(&spec, &left_rows);
                            agg_cols.push(spec.output_name);
                            agg_vals.push(computed);
                        } else if !crate::sql::parser::extract_select_scalar_subqueries(col)
                            .is_empty()
                        {
                            let plan = self.prepare_projection_plan(
                                pager,
                                std::slice::from_ref(col),
                                &[],
                            )?;
                            let mut memo = CorrelatedSubqueryMemo::default();
                            agg_cols.push(plan[0].out_name.clone());
                            agg_vals.push(self.eval_prepared_projection_col(
                                pager,
                                &empty_row,
                                &plan[0],
                                &[],
                                &mut memo,
                            )?);
                        } else {
                            agg_cols.push(parse_col_and_alias(col).1.unwrap_or(col).to_string());
                            agg_vals.push(Value::Null);
                        }
                    }

                    return Ok(vec![Row::new(agg_cols, agg_vals)]);
                }

                // Evaluate window functions if present in columns
                let has_win_fn = columns.iter().any(|c| has_window_function(c));
                if has_win_fn {
                    evaluate_window_functions(&mut left_rows, &columns)?;
                }

                let mut outer_defs = Vec::new();
                if has_proj_sub {
                    outer_defs.push(table_def.clone());
                    for jc in &all_joins {
                        if let Some(rt) = self.catalog.get_table(&jc.table) {
                            outer_defs.push(rt.clone());
                        }
                    }
                }
                let outer_refs_slice: Vec<&TableDef> = outer_defs.iter().collect();
                let proj_plan = if has_proj_sub {
                    Some(self.prepare_projection_plan(pager, &columns, &outer_refs_slice)?)
                } else {
                    None
                };
                let mut proj_memo = CorrelatedSubqueryMemo::default();

                // Apply ORDER BY sorting if not already satisfied by ordered B+Tree PK scan
                if !order_by.is_empty() && !order_by_satisfied {
                    if let Some(ref plan) = proj_plan {
                        self.enrich_rows_with_order_plan(
                            pager,
                            &mut left_rows,
                            plan,
                            &order_by,
                            &outer_refs_slice,
                            &mut proj_memo,
                        )?;
                    } else {
                        enrich_rows_with_order_aliases(self, &mut left_rows, &columns, &order_by)?;
                    }
                    let cmp = |a: &Row, b: &Row| compare_rows_by_order(a, b, &order_by);
                    if let Some(lim) = limit
                        && !distinct
                        && !has_win_fn
                    {
                        let keep = offset.unwrap_or(0).saturating_add(lim);
                        if keep == 0 {
                            left_rows.clear();
                        } else if keep < left_rows.len() {
                            left_rows.select_nth_unstable_by(keep - 1, cmp);
                            left_rows.truncate(keep);
                            left_rows.sort_by(cmp);
                        } else {
                            left_rows.sort_by(cmp);
                        }
                    } else {
                        left_rows.sort_by(cmp);
                    }
                }

                // Project columns and apply LIMIT
                let output_col_names = if columns.is_empty() {
                    if wants_fts_scores {
                        all_col_names.clone()
                    } else if let Some(first_row) = left_rows.first() {
                        first_row.columns().to_vec()
                    } else {
                        all_col_names.clone()
                    }
                } else {
                    columns.clone()
                };

                let output_col_shared = Arc::new(output_col_names);

                // Fast-path: Pre-resolve column indices for direct O(1) projection if columns are simple
                let mut direct_indices: Option<Vec<usize>> = None;
                if proj_plan.is_none()
                    && let Some(first) = left_rows.first()
                {
                    let mut indices = Vec::with_capacity(output_col_shared.len());
                    let mut all_matched = true;
                    for col in output_col_shared.iter() {
                        let col_lower = col.to_ascii_lowercase();
                        let is_direct_bm25 =
                            col_lower.starts_with("bm25(") && !col_lower.contains(" as ");
                        if !is_direct_bm25 && (col.contains('(') || col.contains(' ')) {
                            all_matched = false;
                            break;
                        }
                        if let Some(pos) = first
                            .columns()
                            .iter()
                            .position(|c| col_name_matches(c, col))
                        {
                            indices.push(pos);
                        } else {
                            all_matched = false;
                            break;
                        }
                    }
                    if all_matched {
                        direct_indices = Some(indices);
                    }
                }

                let mut rows = Vec::new();
                let skip_count = offset.unwrap_or(0);
                let mut skipped = 0;
                let mut distinct_set = RowDistinctSet::new();
                for row in left_rows {
                    let projected = if let Some(ref plan) = proj_plan {
                        self.project_row_with_plan(
                            pager,
                            &row,
                            plan,
                            &outer_refs_slice,
                            &mut proj_memo,
                        )?
                    } else if let Some(ref indices) = direct_indices {
                        let mut vals = Vec::with_capacity(indices.len());
                        for &idx in indices {
                            vals.push(row.values().get(idx).cloned().unwrap_or(Value::Null));
                        }
                        Row::with_shared_columns(output_col_shared.clone(), vals)
                    } else {
                        self.project_row(&row, &output_col_shared)?
                    };

                    if distinct {
                        if distinct_set.push_paginated(&mut rows, projected, skip_count, limit) {
                            break;
                        }
                        continue;
                    }
                    if skipped < skip_count {
                        skipped += 1;
                        continue;
                    }
                    rows.push(projected);

                    if let Some(lim) = limit
                        && rows.len() >= lim
                    {
                        break;
                    }
                }

                Ok(rows)
            }

            Statement::VectorSearch {
                columns,
                table,
                query_vector,
                top_k,
                where_clause,
                ..
            } => {
                if top_k == 0 {
                    return Ok(Vec::new());
                }
                let table_def = self
                    .catalog
                    .get_table(&table)
                    .cloned()
                    .ok_or_else(|| Error::TableNotFound(table.clone()))?;

                let root_page = table_def.root_page;
                let all_col_names = table_def.column_names();
                let all_col_shared = Arc::new(all_col_names.clone());
                let output_col_names = if columns.is_empty() {
                    all_col_names.clone()
                } else {
                    columns.clone()
                };

                let resolved_where = if let Some(ref expr) = where_clause {
                    Some(self.resolve_where_expr_for_tables(pager, expr, &[&table_def])?)
                } else {
                    None
                };
                let has_corr = resolved_where
                    .as_ref()
                    .is_some_and(resolved_where_has_correlated);

                let mut rows = Vec::new();
                if self.vector_indexes.contains_key(&table.to_lowercase()) {
                    // HNSW Vector Index path: pre-filter candidate IDs via index or single streaming pass
                    let candidate_ids = if let Some(ref resolved) = resolved_where {
                        let mut matched = std::collections::HashSet::new();
                        let mut used_index = false;
                        if let ResolvedWhereExpr::Condition(cond) = resolved
                            && cond.op == BinaryOp::Equals
                            && let Some(index_def) = self.find_usable_index_for_column(
                                &table,
                                &cond.column,
                                where_clause.as_ref(),
                            )
                        {
                            let key = value_to_index_key(&cond.value);
                            if let Some(bucket) =
                                IndexBucket::load(&self.btree, pager, index_def.root_page, key)?
                                && let Some(rids) = bucket.get_row_ids(&cond.value)
                            {
                                for &rid in rids {
                                    matched.insert(rid);
                                }
                                used_index = true;
                            }
                        }

                        if !used_index {
                            let (scan_where, corr_where) = if has_corr {
                                split_correlated_and_predicates(resolved)
                            } else {
                                (Some(resolved.clone()), None)
                            };
                            if let Some(cw) = corr_where {
                                let mut cand_rows: Vec<(u64, Row)> = Vec::new();
                                self.btree
                                    .scan_with_ref(pager, root_page, |row_id, payload| {
                                        let row = decode_row_shared(payload, &all_col_shared)?;
                                        if scan_where
                                            .as_ref()
                                            .is_none_or(|sw| row_matches_resolved(&row, sw))
                                        {
                                            cand_rows.push((row_id, row));
                                        }
                                        Ok(true)
                                    })?;
                                let mut memo = CorrelatedSubqueryMemo::default();
                                for (row_id, row) in cand_rows {
                                    if self.eval_resolved_where_with_subqueries(
                                        pager,
                                        &row,
                                        &cw,
                                        &[&table_def],
                                        &mut memo,
                                    )? {
                                        matched.insert(row_id);
                                    }
                                }
                            } else {
                                let mut filter_mask = vec![false; all_col_names.len()];
                                let can_slice = mark_resolved_where_columns(
                                    resolved,
                                    &table_def,
                                    &mut filter_mask,
                                );
                                let mut scratch = Vec::with_capacity(all_col_names.len());
                                self.btree
                                    .scan_with_ref(pager, root_page, |row_id, payload| {
                                        if can_slice {
                                            scratch.clear();
                                            decode_row_values_into_projected(
                                                payload,
                                                &filter_mask,
                                                &mut scratch,
                                            )?;
                                            if slice_matches_resolved(
                                                &scratch, &table_def, resolved,
                                            ) {
                                                matched.insert(row_id);
                                            }
                                        } else {
                                            let row = decode_row_shared(payload, &all_col_shared)?;
                                            if row_matches_resolved(&row, resolved) {
                                                matched.insert(row_id);
                                            }
                                        }
                                        Ok(true)
                                    })?;
                            }
                        }
                        Some(matched)
                    } else {
                        None
                    };

                    let neighbors = self
                        .vector_indexes
                        .get(&table.to_lowercase())
                        .expect("checked above")
                        .search_knn_filtered(
                            &query_vector,
                            top_k,
                            DistanceMetric::Cosine,
                            candidate_ids.as_ref(),
                        )?;
                    for (row_id, _) in neighbors {
                        if let Some(payload) = self.btree.search(pager, root_page, row_id)? {
                            let full_row = decode_row_shared(&payload, &all_col_shared)?;
                            rows.push(self.project_row(&full_row, &output_col_names)?);
                        }
                    }
                } else if let Some((vec_idx, _)) = table_def.vector_column() {
                    // Single-pass Streaming Exact Flat KNN with Projection + Bounded Max-Heap (O(N log K), O(K) memory)
                    let mut scan_mask = compute_single_table_needed_mask(
                        &table_def,
                        &output_col_names,
                        resolved_where.as_ref(),
                        &[],
                        false,
                    );
                    if let Some(ref mut mask) = scan_mask
                        && vec_idx < mask.len()
                    {
                        mask[vec_idx] = true;
                    }

                    let mut heap: std::collections::BinaryHeap<TopKVecEntry> =
                        std::collections::BinaryHeap::with_capacity(top_k.saturating_add(1));
                    let mut seq = 0usize;

                    if has_corr {
                        let Some(ref r_expr) = resolved_where else {
                            unreachable!();
                        };
                        let (scan_where, corr_where) = split_correlated_and_predicates(r_expr);
                        let mut cand_rows = Vec::new();
                        self.btree
                            .scan_with_ref(pager, root_page, |_row_id, payload| {
                                let row = if let Some(ref mask) = scan_mask {
                                    decode_row_shared_projected(payload, &all_col_shared, mask)?
                                } else {
                                    decode_row_shared(payload, &all_col_shared)?
                                };
                                if scan_where
                                    .as_ref()
                                    .is_none_or(|sw| row_matches_resolved(&row, sw))
                                {
                                    cand_rows.push(row);
                                }
                                Ok(true)
                            })?;
                        if let Some(cw) = corr_where {
                            self.retain_rows_with_correlated_where(
                                pager,
                                &mut cand_rows,
                                &cw,
                                &[&table_def],
                            )?;
                        }
                        for row in cand_rows {
                            if let Some(Value::Vector(v)) = row.values().get(vec_idx) {
                                let dist = crate::vector::cosine_distance(&query_vector, v);
                                push_top_k_heap(&mut heap, top_k, dist, &mut seq, || row.clone());
                            }
                        }
                    } else {
                        let can_slice = scan_mask.is_some()
                            && resolved_where.as_ref().is_none_or(|w| {
                                let mut chk = vec![false; all_col_names.len()];
                                mark_resolved_where_columns(w, &table_def, &mut chk)
                            });
                        let mut scratch_vals = Vec::with_capacity(all_col_names.len());
                        self.btree
                            .scan_with_ref(pager, root_page, |_row_id, payload| {
                                if let Some(ref mask) = scan_mask
                                    && can_slice
                                {
                                    scratch_vals.clear();
                                    decode_row_values_into_projected(
                                        payload,
                                        mask,
                                        &mut scratch_vals,
                                    )?;
                                    if let Some(ref r_expr) = resolved_where
                                        && !slice_matches_resolved(
                                            &scratch_vals,
                                            &table_def,
                                            r_expr,
                                        )
                                    {
                                        return Ok(true);
                                    }
                                    if let Some(Value::Vector(v)) = scratch_vals.get(vec_idx) {
                                        let dist = crate::vector::cosine_distance(&query_vector, v);
                                        push_top_k_heap(&mut heap, top_k, dist, &mut seq, || {
                                            Row::with_shared_columns(
                                                all_col_shared.clone(),
                                                std::mem::take(&mut scratch_vals),
                                            )
                                        });
                                    }
                                } else {
                                    let row = decode_row_shared(payload, &all_col_shared)?;
                                    if let Some(ref r_expr) = resolved_where
                                        && !row_matches_resolved(&row, r_expr)
                                    {
                                        return Ok(true);
                                    }
                                    if let Some(Value::Vector(v)) = row.values().get(vec_idx) {
                                        let dist = crate::vector::cosine_distance(&query_vector, v);
                                        push_top_k_heap(&mut heap, top_k, dist, &mut seq, || {
                                            row.clone()
                                        });
                                    }
                                }
                                Ok(true)
                            })?;
                    }

                    for entry in heap.into_sorted_vec() {
                        rows.push(self.project_row(&entry.row, &output_col_names)?);
                    }
                } else {
                    // Table without a vector column: stream matching rows up to top_k
                    if has_corr {
                        let Some(ref r_expr) = resolved_where else {
                            unreachable!();
                        };
                        let (scan_where, corr_where) = split_correlated_and_predicates(r_expr);
                        let mut cand_rows = Vec::new();
                        self.btree
                            .scan_with_ref(pager, root_page, |_row_id, payload| {
                                let row = decode_row_shared(payload, &all_col_shared)?;
                                if scan_where
                                    .as_ref()
                                    .is_none_or(|sw| row_matches_resolved(&row, sw))
                                {
                                    cand_rows.push(row);
                                }
                                Ok(true)
                            })?;
                        if let Some(cw) = corr_where {
                            self.retain_rows_with_correlated_where(
                                pager,
                                &mut cand_rows,
                                &cw,
                                &[&table_def],
                            )?;
                        }
                        for row in cand_rows.into_iter().take(top_k) {
                            rows.push(self.project_row(&row, &output_col_names)?);
                        }
                    } else {
                        self.btree
                            .scan_with_ref(pager, root_page, |_row_id, payload| {
                                let row = decode_row_shared(payload, &all_col_shared)?;
                                if let Some(ref r_expr) = resolved_where
                                    && !row_matches_resolved(&row, r_expr)
                                {
                                    return Ok(true);
                                }
                                rows.push(self.project_row(&row, &output_col_names)?);
                                Ok(rows.len() < top_k)
                            })?;
                    }
                }

                Ok(rows)
            }

            Statement::GraphTraverse {
                start_id,
                direction,
                label,
                max_depth,
                where_clause,
            } => {
                let cols = vec![
                    "node_id".to_string(),
                    "label".to_string(),
                    "properties".to_string(),
                    "depth".to_string(),
                ];
                let mut rows = Vec::new();
                let mut visited = std::collections::HashSet::new();
                let mut queue = std::collections::VecDeque::new();

                let resolved_where = if let Some(ref expr) = where_clause {
                    Some(self.resolve_where_expr(pager, expr)?)
                } else {
                    None
                };

                if let Some(start_node) = self.graph.get_node(start_id) {
                    visited.insert(start_id);
                    rows.push(Row::new(
                        cols.clone(),
                        vec![
                            Value::Integer(start_node.id as i64),
                            Value::Text(start_node.label.clone()),
                            Value::Text(start_node.properties.clone()),
                            Value::Integer(0),
                        ],
                    ));
                    queue.push_back((start_id, 0usize));
                }

                while let Some((curr_id, depth)) = queue.pop_front() {
                    if depth >= max_depth {
                        continue;
                    }
                    let neighbors = self
                        .graph
                        .neighbor_refs(curr_id, direction, label.as_deref());
                    for (neighbor_node, edge) in neighbors {
                        if visited.contains(&neighbor_node.id) {
                            continue;
                        }

                        // Evaluate predicate filter if provided
                        if let Some(ref rw) = resolved_where {
                            let mut test_cols = vec![
                                "node_id".to_string(),
                                "id".to_string(),
                                "target.id".to_string(),
                                "label".to_string(),
                                "target.label".to_string(),
                                "depth".to_string(),
                                "weight".to_string(),
                                "edge.weight".to_string(),
                                "edge.label".to_string(),
                            ];
                            let mut test_vals = vec![
                                Value::Integer(neighbor_node.id as i64),
                                Value::Integer(neighbor_node.id as i64),
                                Value::Integer(neighbor_node.id as i64),
                                Value::Text(neighbor_node.label.clone()),
                                Value::Text(neighbor_node.label.clone()),
                                Value::Integer((depth + 1) as i64),
                                Value::Real(edge.weight as f64),
                                Value::Real(edge.weight as f64),
                                Value::Text(edge.label.clone()),
                            ];

                            // Extract JSON properties from neighbor_node
                            if let Ok(serde_json::Value::Object(map)) =
                                serde_json::from_str::<serde_json::Value>(&neighbor_node.properties)
                            {
                                for (k, v) in map {
                                    let db_val = match v {
                                        serde_json::Value::Number(n) => {
                                            if let Some(i) = n.as_i64() {
                                                Value::Integer(i)
                                            } else if let Some(f) = n.as_f64() {
                                                Value::Real(f)
                                            } else {
                                                Value::Null
                                            }
                                        }
                                        serde_json::Value::String(s) => Value::Text(s),
                                        serde_json::Value::Bool(b) => {
                                            Value::Integer(if b { 1 } else { 0 })
                                        }
                                        serde_json::Value::Null => Value::Null,
                                        other => Value::Text(other.to_string()),
                                    };
                                    test_cols.push(k.clone());
                                    test_cols.push(format!("target.{k}"));
                                    test_vals.push(db_val.clone());
                                    test_vals.push(db_val);
                                }
                            }

                            let test_row = Row::new(test_cols, test_vals);
                            if !row_matches_resolved(&test_row, rw) {
                                continue;
                            }
                        }

                        if visited.insert(neighbor_node.id) {
                            rows.push(Row::new(
                                cols.clone(),
                                vec![
                                    Value::Integer(neighbor_node.id as i64),
                                    Value::Text(neighbor_node.label.clone()),
                                    Value::Text(neighbor_node.properties.clone()),
                                    Value::Integer((depth + 1) as i64),
                                ],
                            ));
                            queue.push_back((neighbor_node.id, depth + 1));
                        }
                    }
                }
                Ok(rows)
            }

            Statement::GraphShortestPath { start_id, end_id } => {
                let cols = vec![
                    "step".to_string(),
                    "from_id".to_string(),
                    "to_id".to_string(),
                    "label".to_string(),
                    "weight".to_string(),
                    "total_cost".to_string(),
                ];
                let mut rows = Vec::new();
                if let Some((path, total_cost)) =
                    self.graph.dijkstra_shortest_path(start_id, end_id)
                {
                    for (step_idx, edge) in path.into_iter().enumerate() {
                        rows.push(Row::new(
                            cols.clone(),
                            vec![
                                Value::Integer((step_idx + 1) as i64),
                                Value::Integer(edge.from_id as i64),
                                Value::Integer(edge.to_id as i64),
                                Value::Text(edge.label),
                                Value::Real(edge.weight as f64),
                                Value::Real(total_cost as f64),
                            ],
                        ));
                    }
                }
                Ok(rows)
            }

            Statement::GraphMatch {
                source_var,
                source_label,
                rel_var,
                rel_label,
                target_var,
                target_label,
                where_clause,
                return_items,
            } => {
                let mut rows = Vec::new();
                let resolved_where = if let Some(ref expr) = where_clause {
                    Some(self.resolve_where_expr(pager, expr)?)
                } else {
                    None
                };

                let output_cols = return_items.clone();

                for edge in self.graph.all_edges() {
                    // Filter edge label
                    if let Some(ref rl) = rel_label
                        && !edge.label.eq_ignore_ascii_case(rl)
                    {
                        continue;
                    }

                    // Source node
                    let source_node = match self.graph.get_node(edge.from_id) {
                        Some(n) => n,
                        None => continue,
                    };
                    if let Some(ref sl) = source_label
                        && !source_node.label.eq_ignore_ascii_case(sl)
                    {
                        continue;
                    }

                    // Target node
                    let target_node = match self.graph.get_node(edge.to_id) {
                        Some(n) => n,
                        None => continue,
                    };
                    if let Some(ref tl) = target_label
                        && !target_node.label.eq_ignore_ascii_case(tl)
                    {
                        continue;
                    }

                    // Build column-value test environment for WHERE filter
                    let mut test_cols = Vec::new();
                    let mut test_vals = Vec::new();

                    // Source variables
                    test_cols.push(format!("{source_var}.id"));
                    test_vals.push(Value::Integer(source_node.id as i64));
                    test_cols.push(format!("{source_var}.label"));
                    test_vals.push(Value::Text(source_node.label.clone()));
                    test_cols.push(format!("{source_var}.properties"));
                    test_vals.push(Value::Text(source_node.properties.clone()));

                    // Target variables
                    test_cols.push(format!("{target_var}.id"));
                    test_vals.push(Value::Integer(target_node.id as i64));
                    test_cols.push(format!("{target_var}.label"));
                    test_vals.push(Value::Text(target_node.label.clone()));
                    test_cols.push(format!("{target_var}.properties"));
                    test_vals.push(Value::Text(target_node.properties.clone()));

                    // Relationship variables
                    if let Some(ref rv) = rel_var {
                        test_cols.push(format!("{rv}.id"));
                        test_vals.push(Value::Integer(edge.id as i64));
                        test_cols.push(format!("{rv}.label"));
                        test_vals.push(Value::Text(edge.label.clone()));
                        test_cols.push(format!("{rv}.weight"));
                        test_vals.push(Value::Real(edge.weight as f64));
                        test_cols.push(format!("{rv}.properties"));
                        test_vals.push(Value::Text(edge.properties.clone()));
                    }

                    // Unqualified fallbacks
                    test_cols.push("edge.id".to_string());
                    test_vals.push(Value::Integer(edge.id as i64));
                    test_cols.push("edge.label".to_string());
                    test_vals.push(Value::Text(edge.label.clone()));
                    test_cols.push("edge.weight".to_string());
                    test_vals.push(Value::Real(edge.weight as f64));

                    // Parse JSON properties for source node
                    if let Ok(serde_json::Value::Object(map)) =
                        serde_json::from_str::<serde_json::Value>(&source_node.properties)
                    {
                        for (k, v) in map {
                            let db_val = match v {
                                serde_json::Value::Number(n) => {
                                    if let Some(i) = n.as_i64() {
                                        Value::Integer(i)
                                    } else if let Some(f) = n.as_f64() {
                                        Value::Real(f)
                                    } else {
                                        Value::Null
                                    }
                                }
                                serde_json::Value::String(s) => Value::Text(s),
                                serde_json::Value::Bool(b) => Value::Integer(if b { 1 } else { 0 }),
                                serde_json::Value::Null => Value::Null,
                                other => Value::Text(other.to_string()),
                            };
                            test_cols.push(format!("{source_var}.{k}"));
                            test_vals.push(db_val);
                        }
                    }

                    // Parse JSON properties for target node
                    if let Ok(serde_json::Value::Object(map)) =
                        serde_json::from_str::<serde_json::Value>(&target_node.properties)
                    {
                        for (k, v) in map {
                            let db_val = match v {
                                serde_json::Value::Number(n) => {
                                    if let Some(i) = n.as_i64() {
                                        Value::Integer(i)
                                    } else if let Some(f) = n.as_f64() {
                                        Value::Real(f)
                                    } else {
                                        Value::Null
                                    }
                                }
                                serde_json::Value::String(s) => Value::Text(s),
                                serde_json::Value::Bool(b) => Value::Integer(if b { 1 } else { 0 }),
                                serde_json::Value::Null => Value::Null,
                                other => Value::Text(other.to_string()),
                            };
                            test_cols.push(format!("{target_var}.{k}"));
                            test_vals.push(db_val);
                        }
                    }

                    let test_row = Row::new(test_cols, test_vals);

                    if let Some(ref rw) = resolved_where
                        && !row_matches_resolved(&test_row, rw)
                    {
                        continue;
                    }

                    // Build projected row values
                    let mut row_vals = Vec::new();
                    for ret_col in &output_cols {
                        if ret_col == "*" {
                            row_vals.push(Value::Integer(source_node.id as i64));
                            row_vals.push(Value::Text(source_node.label.clone()));
                            row_vals.push(Value::Text(edge.label.clone()));
                            row_vals.push(Value::Integer(target_node.id as i64));
                            row_vals.push(Value::Text(target_node.label.clone()));
                        } else if ret_col == &source_var {
                            row_vals.push(Value::Text(format!(
                                "(:{} {{id: {}, properties: {}}})",
                                source_node.label, source_node.id, source_node.properties
                            )));
                        } else if ret_col == &target_var {
                            row_vals.push(Value::Text(format!(
                                "(:{} {{id: {}, properties: {}}})",
                                target_node.label, target_node.id, target_node.properties
                            )));
                        } else if rel_var.as_ref() == Some(ret_col) {
                            row_vals.push(Value::Text(format!(
                                "[:{} {{id: {}, weight: {}}}]",
                                edge.label, edge.id, edge.weight
                            )));
                        } else if let Some(val) = test_row.get_value(ret_col) {
                            row_vals.push(val.clone());
                        } else {
                            row_vals.push(Value::Null);
                        }
                    }

                    rows.push(Row::new(output_cols.clone(), row_vals));
                }

                Ok(rows)
            }

            Statement::GraphAlgorithm { algorithm, options } => match algorithm.as_str() {
                "PAGERANK" => {
                    let damping: f32 = options
                        .get("damping")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0.85);
                    let iterations: usize = options
                        .get("iterations")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(20);
                    let tol: f32 = options
                        .get("tolerance")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1e-4);
                    let ranks = self.graph.pagerank(damping, iterations, tol);

                    let cols = vec![
                        "node_id".to_string(),
                        "label".to_string(),
                        "pagerank".to_string(),
                    ];
                    let mut rows = Vec::new();
                    let mut sorted: Vec<(u64, f32)> = ranks.into_iter().collect();
                    sorted
                        .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

                    for (id, rank) in sorted {
                        let label = self
                            .graph
                            .get_node(id)
                            .map(|n| n.label.clone())
                            .unwrap_or_default();
                        rows.push(Row::new(
                            cols.clone(),
                            vec![
                                Value::Integer(id as i64),
                                Value::Text(label),
                                Value::Real(rank as f64),
                            ],
                        ));
                    }
                    Ok(rows)
                }
                "CONNECTED_COMPONENTS" | "COMPONENTS" | "WCC" => {
                    let components = self.graph.connected_components();
                    let cols = vec![
                        "node_id".to_string(),
                        "label".to_string(),
                        "component_id".to_string(),
                    ];
                    let mut rows = Vec::new();
                    let mut sorted: Vec<(u64, usize)> = components.into_iter().collect();
                    sorted.sort_by_key(|&(id, comp)| (comp, id));

                    for (id, comp) in sorted {
                        let label = self
                            .graph
                            .get_node(id)
                            .map(|n| n.label.clone())
                            .unwrap_or_default();
                        rows.push(Row::new(
                            cols.clone(),
                            vec![
                                Value::Integer(id as i64),
                                Value::Text(label),
                                Value::Integer(comp as i64),
                            ],
                        ));
                    }
                    Ok(rows)
                }
                "BETWEENNESS" | "BETWEENNESS_CENTRALITY" => {
                    let normalized = options
                        .get("normalized")
                        .map(|v| v != "false" && v != "0")
                        .unwrap_or(true);
                    let scores = self.graph.betweenness_centrality(normalized);
                    let cols = vec![
                        "node_id".to_string(),
                        "label".to_string(),
                        "betweenness".to_string(),
                    ];
                    let mut rows = Vec::new();
                    let mut sorted: Vec<(u64, f32)> = scores.into_iter().collect();
                    sorted
                        .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

                    for (id, score) in sorted {
                        let label = self
                            .graph
                            .get_node(id)
                            .map(|n| n.label.clone())
                            .unwrap_or_default();
                        rows.push(Row::new(
                            cols.clone(),
                            vec![
                                Value::Integer(id as i64),
                                Value::Text(label),
                                Value::Real(score as f64),
                            ],
                        ));
                    }
                    Ok(rows)
                }
                "LOUVAIN" | "COMMUNITIES" => {
                    let communities = self.graph.louvain_communities();
                    let cols = vec![
                        "node_id".to_string(),
                        "label".to_string(),
                        "community_id".to_string(),
                    ];
                    let mut rows = Vec::new();
                    let mut sorted: Vec<(u64, usize)> = communities.into_iter().collect();
                    sorted.sort_by_key(|&(id, comm)| (comm, id));

                    for (id, comm) in sorted {
                        let label = self
                            .graph
                            .get_node(id)
                            .map(|n| n.label.clone())
                            .unwrap_or_default();
                        rows.push(Row::new(
                            cols.clone(),
                            vec![
                                Value::Integer(id as i64),
                                Value::Text(label),
                                Value::Integer(comm as i64),
                            ],
                        ));
                    }
                    Ok(rows)
                }
                unknown => Err(Error::SqlSyntax(format!(
                    "Unknown graph algorithm '{unknown}'. Supported: PAGERANK, CONNECTED_COMPONENTS, BETWEENNESS, LOUVAIN"
                ))),
            },

            Statement::Pragma { key, value } => {
                let k = key.to_lowercase();
                if k == "table_info" {
                    let target_table = value.unwrap_or_default();
                    let cols = vec![
                        "cid".to_string(),
                        "name".to_string(),
                        "type".to_string(),
                        "notnull".to_string(),
                        "dflt_value".to_string(),
                        "pk".to_string(),
                    ];
                    let mut rows = Vec::new();
                    if let Some(tdef) = self.catalog.get_table(&target_table) {
                        for (cid, col) in tdef.columns.iter().enumerate() {
                            let dtype = format!("{:?}", col.data_type).to_uppercase();
                            let dflt = col
                                .default_value
                                .as_ref()
                                .map(|d| Value::Text(format!("{:?}", d.materialize())))
                                .unwrap_or(Value::Null);
                            let pk_num = if col.primary_key {
                                1
                            } else if let Some(pos) = tdef
                                .primary_key_columns
                                .iter()
                                .position(|p| p.eq_ignore_ascii_case(&col.name))
                            {
                                (pos + 1) as i64
                            } else {
                                0
                            };
                            rows.push(Row::new(
                                cols.clone(),
                                vec![
                                    Value::Integer(cid as i64),
                                    Value::Text(col.name.clone()),
                                    Value::Text(dtype),
                                    Value::Integer(if col.not_null { 1 } else { 0 }),
                                    dflt,
                                    Value::Integer(pk_num),
                                ],
                            ));
                        }
                    }
                    Ok(rows)
                } else if k == "foreign_keys" {
                    if let Some(v) = value {
                        let val_lower = v.trim().to_lowercase();
                        self.foreign_keys_enabled =
                            val_lower == "1" || val_lower == "true" || val_lower == "on";
                    }
                    Ok(vec![Row::new(
                        vec!["foreign_keys".to_string()],
                        vec![Value::Integer(if self.foreign_keys_enabled {
                            1
                        } else {
                            0
                        })],
                    )])
                } else {
                    Ok(Vec::new())
                }
            }

            other => Err(Error::SqlSyntax(format!(
                "Non-query statement {other:?} passed to query()"
            ))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fetch_rows_by_row_ids(
        &self,
        pager: &mut Pager,
        root_page: u32,
        table_def: &TableDef,
        all_col_shared: &Arc<Vec<String>>,
        mut row_ids: Vec<u64>,
        resolved_where: Option<&ResolvedWhereExpr>,
        scan_mask: Option<&[bool]>,
        mode: IndexFetchMode,
    ) -> Result<Vec<Row>> {
        if row_ids.len() > 1 {
            row_ids.sort_unstable();
            row_ids.dedup();
        }
        let mut scratch_vals = Vec::with_capacity(all_col_shared.len());
        let mut collected = Vec::with_capacity(row_ids.len());
        for rid in row_ids {
            let Some(payload) = self.btree.search(pager, root_page, rid)? else {
                continue;
            };
            let row = if let Some(mask) = scan_mask {
                decode_row_values_into_projected(&payload, mask, &mut scratch_vals)?;
                if let Some(r_expr) = resolved_where
                    && !slice_matches_resolved(&scratch_vals, table_def, r_expr)
                {
                    continue;
                }
                Row::with_shared_columns(all_col_shared.clone(), std::mem::take(&mut scratch_vals))
            } else {
                let full_row = decode_row_shared(&payload, all_col_shared)?;
                if let Some(r_expr) = resolved_where
                    && !row_matches_resolved(&full_row, r_expr)
                {
                    continue;
                }
                full_row
            };

            match &mode {
                IndexFetchMode::CollectAll => collected.push(row),
                IndexFetchMode::EarlyReturn { .. } => collected.push(row),
            }
        }

        match mode {
            IndexFetchMode::CollectAll => Ok(collected),
            IndexFetchMode::EarlyReturn {
                output_col_names,
                distinct,
                offset,
                limit,
            } => {
                let mut result_rows = Vec::with_capacity(collected.len());
                let skip_count = offset.unwrap_or(0);
                let mut skipped = 0;
                let mut distinct_set = RowDistinctSet::new();
                for row in collected {
                    let projected = self.project_row(&row, &output_col_names)?;
                    if distinct {
                        if distinct_set.push_paginated(
                            &mut result_rows,
                            projected,
                            skip_count,
                            limit,
                        ) {
                            break;
                        }
                        continue;
                    }
                    if skipped < skip_count {
                        skipped += 1;
                        continue;
                    }
                    result_rows.push(projected);
                    if let Some(lim) = limit
                        && result_rows.len() >= lim
                    {
                        break;
                    }
                }
                Ok(result_rows)
            }
        }
    }
}

enum IndexFetchMode {
    EarlyReturn {
        output_col_names: Vec<String>,
        distinct: bool,
        offset: Option<usize>,
        limit: Option<usize>,
    },
    CollectAll,
}

fn compute_single_table_needed_mask(
    table_def: &TableDef,
    columns: &[String],
    resolved_where: Option<&ResolvedWhereExpr>,
    order_by: &[crate::sql::parser::OrderByClause],
    wants_fts_scores: bool,
) -> Option<Vec<bool>> {
    if columns.is_empty() || wants_fts_scores {
        return None;
    }
    let mut mask = vec![false; table_def.columns.len()];
    for col in columns {
        let (expr, _alias) = parse_col_and_alias(col);
        if !expr.contains('(') && !expr.contains(' ') {
            if let Some(idx) = resolve_direct_table_col(table_def, expr)
                && idx < mask.len()
            {
                mask[idx] = true;
            } else if (expr.starts_with('\'') && expr.ends_with('\''))
                || expr.parse::<f64>().is_ok()
                || expr.eq_ignore_ascii_case("NULL")
            {
                continue;
            } else {
                return None;
            }
        } else {
            let spec = parse_aggregate_spec(col)?;
            if !spec.is_star {
                let idx = resolve_direct_table_col(table_def, &spec.arg)?;
                if idx < mask.len() {
                    mask[idx] = true;
                }
            }
        }
    }

    if let Some(w) = resolved_where
        && !mark_resolved_where_columns(w, table_def, &mut mask)
    {
        return None;
    }

    for ord in order_by {
        if let Some(idx) = resolve_direct_table_col(table_def, &ord.column)
            && idx < mask.len()
        {
            mask[idx] = true;
        } else {
            let shadowed_alias = columns.iter().any(|c| {
                parse_col_and_alias(c)
                    .1
                    .is_some_and(|a| a.eq_ignore_ascii_case(&ord.column))
            });
            if !shadowed_alias {
                return None;
            }
        }
    }

    Some(mask)
}

fn resolve_accumulated_col(
    left_cols: &[String],
    default_table: Option<&str>,
    spec: &str,
) -> Option<usize> {
    if let Some((tbl, col)) = spec.split_once('.') {
        if let Some(pos) = left_cols.iter().position(|c| c.eq_ignore_ascii_case(spec)) {
            return Some(pos);
        }
        if let Some(def_tbl) = default_table
            && def_tbl.eq_ignore_ascii_case(tbl)
        {
            return left_cols.iter().position(|c| c.eq_ignore_ascii_case(col));
        }
        None
    } else {
        left_cols.iter().position(|c| col_name_matches(c, spec))
    }
}

fn resolve_join_key_indices(
    left_cols: &[String],
    default_left_table: Option<&str>,
    right_table_def: &TableDef,
    left_col_spec: &str,
    right_col_spec: &str,
) -> (Option<usize>, Option<usize>) {
    let direct_l = resolve_accumulated_col(left_cols, default_left_table, left_col_spec);
    let direct_r = resolve_direct_table_col(right_table_def, right_col_spec);
    if direct_l.is_some() && direct_r.is_some() {
        return (direct_l, direct_r);
    }
    let flipped_l = resolve_accumulated_col(left_cols, default_left_table, right_col_spec);
    let flipped_r = resolve_direct_table_col(right_table_def, left_col_spec);
    if flipped_l.is_some() && flipped_r.is_some() {
        return (flipped_l, flipped_r);
    }
    (direct_l, direct_r)
}

#[inline]
fn eval_projected_where(
    vals: &[Value],
    cols_shared: &Arc<Vec<String>>,
    table_def: &TableDef,
    where_opt: Option<&ResolvedWhereExpr>,
    can_slice: bool,
) -> bool {
    let Some(w) = where_opt else {
        return true;
    };
    if can_slice {
        slice_matches_resolved(vals, table_def, w)
    } else {
        let row = Row::with_shared_columns(cols_shared.clone(), vals.to_vec());
        row_matches_resolved(&row, w)
    }
}

struct TopKVecEntry {
    dist: f32,
    seq: usize,
    row: Row,
}

impl PartialEq for TopKVecEntry {
    fn eq(&self, other: &Self) -> bool {
        self.dist.to_bits() == other.dist.to_bits() && self.seq == other.seq
    }
}

impl Eq for TopKVecEntry {}

impl PartialOrd for TopKVecEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TopKVecEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let dc = match (self.dist.is_nan(), other.dist.is_nan()) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) => self.dist.total_cmp(&other.dist),
        };
        dc.then_with(|| self.seq.cmp(&other.seq))
    }
}

#[inline]
fn push_top_k_heap<F>(
    heap: &mut std::collections::BinaryHeap<TopKVecEntry>,
    top_k: usize,
    dist: f32,
    seq: &mut usize,
    build_row: F,
) where
    F: FnOnce() -> Row,
{
    let cur_seq = *seq;
    *seq = cur_seq.saturating_add(1);
    if heap.len() < top_k {
        heap.push(TopKVecEntry {
            dist,
            seq: cur_seq,
            row: build_row(),
        });
    } else if let Some(worst) = heap.peek() {
        let better = if worst.dist.is_nan() {
            !dist.is_nan()
        } else if dist.is_nan() {
            false
        } else {
            dist < worst.dist
        };
        if better {
            heap.pop();
            heap.push(TopKVecEntry {
                dist,
                seq: cur_seq,
                row: build_row(),
            });
        }
    }
}

pub(crate) fn collect_or_branches<'a>(expr: &'a WhereExpr, out: &mut Vec<&'a WhereExpr>) {
    if let WhereExpr::Or(l, r) = expr {
        collect_or_branches(l, out);
        collect_or_branches(r, out);
    } else {
        out.push(expr);
    }
}

impl super::SQLExecutor {
    pub(crate) fn plan_or_index_union_labels(
        &self,
        table: &str,
        table_def: &TableDef,
        expr: &WhereExpr,
    ) -> Option<Vec<String>> {
        if !matches!(expr, WhereExpr::Or(_, _)) {
            return None;
        }
        let mut branches = Vec::new();
        collect_or_branches(expr, &mut branches);
        if branches.len() < 2 || branches.len() > 32 {
            return None;
        }
        let pk_idx = table_def.primary_key_index();
        let is_int_pk = pk_idx.is_some_and(|idx| {
            table_def.columns[idx].data_type == crate::sql::catalog::DataType::Integer
        });
        let pk_name = pk_idx.map(|i| table_def.columns[i].name.as_str());
        let mut labels = Vec::with_capacity(branches.len());
        for b in branches {
            if is_int_pk
                && let Some(pk) = pk_name
                && let (Some(lo), Some(hi), true) = extract_pk_signed_range(b, pk)
                && hi.saturating_sub(lo) <= 64
            {
                labels.push(format!("PRIMARY KEY ({pk})"));
                continue;
            }
            let mut eqs = HashMap::new();
            collect_and_eq_conditions(b, &mut eqs);
            if eqs.len() >= 2
                && let Some(comp_idx) = self
                    .catalog
                    .indexes()
                    .into_iter()
                    .filter(|idx| {
                        idx.table.eq_ignore_ascii_case(table)
                            && partial_index_matches_query(idx, Some(b))
                    })
                    .filter(|idx| {
                        let cols = idx.target_columns();
                        cols.len() >= 2 && cols.iter().all(|c| eqs.contains_key(&c.to_lowercase()))
                    })
                    .max_by_key(|idx| idx.target_columns().len())
            {
                labels.push(format!("COMPOSITE INDEX {}", comp_idx.name));
                continue;
            }
            if let Some(idx_def) = eqs
                .keys()
                .find_map(|c| self.find_usable_index_for_column(table, c, Some(b)))
            {
                labels.push(format!("INDEX {}", idx_def.name));
                continue;
            }
            if let WhereExpr::InList {
                column,
                values,
                negated: false,
            } = b
            {
                let bare = column.split_once('.').map_or(column.as_str(), |(_, c)| c);
                if is_int_pk
                    && pk_name.is_some_and(|pk| pk.eq_ignore_ascii_case(bare))
                    && values.len() <= 64
                {
                    labels.push(format!("PRIMARY KEY ({bare})"));
                    continue;
                }
                if values.len() <= 16
                    && let Some(idx_def) = self.find_usable_index_for_column(table, bare, Some(b))
                {
                    labels.push(format!("INDEX {}", idx_def.name));
                    continue;
                }
            }
            return None;
        }
        Some(labels)
    }

    pub(crate) fn try_collect_or_union_row_ids(
        &self,
        pager: &mut Pager,
        table: &str,
        table_def: &TableDef,
        expr: &WhereExpr,
        stats: &crate::sql::planner::TableStats,
    ) -> Result<Option<Vec<u64>>> {
        let mut branches = Vec::new();
        collect_or_branches(expr, &mut branches);
        if branches.len() < 2 || branches.len() > 32 {
            return Ok(None);
        }
        let pk_idx = table_def.primary_key_index();
        let is_int_pk = pk_idx.is_some_and(|idx| {
            table_def.columns[idx].data_type == crate::sql::catalog::DataType::Integer
        });
        let pk_name = pk_idx.map(|i| table_def.columns[i].name.as_str());
        let mut union_rids = Vec::new();
        let max_candidates = if stats.total_rows > 128 {
            (stats.total_rows * 2) / 3
        } else {
            usize::MAX
        };

        for b in branches {
            if is_int_pk
                && let Some(pk) = pk_name
                && let (Some(lo), Some(hi), true) = extract_pk_signed_range(b, pk)
                && hi.saturating_sub(lo) <= 64
            {
                if lo <= hi {
                    for k in lo..=hi {
                        union_rids.push(k as u64);
                    }
                }
                if union_rids.len() > max_candidates {
                    return Ok(None);
                }
                continue;
            }

            let mut eqs = HashMap::new();
            collect_and_eq_conditions(b, &mut eqs);

            if eqs.len() >= 2
                && let Some(comp_idx) = self
                    .catalog
                    .indexes()
                    .into_iter()
                    .filter(|idx| {
                        idx.table.eq_ignore_ascii_case(table)
                            && partial_index_matches_query(idx, Some(b))
                    })
                    .filter(|idx| {
                        let cols = idx.target_columns();
                        cols.len() >= 2 && cols.iter().all(|c| eqs.contains_key(&c.to_lowercase()))
                    })
                    .max_by_key(|idx| idx.target_columns().len())
            {
                let tcols = comp_idx.target_columns();
                let dummy_cols: Vec<String> = tcols.iter().map(|c| (*c).to_string()).collect();
                let dummy_vals: Vec<Value> = tcols
                    .iter()
                    .map(|c| eqs.get(&c.to_lowercase()).cloned().unwrap_or(Value::Null))
                    .collect();
                let dummy_row = Row::new(dummy_cols, dummy_vals);
                if let Some((comp_val, _)) = extract_index_key_value(&dummy_row, &tcols) {
                    let key = value_to_index_key(&comp_val);
                    if let Some(bucket) =
                        IndexBucket::load(&self.btree, pager, comp_idx.root_page, key)?
                        && let Some(rids) = bucket.get_row_ids(&comp_val)
                    {
                        union_rids.extend_from_slice(rids);
                    }
                }
                if union_rids.len() > max_candidates {
                    return Ok(None);
                }
                continue;
            }

            if let Some((cond_val, idx_def)) = eqs.iter().find_map(|(c, v)| {
                self.find_usable_index_for_column(table, c, Some(b))
                    .map(|idx| (v, idx))
            }) {
                let key = value_to_index_key(cond_val);
                if let Some(bucket) = IndexBucket::load(&self.btree, pager, idx_def.root_page, key)?
                    && let Some(rids) = bucket.get_row_ids(cond_val)
                {
                    union_rids.extend_from_slice(rids);
                }
                if union_rids.len() > max_candidates {
                    return Ok(None);
                }
                continue;
            }

            if let WhereExpr::InList {
                column,
                values,
                negated: false,
            } = b
            {
                let bare = column.split_once('.').map_or(column.as_str(), |(_, c)| c);
                if is_int_pk
                    && pk_name.is_some_and(|pk| pk.eq_ignore_ascii_case(bare))
                    && values.len() <= 64
                {
                    for v in values {
                        if let Some(k) = match v {
                            Value::Integer(i) => Some(*i),
                            Value::Real(r)
                                if r.fract() == 0.0
                                    && *r >= (i64::MIN as f64)
                                    && *r < 9223372036854775808.0 =>
                            {
                                Some(*r as i64)
                            }
                            _ => None,
                        } {
                            union_rids.push(k as u64);
                        }
                    }
                    if union_rids.len() > max_candidates {
                        return Ok(None);
                    }
                    continue;
                }
                if values.len() <= 16
                    && let Some(idx_def) = self.find_usable_index_for_column(table, bare, Some(b))
                {
                    for val in values {
                        if val.is_null() {
                            continue;
                        }
                        let key = value_to_index_key(val);
                        if let Some(bucket) =
                            IndexBucket::load(&self.btree, pager, idx_def.root_page, key)?
                            && let Some(rids) = bucket.get_row_ids(val)
                        {
                            union_rids.extend_from_slice(rids);
                        }
                    }
                    if union_rids.len() > max_candidates {
                        return Ok(None);
                    }
                    continue;
                }
            }

            return Ok(None);
        }

        union_rids.sort_unstable();
        union_rids.dedup();
        Ok(Some(union_rids))
    }
}
