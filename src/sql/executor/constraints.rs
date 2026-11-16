#[allow(unused_imports)]
use super::*;

impl SQLExecutor {
    pub(super) fn partial_index_matches_row(
        &mut self,
        pager: &mut Pager,
        index_def: &IndexDef,
        row: &Row,
    ) -> Result<bool> {
        let Some(ref pred_sql) = index_def.predicate_sql else {
            return Ok(true);
        };
        let expr = self.parse_cached_where_expr(pred_sql)?;
        let resolved = self.resolve_where_expr(pager, &expr)?;
        Ok(row_matches_resolved(row, &resolved))
    }

    pub(super) fn find_usable_index_for_column(
        &self,
        table: &str,
        column: &str,
        query_where: Option<&WhereExpr>,
    ) -> Option<IndexDef> {
        let norm_target = normalize_index_expr(column, Some(table));
        let candidates: Vec<&IndexDef> = self
            .catalog
            .indexes_for_table(table)
            .into_iter()
            .filter(|idx| {
                (idx.column.eq_ignore_ascii_case(column)
                    || normalize_index_expr(&idx.column, Some(table)) == norm_target)
                    && partial_index_matches_query(idx, query_where)
            })
            .collect();
        candidates
            .iter()
            .copied()
            .find(|idx| idx.predicate_sql.is_none())
            .or_else(|| candidates.into_iter().next())
            .cloned()
    }

    pub(super) fn index_add_row(
        &mut self,
        pager: &mut Pager,
        index_def: &IndexDef,
        row: &Row,
        row_id: u64,
    ) -> Result<()> {
        if !self.partial_index_matches_row(pager, index_def, row)? {
            return Ok(());
        }
        let cols = index_def.target_columns();
        if let Some((val, _)) = extract_index_key_value(row, &cols) {
            let key = value_to_index_key(&val);
            // A corrupted bucket must surface as an error: silently replacing it with an
            // empty bucket would drop every other row ID sharing this hash key.
            let mut bucket = IndexBucket::load(&self.btree, pager, index_def.root_page, key)?
                .unwrap_or_default();
            bucket.add_row_id(val, row_id);
            let idx_payload = bucket.to_bytes();
            self.btree
                .insert(pager, index_def.root_page, key, &idx_payload)?;
        }
        Ok(())
    }

