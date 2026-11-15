#[allow(unused_imports)]
use super::*;

impl SQLExecutor {
    /// Generate an EXPLAIN / EXPLAIN QUERY PLAN representation
    pub(super) fn explain_query_plan(
        &self,
        _pager: &mut Pager,
        stmt: Statement,
        _is_query_plan: bool,
    ) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        let cols = vec!["id".to_string(), "parent".to_string(), "detail".to_string()];
        match stmt {
            Statement::Select {
                table,
                join,
                extra_joins,
                where_clause,
                order_by,
                limit,
                ..
            } => {
                let has_joins = join.is_some() || !extra_joins.is_empty();
                let table_def = self.catalog.get_table(&table);
                let pk_idx = table_def.and_then(|t| t.primary_key_index());
                let mut id = 0;

                let optimizer = crate::sql::planner::CostOptimizer::new();
                let default_stats = crate::sql::planner::TableStats::new(&table, 100, 10);
                let stats = self
                    .catalog
                    .get_table_stats(&table)
                    .unwrap_or(&default_stats);

                // Check if fast point lookup by PK is used (including compound AND predicates)
                let mut is_pk_search = false;
                if !has_joins
                    && let Some(tdef) = table_def
                    && let Some(pk_col_idx) = pk_idx
                    && tdef.columns[pk_col_idx].data_type == crate::sql::catalog::DataType::Integer
                    && let Some(ref w_expr) = where_clause
                {
                    let pk_name = &tdef.columns[pk_col_idx].name;
                    if let (Some(lo), Some(hi), true) = extract_pk_signed_range(w_expr, pk_name)
                        && lo == hi
                    {
                        is_pk_search = true;
                        let cost = optimizer.estimate_pk_lookup(stats);
                        rows.push(Row::new(
                            cols.clone(),
                            vec![
                                Value::Integer(id),
                                Value::Integer(0),
                                Value::Text(format!(
                                    "SEARCH TABLE {} USING PRIMARY KEY ({} = {}) {}",
                                    table, pk_name, lo, cost
                                )),
                            ],
                        ));
                        id += 1;
                    }
                }

                let mut is_index_search = false;
                if !is_pk_search && !has_joins {
                    if order_by.is_none()
                        && let Some(ref expr) = where_clause
                    {
                        let mut and_eqs = HashMap::new();
                        collect_and_eq_conditions(expr, &mut and_eqs);
                        if and_eqs.len() >= 2
                            && let Some(idx_def) = self
                                .catalog
                                .indexes()
                                .into_iter()
                                .filter(|idx| {
                                    idx.table.eq_ignore_ascii_case(&table)
                                        && partial_index_matches_query(idx, where_clause.as_ref())
                                })
                                .filter(|idx| {
                                    let cols = idx.target_columns();
                                    cols.len() >= 2
                                        && cols
                                            .iter()
                                            .all(|c| and_eqs.contains_key(&c.to_lowercase()))
                                })
                                .max_by_key(|idx| idx.target_columns().len())
                        {
                            is_index_search = true;
                            let tcols = idx_def.target_columns().join(", ");
                            rows.push(Row::new(
                                cols.clone(),
                                vec![
                                    Value::Integer(id),
                                    Value::Integer(0),
                                    Value::Text(format!(
                                        "SEARCH TABLE {} USING COMPOSITE INDEX {} ({})",
                                        table, idx_def.name, tcols
                                    )),
                                ],
                            ));
                            id += 1;
                        }
                    }
                    if !is_index_search && let Some(ref w_expr) = where_clause {
                        let mut eq_conds = HashMap::new();
                        collect_and_eq_conditions(w_expr, &mut eq_conds);
                        if let Some((col_key, col_val, idx_def)) =
                            eq_conds.iter().find_map(|(c, v)| {
                                self.find_usable_index_for_column(&table, c, Some(w_expr))
                                    .map(|idx| (c.clone(), v.clone(), idx))
                            })
                        {
                            is_index_search = true;
                            let sel = stats.estimate_selectivity(&col_key, true);
                            let cost =
                                optimizer.estimate_index_scan(stats, &idx_def.name, &col_key, sel);
                            let idx_kind = if idx_def.predicate_sql.is_some() {
                                "PARTIAL INDEX"
                            } else {
                                "INDEX"
                            };
                            rows.push(Row::new(
                                cols.clone(),
                                vec![
                                    Value::Integer(id),
                                    Value::Integer(0),
                                    Value::Text(format!(
                                        "SEARCH TABLE {} USING {} {} ({} = {:?}) {}",
                                        table, idx_kind, idx_def.name, col_key, col_val, cost
                                    )),
                                ],
                            ));
                            id += 1;
                        }
                    }
                    if !is_index_search && let Some(ref w_expr) = where_clause {
                        let mut in_cols = Vec::new();
                        collect_top_level_in_cols(w_expr, &mut in_cols);
                        if let Some(tdef) = table_def
                            && let Some(pk_col_idx) = pk_idx
                            && tdef.columns[pk_col_idx].data_type
                                == crate::sql::catalog::DataType::Integer
                        {
                            let pk_name = &tdef.columns[pk_col_idx].name;
                            if in_cols.iter().any(|c| {
                                let bare = c.split_once('.').map_or(*c, |(_, col)| col);
                                bare.eq_ignore_ascii_case(pk_name)
                            }) {
                                is_pk_search = true;
                                let cost = optimizer.estimate_pk_lookup(stats);
                                rows.push(Row::new(
                                    cols.clone(),
                                    vec![
                                        Value::Integer(id),
                                        Value::Integer(0),
                                        Value::Text(format!(
                                            "SEARCH TABLE {} USING PRIMARY KEY ({} IN (...)) {}",
                                            table, pk_name, cost
                                        )),
                                    ],
                                ));
                                id += 1;
                            }
                        }
                        if !is_pk_search {
                            for col_spec in in_cols {
                                let bare = col_spec.split_once('.').map_or(col_spec, |(_, c)| c);
                                if let Some(idx_def) =
                                    self.find_usable_index_for_column(&table, bare, Some(w_expr))
                                {
                                    is_index_search = true;
                                    let sel = stats.estimate_selectivity(bare, true);
                                    let cost = optimizer.estimate_index_scan(
                                        stats,
                                        &idx_def.name,
                                        bare,
                                        sel,
                                    );
                                    rows.push(Row::new(
                                        cols.clone(),
                                        vec![
                                            Value::Integer(id),
                                            Value::Integer(0),
                                            Value::Text(format!(
                                                "SEARCH TABLE {} USING INDEX {} ({} IN (...)) {}",
                                                table, idx_def.name, bare, cost
                                            )),
                                        ],
                                    ));
                                    id += 1;
                                    break;
                                }
                            }
                        }
                        if !is_pk_search
                            && !is_index_search
                            && let Some(tdef) = table_def
                            && let Some(labels) =
                                self.plan_or_index_union_labels(&table, tdef, w_expr)
                        {
                            is_index_search = true;
                            rows.push(Row::new(
                                cols.clone(),
                                vec![
                                    Value::Integer(id),
                                    Value::Integer(0),
                                    Value::Text(format!(
                                        "SEARCH TABLE {} USING INDEX UNION ({})",
                                        table,
                                        labels.join(" OR ")
                                    )),
                                ],
                            ));
                            id += 1;
                        }
                    }
                }

                let mut pk_order_pushdown = false;
                if !is_pk_search && !is_index_search {
                    let is_int_pk = pk_idx.is_some_and(|idx| {
                        table_def.is_some_and(|t| {
                            t.columns[idx].data_type == crate::sql::catalog::DataType::Integer
                        })
                    });
                    let pk_col_name =
                        pk_idx.and_then(|idx| table_def.map(|t| t.columns[idx].name.as_str()));
                    let has_pk_range = is_int_pk
                        && !has_joins
                        && pk_col_name.is_some_and(|pk_name| {
                            where_clause
                                .as_ref()
                                .is_some_and(|w| extract_pk_signed_range(w, pk_name).2)
                        });
                    pk_order_pushdown = is_int_pk
                        && !has_joins
                        && pk_col_name.is_some_and(|pk_name| {
                            order_by.as_ref().is_some_and(|ord| {
                                let ord_col = ord
                                    .column
                                    .split_once('.')
                                    .and_then(|(tbl, c)| {
                                        tbl.eq_ignore_ascii_case(&table).then_some(c)
                                    })
                                    .unwrap_or(ord.column.as_str());
                                pk_name.eq_ignore_ascii_case(ord_col)
                            })
                        });

                    if has_pk_range || pk_order_pushdown {
                        let scan_kind = if has_pk_range {
                            "RANGE SCAN TABLE"
                        } else {
                            "SCAN TABLE"
                        };
                        let order_note = if pk_order_pushdown {
                            let dir = order_by
                                .as_ref()
                                .map_or("ASC", |o| if o.is_ascending { "ASC" } else { "DESC" });
                            format!(" ORDER ({dir})")
                        } else {
                            String::new()
                        };
                        let cost = optimizer.estimate_seq_scan(stats, 0.2);
                        rows.push(Row::new(
                            cols.clone(),
                            vec![
                                Value::Integer(id),
                                Value::Integer(0),
                                Value::Text(format!(
                                    "{scan_kind} {table} USING PRIMARY KEY{order_note} {cost}"
                                )),
                            ],
                        ));
                    } else {
                        let filter_str = if let Some(ref expr) = where_clause {
                            format!(" WITH FILTER ({expr:?})")
                        } else {
                            String::new()
                        };
                        let cost = optimizer.estimate_seq_scan(stats, 1.0);
                        rows.push(Row::new(
                            cols.clone(),
                            vec![
                                Value::Integer(id),
                                Value::Integer(0),
                                Value::Text(format!("SCAN TABLE {}{filter_str} {}", table, cost)),
                            ],
                        ));
                    }
                    id += 1;
                }

                for j in join.iter().chain(extra_joins.iter()) {
                    rows.push(Row::new(
                        cols.clone(),
                        vec![
                            Value::Integer(id),
                            Value::Integer(0),
                            Value::Text(format!(
                                "SEARCH SUBQUERY / JOIN TABLE {} ON {} = {}",
                                j.table, j.left_col, j.right_col
                            )),
                        ],
                    ));
                    id += 1;
                }

                if let Some(ord) = order_by
                    && !pk_order_pushdown
                {
                    let dir = if ord.is_ascending { "ASC" } else { "DESC" };
                    rows.push(Row::new(
                        cols.clone(),
                        vec![
                            Value::Integer(id),
                            Value::Integer(0),
                            Value::Text(format!(
                                "USE TEMP B-TREE FOR ORDER BY {} {}",
                                ord.column, dir
                            )),
                        ],
                    ));
                    id += 1;
                }

                if let Some(lim) = limit {
                    rows.push(Row::new(
                        cols.clone(),
                        vec![
                            Value::Integer(id),
                            Value::Integer(0),
                            Value::Text(format!("LIMIT {}", lim)),
                        ],
                    ));
                }
            }
            Statement::VectorSearch {
                table,
                vector_col,
                top_k,
                query_vector,
                ..
            } => {
                let optimizer = crate::sql::planner::CostOptimizer::new();
                let default_stats = crate::sql::planner::TableStats::new(&table, 100, 10);
                let stats = self
                    .catalog
                    .get_table_stats(&table)
                    .unwrap_or(&default_stats);
                let has_hnsw = self.vector_indexes.contains_key(&table.to_lowercase());
                let cost =
                    optimizer.estimate_vector_search(stats, top_k, query_vector.len(), has_hnsw);
                rows.push(Row::new(
                    cols.clone(),
                    vec![
                        Value::Integer(0),
                        Value::Integer(0),
                        Value::Text(format!(
                            "SEARCH TABLE {} USING {} ON {} (TOP {}) {}",
                            table,
                            if has_hnsw {
                                "HNSW VECTOR INDEX"
                            } else {
                                "BRUTE-FORCE SCAN"
                            },
                            vector_col,
                            top_k,
                            cost
                        )),
                    ],
                ));
            }
            _ => {
                rows.push(Row::new(
                    cols.clone(),
                    vec![
                        Value::Integer(0),
                        Value::Integer(0),
                        Value::Text("EXECUTE STATEMENT (DIRECT B-TREE ENGINE)".to_string()),
                    ],
                ));
            }
        }
        Ok(rows)
    }

    /// Execute a query with Common Table Expressions (WITH [RECURSIVE] cte AS (...) SELECT ...)
    pub(super) fn query_with_cte(
        &mut self,
        pager: &mut Pager,
        ctes: Vec<crate::sql::parser::CteClause>,
        main_query: Statement,
    ) -> Result<Vec<Row>> {
        let mut cte_store: HashMap<String, Vec<Row>> = HashMap::new();

        for cte in ctes {
            let cte_key = cte.name.to_ascii_lowercase();
            let mapped_rows = if cte.recursive
                && let Statement::Compound { left, op, right } = *cte.query
            {
                self.execute_recursive_cte(
                    pager,
                    &cte_key,
                    cte.columns.as_deref(),
                    *left,
                    op,
                    *right,
                    &cte_store,
                )?
            } else {
                let rows = self.eval_cte_statement(pager, *cte.query, &cte_store)?;
                apply_cte_column_names(rows, cte.columns.as_deref())
            };

            cte_store.insert(cte_key, mapped_rows);
        }

        // Execute main query against ephemeral CTE store
        self.eval_cte_statement(pager, main_query, &cte_store)
    }

    pub(super) fn eval_cte_statement(
        &mut self,
        pager: &mut Pager,
        stmt: Statement,
        cte_store: &HashMap<String, Vec<Row>>,
    ) -> Result<Vec<Row>> {
        match stmt {
            Statement::Compound { left, op, right } => {
                let mut left_rows = self.eval_cte_statement(pager, *left, cte_store)?;
                let right_rows = self.eval_cte_statement(pager, *right, cte_store)?;
                combine_compound_rows(&mut left_rows, right_rows, op);
                Ok(left_rows)
            }
            Statement::Select {
                ref table,
                ref join,
                ref extra_joins,
                ..
            } if cte_store.contains_key(&table.to_ascii_lowercase())
                || join
                    .iter()
                    .chain(extra_joins.iter())
                    .any(|j| cte_store.contains_key(&j.table.to_ascii_lowercase())) =>
            {
                self.query_on_ephemeral_rows(pager, stmt, cte_store)
            }
            other => self.query(pager, other),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn execute_recursive_cte(
        &mut self,
        pager: &mut Pager,
        cte_key: &str,
        explicit_cols: Option<&[String]>,
        anchor: Statement,
        op: crate::sql::parser::SetOperation,
        recursive_step: Statement,
        outer_store: &HashMap<String, Vec<Row>>,
    ) -> Result<Vec<Row>> {
        const MAX_RECURSIVE_ITERATIONS: usize = 10_000;
        let anchor_rows = apply_cte_column_names(
            self.eval_cte_statement(pager, anchor, outer_store)?,
            explicit_cols,
        );
        let mut accumulated = Vec::new();
        let mut seen: Option<HashSet<String>> =
            (op == crate::sql::parser::SetOperation::Union).then(HashSet::new);
        let mut working = Vec::with_capacity(anchor_rows.len());
        for row in anchor_rows {
            if let Some(ref mut set) = seen
                && !set.insert(format!("{:?}", row.values()))
            {
                continue;
            }
            accumulated.push(row.clone());
            working.push(row);
        }

        let mut step_store = outer_store.clone();
        let mut iterations = 0usize;
        while !working.is_empty() {
            iterations += 1;
            if iterations > MAX_RECURSIVE_ITERATIONS {
                return Err(Error::SqlSyntax(format!(
                    "WITH RECURSIVE '{cte_key}' exceeded maximum recursion depth ({MAX_RECURSIVE_ITERATIONS})"
                )));
            }
            step_store.insert(cte_key.to_string(), working);
            let raw_next = self.eval_cte_statement(pager, recursive_step.clone(), &step_store)?;
            let next_mapped = apply_cte_column_names(raw_next, explicit_cols);
            let mut next_working = Vec::new();
            for row in next_mapped {
                if let Some(ref mut set) = seen
                    && !set.insert(format!("{:?}", row.values()))
                {
                    continue;
                }
                accumulated.push(row.clone());
                next_working.push(row);
            }
            working = next_working;
        }
        Ok(accumulated)
    }

    /// Evaluate a SELECT statement against in-memory ephemeral CTE tables
    pub(super) fn query_on_ephemeral_rows(
        &mut self,
        pager: &mut Pager,
        stmt: Statement,
        cte_store: &HashMap<String, Vec<Row>>,
    ) -> Result<Vec<Row>> {
        if let Statement::Select {
            distinct,
            columns,
            table,
            join,
            extra_joins,
            where_clause,
            group_by,
            having,
            order_by,
            limit,
            offset,
            ..
        } = stmt
        {
            let base_rows = if table.is_empty() {
                vec![Row::new(Vec::new(), Vec::new())]
            } else if let Some(cte_rows) = cte_store.get(&table.to_ascii_lowercase()) {
                cte_rows.clone()
            } else if self.catalog.table_exists(&table) {
                self.query(
                    pager,
                    Statement::Select {
                        distinct: false,
                        columns: Vec::new(),
                        table: table.clone(),
                        from_subquery: None,
                        join: None,
                        extra_joins: Vec::new(),
                        where_clause: None,
                        group_by: None,
                        having: None,
                        order_by: None,
                        limit: None,
                        offset: None,
                        as_of_timestamp: None,
                    },
                )?
            } else {
                return Err(Error::TableNotFound(table.clone()));
            };

            let mut rows = base_rows;

            // Handle chained JOINs
            for j in join.iter().chain(extra_joins.iter()) {
                let right_rows = if let Some(r_cte) = cte_store.get(&j.table.to_ascii_lowercase()) {
                    r_cte.clone()
                } else if self.catalog.table_exists(&j.table) {
                    self.query(
                        pager,
                        Statement::Select {
                            distinct: false,
                            columns: vec![],
                            table: j.table.clone(),
                            from_subquery: None,
                            join: None,
                            extra_joins: Vec::new(),
                            where_clause: None,
                            group_by: None,
                            having: None,
                            order_by: None,
                            limit: None,
                            offset: None,
                            as_of_timestamp: None,
                        },
                    )?
                } else {
                    return Err(Error::TableNotFound(j.table.clone()));
                };

                let mut joined_rows = Vec::new();
                for left_r in &rows {
                    let mut matched = false;
                    for right_r in &right_rows {
                        let (left_v, right_v) = if left_r.get_value(&j.left_col).is_some()
                            && right_r.get_value(&j.right_col).is_some()
                        {
                            (
                                left_r.get_value(&j.left_col),
                                right_r.get_value(&j.right_col),
                            )
                        } else {
                            (
                                left_r.get_value(&j.right_col),
                                right_r.get_value(&j.left_col),
                            )
                        };
                        if let (Some(lv), Some(rv)) = (left_v, right_v)
                            && lv == rv
                            && !lv.is_null()
                        {
                            matched = true;
                            let mut j_cols = left_r.columns().to_vec();
                            let mut j_vals = left_r.values().to_vec();
                            for (rc, rv) in right_r.columns().iter().zip(right_r.values().iter()) {
                                j_cols.push(format!("{}.{}", j.table, rc));
                                j_vals.push(rv.clone());
                                if !j_cols.contains(rc) {
                                    j_cols.push(rc.clone());
                                    j_vals.push(rv.clone());
                                }
                            }
                            joined_rows.push(Row::new(j_cols, j_vals));
                        }
                    }
                    if !matched && j.is_left {
                        let mut j_cols = left_r.columns().to_vec();
                        let mut j_vals = left_r.values().to_vec();
                        if let Some(first_r) = right_rows.first() {
                            for rc in first_r.columns() {
                                j_cols.push(format!("{}.{}", j.table, rc));
                                j_vals.push(Value::Null);
                                if !j_cols.contains(rc) {
                                    j_cols.push(rc.clone());
                                    j_vals.push(Value::Null);
                                }
                            }
                        }
                        joined_rows.push(Row::new(j_cols, j_vals));
                    }
                }
                rows = joined_rows;
            }

            // Apply WHERE clause (supporting correlated subqueries against CTE/ephemeral outer columns)
            if let Some(ref expr) = where_clause {
                if let Some(first_row) = rows.first() {
                    let pseudo_cols: Vec<crate::sql::catalog::ColumnDef> = first_row
                        .columns()
                        .iter()
                        .map(|name| {
                            crate::sql::catalog::ColumnDef::new(
                                name.clone(),
                                crate::sql::catalog::DataType::Text,
                            )
                        })
                        .collect();
                    let pseudo_td =
                        crate::sql::catalog::TableDef::new(table.clone(), 0, pseudo_cols);
                    let resolved =
                        self.resolve_where_expr_for_tables(pager, expr, &[&pseudo_td])?;
                    self.retain_rows_with_correlated_where(
                        pager,
                        &mut rows,
                        &resolved,
                        &[&pseudo_td],
                    )?;
                } else {
                    let _ = self.resolve_where_expr(pager, expr)?;
                }
            }

            // Apply GROUP BY / Aggregates / Projection
            if let Some(ref gb_cols) = group_by {
                let resolved_having = if let Some(ref h) = having {
                    Some(self.resolve_where_expr(pager, h)?)
                } else {
                    None
                };
                rows = aggregate_ephemeral_rows(&rows, &columns, gb_cols, resolved_having.as_ref());
            } else if is_aggregate_query(&columns) {
                let mut agg_cols = Vec::new();
                let mut agg_vals = Vec::new();

                for col in &columns {
                    if let Some(spec) = parse_aggregate_spec(col) {
                        let computed = compute_aggregate_from_spec(&spec, &rows);
                        agg_cols.push(spec.output_name);
                        agg_vals.push(computed);
                    } else {
                        agg_cols.push(parse_col_and_alias(col).1.unwrap_or(col).to_string());
                        agg_vals.push(Value::Null);
                    }
                }

                return Ok(vec![Row::new(agg_cols, agg_vals)]);
            } else if !columns.is_empty() && columns[0] != "*" {
                // Full expression projection (supports arithmetic, scalar UDFs, JSON paths, aliases)
                rows = rows
                    .iter()
                    .map(|r| self.project_row(r, &columns))
                    .collect::<Result<Vec<Row>>>()?;
            }

            // Apply ORDER BY (after GROUP BY / projection so aliases and aggregate names resolve)
            if let Some(ref order) = order_by {
                rows.sort_by(|a, b| {
                    let val_a = a.get_value(&order.column).unwrap_or(&Value::Null);
                    let val_b = b.get_value(&order.column).unwrap_or(&Value::Null);
                    if order.is_ascending {
                        val_a.compare(val_b)
                    } else {
                        val_b.compare(val_a)
                    }
                });
            }

            // Apply DISTINCT
            if distinct {
                let mut seen = HashSet::new();
                rows.retain(|r| {
                    let key = format!("{:?}", r.values());
                    seen.insert(key)
                });
            }

            // Apply OFFSET
            if let Some(off) = offset {
                if off < rows.len() {
                    rows = rows.split_off(off);
                } else {
                    rows.clear();
                }
            }

            // Apply LIMIT
            if let Some(lim) = limit {
                rows.truncate(lim);
            }

            Ok(rows)
        } else {
            Err(Error::SqlSyntax("Expected SELECT in CTE main query".into()))
        }
    }
}

fn collect_top_level_in_cols<'a>(expr: &'a WhereExpr, out: &mut Vec<&'a str>) {
    match expr {
        WhereExpr::And(l, r) => {
            collect_top_level_in_cols(l, out);
            collect_top_level_in_cols(r, out);
        }
        WhereExpr::InList {
            column,
            negated: false,
            ..
        }
        | WhereExpr::InSubquery {
            column,
            negated: false,
            ..
        } => {
            out.push(column.as_str());
        }
        _ => {}
    }
}