    pub(super) fn index_remove_row(
        &mut self,
        pager: &mut Pager,
        index_def: &IndexDef,
        row: &Row,
        row_id: u64,
    ) -> Result<()> {
        if !self.partial_index_matches_row(pager, index_def, row)? {
            return Ok(());
        }
        let cols = index_def.target_columns();
        if let Some((val, _)) = extract_index_key_value(row, &cols) {
            let key = value_to_index_key(&val);
            if let Some(mut bucket) =
                IndexBucket::load(&self.btree, pager, index_def.root_page, key)?
            {
                bucket.remove_row_id(&val, row_id);
                if bucket.is_empty() {
                    self.btree.delete(pager, index_def.root_page, key)?;
                } else {
                    self.btree
                        .insert(pager, index_def.root_page, key, &bucket.to_bytes())?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn remove_row_from_all_indexes(
        &mut self,
        pager: &mut Pager,
        table: &str,
        row: &Row,
        row_id: u64,
    ) -> Result<()> {
        for index_def in self.catalog.indexes() {
            if index_def.table.eq_ignore_ascii_case(table) {
                self.index_remove_row(pager, &index_def, row, row_id)?;
            }
        }
        Ok(())
    }

    pub(super) fn add_row_to_all_indexes(
        &mut self,
        pager: &mut Pager,
        table: &str,
        row: &Row,
        row_id: u64,
    ) -> Result<()> {
        for index_def in self.catalog.indexes() {
            if index_def.table.eq_ignore_ascii_case(table) {
                self.index_add_row(pager, &index_def, row, row_id)?;
            }
        }
        Ok(())
    }

    pub(super) fn validate_unique_constraints(
        &mut self,
        pager: &mut Pager,
        table_def: &TableDef,
        candidate_row: &Row,
        exclude_row_id: Option<u64>,
    ) -> Result<()> {
        // 1. Check any explicit CREATE UNIQUE INDEX using B+Tree bucket O(log N)
        for index_def in self.catalog.indexes() {
            if index_def.unique && index_def.table.eq_ignore_ascii_case(&table_def.name) {
                if !self.partial_index_matches_row(pager, &index_def, candidate_row)? {
                    continue;
                }
                let cols = index_def.target_columns();
                if let Some((val, all_non_null)) = extract_index_key_value(candidate_row, &cols)
                    && all_non_null
                {
                    let key = value_to_index_key(&val);
                    if let Some(bucket) =
                        IndexBucket::load(&self.btree, pager, index_def.root_page, key)?
                        && let Some(rids) = bucket.get_row_ids(&val)
                        && rids.iter().any(|&rid| Some(rid) != exclude_row_id)
                    {
                        return Err(Error::ConstraintViolation(format!(
                            "UNIQUE constraint failed: {}.{}",
                            table_def.name,
                            cols.join(", ")
                        )));
                    }
                }
            }
        }

        // 2. Collect inline column UNIQUE, table UNIQUE(...), and composite PRIMARY KEY(...)
        let mut unique_groups: Vec<Vec<String>> = Vec::new();
        for col in &table_def.columns {
            if col.unique {
                unique_groups.push(vec![col.name.clone()]);
            }
        }
        for grp in &table_def.unique_constraints {
            if !grp.is_empty() {
                unique_groups.push(grp.clone());
            }
        }
        if table_def.primary_key_columns.len() > 1 {
            unique_groups.push(table_def.primary_key_columns.clone());
        }
        if unique_groups.is_empty() {
            return Ok(());
        }

        let candidate_tuples: Vec<Option<Vec<Value>>> = unique_groups
            .iter()
            .map(|cols| {
                let mut vals = Vec::with_capacity(cols.len());
                for c in cols {
                    let v = candidate_row
                        .get_field_or_json_path(c)
                        .unwrap_or(Value::Null);
                    if v.is_null() {
                        return None;
                    }
                    vals.push(v);
                }
                Some(vals)
            })
            .collect();

        if candidate_tuples.iter().all(Option::is_none) {
            return Ok(());
        }

        let mut unique_indices: Vec<Vec<usize>> = Vec::with_capacity(unique_groups.len());
        let mut needed_mask = vec![false; table_def.columns.len()];
        for cols in &unique_groups {
            let mut idxs = Vec::with_capacity(cols.len());
            for c in cols {
                if let Some(i) = table_def.column_index(c)
                    && i < needed_mask.len()
                {
                    needed_mask[i] = true;
                    idxs.push(i);
                } else {
                    idxs.clear();
                    break;
                }
            }
            unique_indices.push(idxs);
        }

        let mut row_vals = Vec::with_capacity(table_def.columns.len());
        let mut violation: Option<String> = None;
        self.btree
            .scan_with_ref(pager, table_def.root_page, |row_id, payload| {
                if Some(row_id) == exclude_row_id {
                    return Ok(true);
                }
                decode_row_values_into_projected(payload, &needed_mask, &mut row_vals)?;
                for (g_idx, cols) in unique_groups.iter().enumerate() {
                    let Some(ref cand_vals) = candidate_tuples[g_idx] else {
                        continue;
                    };
                    let idxs = &unique_indices[g_idx];
                    if idxs.len() != cols.len() {
                        continue;
                    }
                    let all_match = idxs
                        .iter()
                        .zip(cand_vals.iter())
                        .all(|(&col_i, cand)| row_vals.get(col_i).unwrap_or(&Value::Null) == cand);
                    if all_match {
                        violation = Some(format!(
                            "UNIQUE constraint failed: {}.{}",
                            table_def.name,
                            cols.join(", ")
                        ));
                        return Ok(false);
                    }
                }
                Ok(true)
            })?;
        if let Some(msg) = violation {
            return Err(Error::ConstraintViolation(msg));
        }
        Ok(())
    }

    pub(super) fn validate_check_constraints_for_row(
        &mut self,
        table_def: &TableDef,
        row: &Row,
    ) -> Result<()> {
        let has_col_checks = table_def.columns.iter().any(|c| c.check_expr.is_some());
        if !has_col_checks && table_def.check_constraints.is_empty() {
            return Ok(());
        }
        let mut exprs: Vec<(Option<String>, String)> = Vec::new();
        for col in &table_def.columns {
            if let Some(ref expr_sql) = col.check_expr {
                exprs.push((Some(col.name.clone()), expr_sql.clone()));
            }
        }
        for chk in &table_def.check_constraints {
            exprs.push((chk.name.clone(), chk.expr_sql.clone()));
        }
        for (label, expr_sql) in exprs {
            let parsed = self.parse_cached_where_expr(&expr_sql)?;
            if eval_where_three_valued(row, &parsed, Some(&self.udf_registry)) == Some(false) {
                let tag = label.as_deref().unwrap_or("CHECK");
                return Err(Error::ConstraintViolation(format!(
                    "CHECK constraint failed ({tag}) on {}: {expr_sql}",
                    table_def.name
                )));
            }
        }
        Ok(())
    }

    pub(super) fn validate_child_foreign_keys(
        &self,
        pager: &mut Pager,
        table_def: &TableDef,
        row: &Row,
    ) -> Result<()> {
        if !self.foreign_keys_enabled {
            return Ok(());
        }
        for fk in collect_table_foreign_keys(table_def) {
            let child_vals: Vec<Value> = fk
                .columns
                .iter()
                .map(|c| row.get_field_or_json_path(c).unwrap_or(Value::Null))
                .collect();
            if child_vals.iter().any(Value::is_null) {
                continue;
            }
            let parent_def = self.catalog.get_table(&fk.ref_table).ok_or_else(|| {
                Error::ConstraintViolation(format!(
                    "FOREIGN KEY references non-existent table '{}'",
                    fk.ref_table
                ))
            })?;
            let parent_cols = resolve_fk_parent_cols(parent_def, &fk);
            if parent_cols.len() != child_vals.len() {
                return Err(Error::ConstraintViolation(format!(
                    "FOREIGN KEY column count mismatch between {} and {}",
                    table_def.name, fk.ref_table
                )));
            }

            if parent_cols.len() == 1
                && parent_def
                    .primary_key_index()
                    .and_then(|idx| parent_def.columns.get(idx))
                    .is_some_and(|pk| pk.name.eq_ignore_ascii_case(&parent_cols[0]))
                && let Value::Integer(pk_id) = child_vals[0]
            {
                if self
                    .btree
                    .search(pager, parent_def.root_page, pk_id as u64)?
                    .is_some()
                {
                    continue;
                } else {
                    return Err(Error::ConstraintViolation(format!(
                        "FOREIGN KEY constraint failed: {}.{} -> {}.{}",
                        table_def.name,
                        fk.columns.join(","),
                        fk.ref_table,
                        parent_cols.join(",")
                    )));
                }
            }

            // Check any full (non-partial) secondary index matching `parent_cols` in O(log N)
            let parent_index = self.catalog.indexes().into_iter().find(|idx| {
                idx.table.eq_ignore_ascii_case(&parent_def.name) && idx.predicate_sql.is_none() && {
                    let tcols = idx.target_columns();
                    tcols.len() == parent_cols.len()
                        && tcols
                            .iter()
                            .zip(parent_cols.iter())
                            .all(|(a, b)| a.eq_ignore_ascii_case(b))
                }
            });
            if let Some(idx_def) = parent_index {
                let dummy_row = Row::new(parent_cols.clone(), child_vals.clone());
                let tcols = idx_def.target_columns();
                if let Some((idx_val, _)) = extract_index_key_value(&dummy_row, &tcols) {
                    let key = value_to_index_key(&idx_val);
                    let found = IndexBucket::load(&self.btree, pager, idx_def.root_page, key)?
                        .and_then(|b| b.get_row_ids(&idx_val).map(|ids| !ids.is_empty()))
                        .unwrap_or(false);
                    if found {
                        continue;
                    }
                    return Err(Error::ConstraintViolation(format!(
                        "FOREIGN KEY constraint failed: {}.{} -> {}.{}",
                        table_def.name,
                        fk.columns.join(","),
                        fk.ref_table,
                        parent_cols.join(",")
                    )));
                }
            }

            let mut p_indices = Vec::with_capacity(parent_cols.len());
            let mut p_mask = vec![false; parent_def.columns.len()];
            let mut all_simple = true;
            for c in &parent_cols {
                if let Some(i) = parent_def.column_index(c)
                    && i < p_mask.len()
                {
                    p_mask[i] = true;
                    p_indices.push(i);
                } else {
                    all_simple = false;
                    break;
                }
            }

            let mut found = false;
            if all_simple {
                let mut row_vals = Vec::with_capacity(parent_def.columns.len());
                self.btree
                    .scan_with_ref(pager, parent_def.root_page, |_rid, payload| {
                        decode_row_values_into_projected(payload, &p_mask, &mut row_vals)?;
                        if p_indices
                            .iter()
                            .zip(child_vals.iter())
                            .all(|(&pi, cv)| row_vals.get(pi).unwrap_or(&Value::Null) == cv)
                        {
                            found = true;
                            return Ok(false);
                        }
                        Ok(true)
                    })?;
            } else {
                let parent_all_cols = parent_def.column_names();
                self.btree
                    .scan_with_ref(pager, parent_def.root_page, |_rid, payload| {
                        let p_row = decode_row(payload, &parent_all_cols)?;
                        let p_vals: Vec<Value> = parent_cols
                            .iter()
                            .map(|c| p_row.get_field_or_json_path(c).unwrap_or(Value::Null))
                            .collect();
                        if p_vals == child_vals {
                            found = true;
                            return Ok(false);
                        }
                        Ok(true)
                    })?;
            }
            if !found {
                return Err(Error::ConstraintViolation(format!(
                    "FOREIGN KEY constraint failed: {}.{} -> {}.{}",
                    table_def.name,
                    fk.columns.join(","),
                    fk.ref_table,
                    parent_cols.join(",")
                )));
            }
        }
        Ok(())
    }

    pub(super) fn find_matching_child_rows(
        &self,
        pager: &mut Pager,
        child_def: &TableDef,
        child_cols: &[String],
        parent_vals: &[Value],
    ) -> Result<Vec<(u64, Row)>> {
        if parent_vals.iter().any(Value::is_null) {
            return Ok(Vec::new());
        }
        let all_cols = child_def.column_names();
        let shared_cols = Arc::new(all_cols);

        // Fast O(log N) secondary-index probe on child FK columns when available
        let child_index = self.catalog.indexes().into_iter().find(|idx| {
            idx.table.eq_ignore_ascii_case(&child_def.name) && idx.predicate_sql.is_none() && {
                let tcols = idx.target_columns();
                tcols.len() == child_cols.len()
                    && tcols
                        .iter()
                        .zip(child_cols.iter())
                        .all(|(a, b)| a.eq_ignore_ascii_case(b))
            }
        });
        if let Some(idx_def) = child_index {
            let dummy_row = Row::new(child_cols.to_vec(), parent_vals.to_vec());
            let tcols = idx_def.target_columns();
            if let Some((idx_val, _)) = extract_index_key_value(&dummy_row, &tcols) {
                let key = value_to_index_key(&idx_val);
                let mut matches = Vec::new();
                if let Some(bucket) = IndexBucket::load(&self.btree, pager, idx_def.root_page, key)?
                    && let Some(rids) = bucket.get_row_ids(&idx_val)
                {
                    for &rid in rids {
                        if let Some(payload) = self.btree.search(pager, child_def.root_page, rid)? {
                            let row = decode_row_shared(&payload, &shared_cols)?;
                            matches.push((rid, row));
                        }
                    }
                }
                return Ok(matches);
            }
        }

        let mut c_indices = Vec::with_capacity(child_cols.len());
        let mut c_mask = vec![false; child_def.columns.len()];
        let mut all_simple = true;
        for c in child_cols {
            if let Some(i) = child_def.column_index(c)
                && i < c_mask.len()
            {
                c_mask[i] = true;
                c_indices.push(i);
            } else {
                all_simple = false;
                break;
            }
        }

        let mut matches = Vec::new();
        if all_simple {
            let mut row_vals = Vec::with_capacity(child_def.columns.len());
            self.btree
                .scan_with_ref(pager, child_def.root_page, |row_id, payload| {
                    decode_row_values_into_projected(payload, &c_mask, &mut row_vals)?;
                    if c_indices
                        .iter()
                        .zip(parent_vals.iter())
                        .all(|(&ci, pv)| row_vals.get(ci).unwrap_or(&Value::Null) == pv)
                    {
                        let row = decode_row_shared(payload, &shared_cols)?;
                        matches.push((row_id, row));
                    }
                    Ok(true)
                })?;
        } else {
            self.btree
                .scan_with_ref(pager, child_def.root_page, |row_id, payload| {
                    let row = decode_row_shared(payload, &shared_cols)?;
                    let c_vals: Vec<Value> = child_cols
                        .iter()
                        .map(|c| row.get_field_or_json_path(c).unwrap_or(Value::Null))
                        .collect();
                    if c_vals == parent_vals {
                        matches.push((row_id, row));
                    }
                    Ok(true)
                })?;
        }
        Ok(matches)
    }

    pub(super) fn handle_parent_foreign_key_on_delete(
        &mut self,
        pager: &mut Pager,
        parent_table: &str,
        old_parent_row: &Row,
    ) -> Result<()> {
        if !self.foreign_keys_enabled {
            return Ok(());
        }
        if self.fk_cascade_depth >= 16 {
            return Err(Error::ConstraintViolation(
                "FOREIGN KEY CASCADE recursion limit exceeded".into(),
            ));
        }
        let parent_def = match self.catalog.get_table(parent_table).cloned() {
            Some(d) => d,
            None => return Ok(()),
        };
        let all_tables = self.catalog.tables();
        for child_def in all_tables {
            for fk in collect_table_foreign_keys(&child_def) {
                if !fk.ref_table.eq_ignore_ascii_case(parent_table) {
                    continue;
                }
                let parent_cols = resolve_fk_parent_cols(&parent_def, &fk);
                let parent_vals: Vec<Value> = parent_cols
                    .iter()
                    .map(|c| {
                        old_parent_row
                            .get_field_or_json_path(c)
                            .unwrap_or(Value::Null)
                    })
                    .collect();
                let matches =
                    self.find_matching_child_rows(pager, &child_def, &fk.columns, &parent_vals)?;
                if matches.is_empty() {
                    continue;
                }
                match fk.on_delete {
                    ForeignKeyAction::NoAction | ForeignKeyAction::Restrict => {
                        return Err(Error::ConstraintViolation(format!(
                            "FOREIGN KEY constraint failed: '{parent_table}' row referenced by '{}.{}'",
                            child_def.name,
                            fk.columns.join(",")
                        )));
                    }
                    ForeignKeyAction::Cascade => {
                        self.fk_cascade_depth += 1;
                        let mut res = Ok(());
                        for (child_rid, _) in matches {
                            if let Some(pk_idx) = child_def.primary_key_index() {
                                let pk_name = &child_def.columns[pk_idx].name;
                                let cond = WhereExpr::Condition(WhereCondition {
                                    column: pk_name.clone(),
                                    op: BinaryOp::Equals,
                                    value: Value::Integer(child_rid as i64),
                                });
                                res = self
                                    .execute_delete(pager, child_def.name.clone(), Some(cond), None)
                                    .map(|_| ());
                            } else if let Some(first_col) = fk.columns.first() {
                                let cond = WhereExpr::Condition(WhereCondition {
                                    column: first_col.clone(),
                                    op: BinaryOp::Equals,
                                    value: parent_vals[0].clone(),
                                });
                                res = self
                                    .execute_delete(pager, child_def.name.clone(), Some(cond), None)
                                    .map(|_| ());
                            }
                            if res.is_err() {
                                break;
                            }
                        }
                        self.fk_cascade_depth -= 1;
                        res?;
                    }
                    ForeignKeyAction::SetNull => {
                        self.fk_cascade_depth += 1;
                        let mut res = Ok(());
                        let assignments: Vec<(String, Value)> = fk
                            .columns
                            .iter()
                            .map(|c| (c.clone(), Value::Null))
                            .collect();
                        for (idx, col_name) in fk.columns.iter().enumerate() {
                            if idx < parent_vals.len() {
                                let cond = WhereExpr::Condition(WhereCondition {
                                    column: col_name.clone(),
                                    op: BinaryOp::Equals,
                                    value: parent_vals[idx].clone(),
                                });
                                res = self
                                    .execute_update(
                                        pager,
                                        child_def.name.clone(),
                                        assignments.clone(),
                                        Vec::new(),
                                        Some(cond),
                                        None,
                                    )
                                    .map(|_| ());
                                break;
                            }
                        }
                        self.fk_cascade_depth -= 1;
                        res?;
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn handle_parent_foreign_key_on_update(
        &mut self,
        pager: &mut Pager,
        parent_table: &str,
        old_parent_row: &Row,
        new_parent_row: &Row,
    ) -> Result<()> {
        if !self.foreign_keys_enabled {
            return Ok(());
        }
        if self.fk_cascade_depth >= 16 {
            return Err(Error::ConstraintViolation(
                "FOREIGN KEY CASCADE recursion limit exceeded".into(),
            ));
        }
        let parent_def = match self.catalog.get_table(parent_table).cloned() {
            Some(d) => d,
            None => return Ok(()),
        };
        let all_tables = self.catalog.tables();
        for child_def in all_tables {
            for fk in collect_table_foreign_keys(&child_def) {
                if !fk.ref_table.eq_ignore_ascii_case(parent_table) {
                    continue;
                }
                let parent_cols = resolve_fk_parent_cols(&parent_def, &fk);
                let old_vals: Vec<Value> = parent_cols
                    .iter()
                    .map(|c| {
                        old_parent_row
                            .get_field_or_json_path(c)
                            .unwrap_or(Value::Null)
                    })
                    .collect();
                let new_vals: Vec<Value> = parent_cols
                    .iter()
                    .map(|c| {
                        new_parent_row
                            .get_field_or_json_path(c)
                            .unwrap_or(Value::Null)
                    })
                    .collect();
                if old_vals == new_vals {
                    continue;
                }
                let matches =
                    self.find_matching_child_rows(pager, &child_def, &fk.columns, &old_vals)?;
                if matches.is_empty() {
                    continue;
                }
                match fk.on_update {
                    ForeignKeyAction::NoAction | ForeignKeyAction::Restrict => {
                        return Err(Error::ConstraintViolation(format!(
                            "FOREIGN KEY constraint failed on UPDATE of '{parent_table}': referenced by '{}.{}'",
                            child_def.name,
                            fk.columns.join(",")
                        )));
                    }
                    ForeignKeyAction::Cascade | ForeignKeyAction::SetNull => {
                        self.fk_cascade_depth += 1;
                        let target_vals: Vec<Value> = if fk.on_update == ForeignKeyAction::SetNull {
                            vec![Value::Null; fk.columns.len()]
                        } else {
                            new_vals
                        };
                        let assignments: Vec<(String, Value)> =
                            fk.columns.iter().cloned().zip(target_vals).collect();
                        let cond = WhereExpr::Condition(WhereCondition {
                            column: fk.columns[0].clone(),
                            op: BinaryOp::Equals,
                            value: old_vals[0].clone(),
                        });
                        let res = self
                            .execute_update(
                                pager,
                                child_def.name.clone(),
                                assignments,
                                Vec::new(),
                                Some(cond),
                                None,
                            )
                            .map(|_| ());
                        self.fk_cascade_depth -= 1;
                        res?;
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn ensure_fts_index(
        &mut self,
        pager: &mut Pager,
        table_def: &TableDef,
    ) -> Result<()> {
        let Some(ref fts_cfg) = table_def.fts_config else {
            return Ok(());
        };
        let key = table_def.name.to_lowercase();
        if self.fts_indexes.contains_key(&key) {
            return Ok(());
        }
        let mut idx = Bm25Index::new();
        let all_cols = table_def.column_names();
        self.btree
            .scan_with_ref(pager, table_def.root_page, |row_id, payload| {
                let row = decode_row(payload, &all_cols)?;
                let mut text = String::new();
                for col in &fts_cfg.columns {
                    if let Some(Value::Text(s)) = row.get_value(col) {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(s);
                    }
                }
                idx.index_document(row_id, &text);
                Ok(true)
            })?;
        self.fts_indexes.insert(key, idx);
        Ok(())
    }

    pub(super) fn fts_upsert_row(
        &mut self,
        pager: &mut Pager,
        table_def: &TableDef,
        row: &Row,
        row_id: u64,
    ) -> Result<()> {
        let Some(ref fts_cfg) = table_def.fts_config else {
            return Ok(());
        };
        self.ensure_fts_index(pager, table_def)?;
        let mut text = String::new();
        for col in &fts_cfg.columns {
            if let Some(Value::Text(s)) = row.get_value(col) {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(s);
            }
        }
        if let Some(idx) = self.fts_indexes.get_mut(&table_def.name.to_lowercase()) {
            idx.remove_document(row_id);
            idx.index_document(row_id, &text);
        }
        Ok(())
    }

    pub(super) fn fts_remove_row(&mut self, table_def: &TableDef, row_id: u64) {
        if table_def.fts_config.is_some()
            && let Some(idx) = self.fts_indexes.get_mut(&table_def.name.to_lowercase())
        {
            idx.remove_document(row_id);
        }
    }

    pub(super) fn fire_triggers(
        &mut self,
        pager: &mut Pager,
        table: &str,
        timing: TriggerTiming,
        event: TriggerEvent,
        old_row: Option<&Row>,
        new_row: Option<&Row>,
    ) -> Result<bool> {
        let triggers = self.catalog.triggers_for(table, timing, event);
        if triggers.is_empty() {
            return Ok(true);
        }
        if self.trigger_depth >= 16 {
            return Err(Error::ConstraintViolation(
                "Trigger recursion depth limit exceeded".into(),
            ));
        }
        self.trigger_depth += 1;
        let result = (|| -> Result<bool> {
            for trig in triggers {
                if let Some(ref when_sql) = trig.when_sql {
                    let when_tokens = tokenize(when_sql)?;
                    let mut c = 0usize;
                    let when_expr =
                        crate::sql::parser::parse_where_expr_public(&when_tokens, &mut c)?;
                    let resolved = self.resolve_where_expr(pager, &when_expr)?;
                    let mut t_cols = Vec::new();
                    let mut t_vals = Vec::new();
                    if let Some(nr) = new_row {
                        for (col, val) in nr.columns().iter().zip(nr.values().iter()) {
                            t_cols.push(format!("NEW.{col}"));
                            t_vals.push(val.clone());
                            t_cols.push(col.clone());
                            t_vals.push(val.clone());
                        }
                    }
                    if let Some(or) = old_row {
                        for (col, val) in or.columns().iter().zip(or.values().iter()) {
                            t_cols.push(format!("OLD.{col}"));
                            t_vals.push(val.clone());
                            if new_row.is_none() {
                                t_cols.push(col.clone());
                                t_vals.push(val.clone());
                            }
                        }
                    }
                    let trigger_row = Row::new(t_cols, t_vals);
                    if !row_matches_resolved(&trigger_row, &resolved) {
                        continue;
                    }
                }

                let body_tokens = tokenize(&trig.body_sql)?;
                let subst_body = substitute_trigger_row_tokens(&body_tokens, old_row, new_row);
                for stmt_tokens in split_token_statements(&subst_body) {
                    let stmt = parse_tokens(&stmt_tokens)?;
                    match stmt {
                        Statement::Raise { action, message } => {
                            if action.eq_ignore_ascii_case("IGNORE") {
                                return Ok(false);
                            }
                            return Err(Error::ConstraintViolation(format!(
                                "Trigger '{}' RAISE({action}): {message}",
                                trig.name
                            )));
                        }
                        other => {
                            let _ = self.execute_inner(pager, other)?;
                        }
                    }
                }
            }
            Ok(true)
        })();
        self.trigger_depth -= 1;
        result
    }
}
