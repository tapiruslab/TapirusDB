#[allow(unused_imports)]
use super::*;

impl SQLExecutor {
    /// Ultra-fast batch insert bypassing SQL string construction and per-row parsing.
    ///
    /// Resolves schema metadata once per batch and reuses a single binary encoding buffer
    /// for tables without triggers, temporal history, foreign keys, check constraints, FTS,
    /// or secondary indexes; falls back to full per-row enforcement automatically when those
    /// features are enabled.
    pub fn execute_insert_batch(
        &mut self,
        pager: &mut Pager,
        table: &str,
        columns: Option<&[String]>,
        rows: &[Vec<Value>],
    ) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }

        let (
            root_page,
            table_cols,
            pk_idx,
            vector_col,
            is_temporal,
            initial_next_row_id,
            needs_full_path,
        ) = {
            let table_def = self
                .catalog
                .get_table(table)
                .ok_or_else(|| Error::TableNotFound(table.to_string()))?;

            let has_triggers = self
                .catalog
                .triggers()
                .iter()
                .any(|t| t.table.eq_ignore_ascii_case(table));
            let has_indexes = self
                .catalog
                .indexes()
                .iter()
                .any(|idx| idx.table.eq_ignore_ascii_case(table));
            let has_checks = !table_def.check_constraints.is_empty();
            let has_fks =
                self.foreign_keys_enabled && !collect_table_foreign_keys(table_def).is_empty();
            let has_unique_cols = table_def.columns.iter().any(|c| c.unique && !c.primary_key);
            let has_generated = table_def.columns.iter().any(|c| c.generated_expr.is_some());
            let is_temporal = table_def.system_versioning || self.default_system_versioning;
            let has_fts = table_def.fts_config.is_some();

            let complex = has_triggers
                || has_indexes
                || has_checks
                || has_fks
                || has_unique_cols
                || has_generated
                || is_temporal
                || has_fts;

            (
                table_def.root_page,
                table_def.columns.clone(),
                table_def.primary_key_index(),
                table_def.vector_column(),
                is_temporal,
                table_def.next_row_id,
                complex,
            )
        };

        let _ = is_temporal;

        if needs_full_path {
            let cols_owned = columns.map(<[String]>::to_vec);
            return self.atomic_statement(pager, |executor, pager| {
                let mut total = 0usize;
                for row_vals in rows {
                    let (n, _) = executor.execute_insert(
                        pager,
                        table.to_string(),
                        cols_owned.clone(),
                        row_vals.clone(),
                        OnConflict::Abort,
                        None,
                    )?;
                    total += n;
                }
                Ok(total)
            });
        }

        // Precompute column alignment mapping once for the entire batch.
        let col_mapping: Option<Vec<usize>> = if let Some(cols) = columns {
            let table_def = self.catalog.get_table(table).unwrap();
            let mut map = Vec::with_capacity(cols.len());
            for c_name in cols {
                let idx = table_def
                    .column_index(c_name)
                    .ok_or_else(|| Error::Corrupted(format!("Unknown column: {c_name}")))?;
                map.push(idx);
            }
            Some(map)
        } else {
            None
        };

        let template_defaults: Vec<Value> = table_cols
            .iter()
            .map(|c| {
                c.default_value
                    .as_ref()
                    .map(DefaultValue::materialize)
                    .unwrap_or(Value::Null)
            })
            .collect();

        let num_cols = table_cols.len();
        self.atomic_statement(pager, |executor, pager| {
            let mut next_row_id = initial_next_row_id;
            let mut last_row_id = 0u64;
            let mut inserted = 0usize;
            let mut aligned = Vec::with_capacity(num_cols);
            let mut encode_buf = Vec::with_capacity(256);

            for input_vals in rows {
                aligned.clear();
                if let Some(ref map) = col_mapping {
                    if map.len() != input_vals.len() {
                        return Err(Error::SqlSyntax(
                            "Column count does not match value count in INSERT batch".into(),
                        ));
                    }
                    aligned.extend(template_defaults.iter().cloned());
                    for (&dst_idx, val) in map.iter().zip(input_vals.iter()) {
                        aligned[dst_idx] = val.clone();
                    }
                } else {
                    if input_vals.len() != num_cols {
                        return Err(Error::SqlSyntax(format!(
                            "INSERT requires {} values, got {}",
                            num_cols,
                            input_vals.len()
                        )));
                    }
                    aligned.extend(input_vals.iter().cloned());
                }

                // Coerce types & enforce NOT NULL
                for (col_idx, col_def) in table_cols.iter().enumerate() {
                    let orig = std::mem::replace(&mut aligned[col_idx], Value::Null);
                    let coerced = col_def.data_type.coerce_value(orig).map_err(|e| match e {
                        Error::DimensionMismatch(exp, got) => Error::DimensionMismatch(exp, got),
                        Error::ConstraintViolation(msg) => Error::ConstraintViolation(format!(
                            "{table}.{}: {msg}",
                            col_def.name
                        )),
                        other => other,
                    })?;
                    if col_def.not_null && coerced.is_null() {
                        return Err(Error::ConstraintViolation(format!(
                            "NOT NULL constraint failed: {table}.{}",
                            col_def.name
                        )));
                    }
                    aligned[col_idx] = coerced;
                }

                let (row_id, need_pk_check) = if let Some(idx) = pk_idx {
                    match &aligned[idx] {
                        Value::Integer(i) => {
                            let id_val = *i as u64;
                            if id_val >= next_row_id {
                                next_row_id = id_val + 1;
                                (id_val, false)
                            } else {
                                (id_val, true)
                            }
                        }
                        Value::Null => {
                            let rid = next_row_id;
                            next_row_id += 1;
                            aligned[idx] = Value::Integer(rid as i64);
                            (rid, false)
                        }
                        _ => {
                            let rid = next_row_id;
                            next_row_id += 1;
                            (rid, false)
                        }
                    }
                } else {
                    let rid = next_row_id;
                    next_row_id += 1;
                    (rid, false)
                };

                if need_pk_check && executor.btree.search(pager, root_page, row_id)?.is_some() {
                    let pk_col_name = pk_idx
                        .map(|i| table_cols[i].name.as_str())
                        .unwrap_or("id");
                    return Err(Error::ConstraintViolation(format!(
                        "UNIQUE constraint failed: {table}.{pk_col_name} (key {row_id} already exists)"
                    )));
                }

                encode_row_into(&aligned, &mut encode_buf);
                executor.btree.insert(pager, root_page, row_id, &encode_buf)?;

                if let Some((v_idx, _)) = vector_col
                    && let Some(Value::Vector(vec_data)) = aligned.get(v_idx)
                {
                    let index = executor
                        .vector_indexes
                        .entry(table.to_lowercase())
                        .or_insert_with(|| HnswIndex::new(vec_data.len(), DistanceMetric::Cosine));
                    index.insert_vector(row_id, vec_data)?;
                }

                last_row_id = row_id;
                inserted += 1;
            }

            if let Some(table_def) = executor.catalog.get_table_mut(table) {
                table_def.next_row_id = next_row_id;
            }
            if inserted > 0 {
                executor.last_insert_rowid = last_row_id as i64;
            }
            Ok(inserted)
        })
    }

    #[cfg(feature = "std")]
    pub(super) fn execute_copy_from(
        &mut self,
        pager: &mut Pager,
        table: &str,
        source: &str,
        format: Option<&str>,
        options: &[(String, String)],
    ) -> Result<usize> {
        let fmt = crate::lakehouse::DataFormat::parse_format_or_path(format, source)?;
        let src_table = options
            .iter()
            .find(|(k, _)| k == "TABLE")
            .map(|(_, v)| v.as_str());
        let scan_tag = match fmt {
            crate::lakehouse::DataFormat::Csv => "read_csv",
            crate::lakehouse::DataFormat::Tsv => "read_tsv",
            crate::lakehouse::DataFormat::Jsonl => "read_jsonl",
            crate::lakehouse::DataFormat::Parquet => "read_parquet",
            crate::lakehouse::DataFormat::Excel => "read_excel",
            crate::lakehouse::DataFormat::Sqlite => "read_sqlite",
            crate::lakehouse::DataFormat::Pdf => "read_pdf",
            crate::lakehouse::DataFormat::Text | crate::lakehouse::DataFormat::Markdown => {
                "read_text"
            }
            other => {
                return Err(Error::SqlSyntax(format!(
                    "COPY FROM does not support {other:?} directly via SQL; use `tapirus convert`."
                )));
            }
        };

        let ext_rows = crate::lakehouse::scan_external_rows(scan_tag, source, src_table, None)?;
        if ext_rows.is_empty() {
            return Ok(0);
        }

        let cols = ext_rows[0].columns().to_vec();
        if self.catalog.get_table(table).is_none() {
            let mut col_defs = Vec::with_capacity(cols.len());
            for (c_idx, c_name) in cols.iter().enumerate() {
                let mut has_int = false;
                let mut has_real = false;
                let mut has_blob = false;
                let mut has_vec: Option<usize> = None;
                let mut has_text = false;
                for r in ext_rows.iter().take(256) {
                    if let Some(v) = r.values().get(c_idx) {
                        match v {
                            Value::Null => {}
                            Value::Integer(_) => has_int = true,
                            Value::Real(_) => has_real = true,
                            Value::Blob(_) => has_blob = true,
                            Value::Vector(vec) => has_vec = Some(vec.len()),
                            Value::Text(_) => has_text = true,
                        }
                    }
                }
                let ty = if has_text {
                    "TEXT".to_string()
                } else if let Some(d) = has_vec {
                    format!("VECTOR({d})")
                } else if has_blob {
                    "BLOB".to_string()
                } else if has_real {
                    "REAL".to_string()
                } else if has_int {
                    "INTEGER".to_string()
                } else {
                    "TEXT".to_string()
                };
                col_defs.push(format!("{c_name} {ty}"));
            }
            let ddl = format!(
                "CREATE TABLE IF NOT EXISTS {table} ({});",
                col_defs.join(", ")
            );
            let parsed = crate::sql::parser::parse_sql(&ddl)?;
            self.execute_inner(pager, parsed)?;
        }

        let mut total = 0usize;
        for chunk in ext_rows.chunks(2048) {
            let batch: Vec<Vec<Value>> = chunk.iter().map(|r| r.values().to_vec()).collect();
            total += self.execute_insert_batch(pager, table, Some(&cols), &batch)?;
        }
        Ok(total)
    }

    #[cfg(not(feature = "std"))]
    pub(super) fn execute_copy_from(
        &mut self,
        _pager: &mut Pager,
        _table: &str,
        _source: &str,
        _format: Option<&str>,
        _options: &[(String, String)],
    ) -> Result<usize> {
        Err(Error::SqlSyntax(
            "COPY FROM requires the 'std' feature".into(),
        ))
    }

    #[cfg(feature = "std")]
    pub(super) fn execute_copy_to(
        &mut self,
        pager: &mut Pager,
        source_table: Option<&str>,
        source_query: Option<&Statement>,
        dest: &str,
        format: Option<&str>,
        partition_by: Option<&str>,
    ) -> Result<usize> {
        let (rows, tbl_label) = if let Some(q) = source_query {
            (self.query_inner(pager, q.clone())?, "query_export")
        } else if let Some(tbl) = source_table {
            let sel = crate::sql::parser::parse_sql(&format!("SELECT * FROM {tbl};"))?;
            (self.query_inner(pager, sel)?, tbl)
        } else {
            return Err(Error::SqlSyntax(
                "COPY TO requires a table name or (SELECT ...) query".into(),
            ));
        };

        let stats = crate::lakehouse::export_rows_to_path(
            &rows,
            tbl_label,
            std::path::Path::new(dest),
            format,
            partition_by,
        )?;
        Ok(stats.rows_exported)
    }

    #[cfg(not(feature = "std"))]
    pub(super) fn execute_copy_to(
        &mut self,
        _pager: &mut Pager,
        _source_table: Option<&str>,
        _source_query: Option<&Statement>,
        _dest: &str,
        _format: Option<&str>,
        _partition_by: Option<&str>,
    ) -> Result<usize> {
        Err(Error::SqlSyntax(
            "COPY TO requires the 'std' feature".into(),
        ))
    }

    /// High-performance compiled/prepared statement insert execution path.
    /// `encode_buf` is an optional reusable buffer for `encode_row_into` to avoid per-row allocations.
    pub fn execute_insert_fast(
        &mut self,
        pager: &mut Pager,
        table: &str,
        columns: Option<&[String]>,
        values: &[Value],
        encode_buf: Option<&mut Vec<u8>>,
    ) -> Result<(usize, Vec<Row>)> {
        if let Some(table_def) = self.catalog.get_table(table) {
            let has_complex = self.catalog.has_table_triggers(table)
                || self.catalog.has_table_indexes(table)
                || !table_def.check_constraints.is_empty()
                || (self.foreign_keys_enabled && !collect_table_foreign_keys(table_def).is_empty())
                || table_def.columns.iter().any(|c| c.unique && !c.primary_key)
                || !table_def.unique_constraints.is_empty()
                || table_def.system_versioning
                || self.default_system_versioning
                || table_def.fts_config.is_some()
                || table_def.vector_column().is_some();

            if !has_complex {
                let root_page = table_def.root_page;
                let num_cols = table_def.columns.len();
                let pk_idx = table_def.primary_key_index();
                let current_next_row_id = table_def.next_row_id;

                self.insert_scratch_vals.clear();
                if let Some(cols) = columns {
                    if cols.len() != values.len() {
                        return Err(Error::SqlSyntax(
                            "Column count does not match value count in INSERT".into(),
                        ));
                    }
                    for c in &table_def.columns {
                        self.insert_scratch_vals.push(
                            c.default_value
                                .as_ref()
                                .map(DefaultValue::materialize)
                                .unwrap_or(Value::Null),
                        );
                    }
                    for (c_name, val) in cols.iter().zip(values.iter()) {
                        let idx = table_def
                            .column_index(c_name)
                            .ok_or_else(|| Error::Corrupted(format!("Unknown column: {c_name}")))?;
                        self.insert_scratch_vals[idx] = val.clone();
                    }
                } else {
                    if values.len() != num_cols {
                        return Err(Error::SqlSyntax(format!(
                            "INSERT requires {} values, got {}",
                            num_cols,
                            values.len()
                        )));
                    }
                    self.insert_scratch_vals.extend(values.iter().cloned());
                }

                for (col_idx, col_def) in table_def.columns.iter().enumerate() {
                    let orig =
                        std::mem::replace(&mut self.insert_scratch_vals[col_idx], Value::Null);
                    let coerced = col_def.data_type.coerce_value(orig).map_err(|e| match e {
                        Error::DimensionMismatch(exp, got) => Error::DimensionMismatch(exp, got),
                        Error::ConstraintViolation(msg) => {
                            Error::ConstraintViolation(format!("{table}.{}: {msg}", col_def.name))
                        }
                        other => other,
                    })?;
                    if col_def.not_null && coerced.is_null() {
                        return Err(Error::ConstraintViolation(format!(
                            "NOT NULL constraint failed: {table}.{}",
                            col_def.name
                        )));
                    }
                    self.insert_scratch_vals[col_idx] = coerced;
                }

                let mut next_rid = current_next_row_id;
                let (row_id, need_pk_check) = if let Some(idx) = pk_idx {
                    match &self.insert_scratch_vals[idx] {
                        Value::Integer(i) => {
                            let id_val = *i as u64;
                            if id_val >= next_rid {
                                next_rid = id_val + 1;
                                (id_val, false)
                            } else {
                                (id_val, true)
                            }
                        }
                        Value::Null => {
                            let rid = next_rid;
                            next_rid += 1;
                            self.insert_scratch_vals[idx] = Value::Integer(rid as i64);
                            (rid, false)
                        }
                        _ => {
                            let rid = next_rid;
                            next_rid += 1;
                            (rid, false)
                        }
                    }
                } else {
                    let rid = next_rid;
                    next_rid += 1;
                    (rid, false)
                };

                if need_pk_check && self.btree.search(pager, root_page, row_id)?.is_some() {
                    let pk_col_name = pk_idx
                        .map(|i| table_def.columns[i].name.as_str())
                        .unwrap_or("id");
                    return Err(Error::ConstraintViolation(format!(
                        "UNIQUE constraint failed: {table}.{pk_col_name} (key {row_id} already exists)"
                    )));
                }

                let buf = match encode_buf {
                    Some(b) => b,
                    None => &mut self.insert_scratch_buf,
                };
                encode_row_into(&self.insert_scratch_vals, buf);

                let own_tx = !pager.is_in_transaction();
                if own_tx {
                    pager.begin_transaction()?;
                }
                if let Err(err) = self.btree.insert(pager, root_page, row_id, buf) {
                    if own_tx && pager.is_in_transaction() {
                        let _ = pager.rollback_transaction();
                    }
                    return Err(err);
                }
                if own_tx && let Err(err) = pager.commit_transaction() {
                    if pager.is_in_transaction() {
                        let _ = pager.rollback_transaction();
                    }
                    let _ = self.reload_from_pager(pager);
                    return Err(err);
                }

                if let Some(table_def_mut) = self.catalog.get_table_mut(table) {
                    table_def_mut.next_row_id = next_rid;
                }
                self.last_insert_rowid = row_id as i64;
                return Ok((1, Vec::new()));
            }
        }

        let statement = Statement::Insert {
            table: table.to_owned(),
            columns: columns.map(<[String]>::to_vec),
            values: values.to_vec(),
            conflict_action: OnConflict::Abort,
            returning: None,
        };
        self.atomic_statement(pager, |executor, pager| {
            if let Statement::Insert {
                table,
                columns,
                values,
                conflict_action,
                returning,
            } = statement
            {
                executor.execute_insert(pager, table, columns, values, conflict_action, returning)
            } else {
                unreachable!()
            }
        })
    }

    pub(super) fn execute_insert(
        &mut self,
        pager: &mut Pager,
        table: String,
        columns: Option<Vec<String>>,
        values: Vec<Value>,
        conflict_action: OnConflict,
        returning: Option<Vec<String>>,
    ) -> Result<(usize, Vec<Row>)> {
        let table_def_snapshot;
        let table_cols;
        let root_page;
        let row_id;
        let key_may_exist;
        let mut aligned_values;
        let vector_col;
        let is_temporal;

        {
            let table_def = self
                .catalog
                .get_table_mut(&table)
                .ok_or_else(|| Error::TableNotFound(table.clone()))?;

            root_page = table_def.root_page;
            table_cols = table_def.columns.clone();
            vector_col = table_def.vector_column();
            is_temporal = table_def.system_versioning || self.default_system_versioning;

            // Align values to table columns, applying declared DEFAULT values for omitted columns
            aligned_values = if let Some(cols) = columns {
                if cols.len() != values.len() {
                    return Err(Error::SqlSyntax(
                        "Column count does not match value count in INSERT".into(),
                    ));
                }
                let mut row_vals: Vec<Value> = table_cols
                    .iter()
                    .map(|c| {
                        c.default_value
                            .as_ref()
                            .map(DefaultValue::materialize)
                            .unwrap_or(Value::Null)
                    })
                    .collect();
                for (c_name, val) in cols.iter().zip(values) {
                    let idx = table_def
                        .column_index(c_name)
                        .ok_or_else(|| Error::Corrupted(format!("Unknown column: {c_name}")))?;
                    if table_cols[idx].generated_expr.is_some() {
                        return Err(Error::ConstraintViolation(format!(
                            "Cannot explicitly write to generated column '{table}.{}'",
                            table_cols[idx].name
                        )));
                    }
                    row_vals[idx] = val;
                }
                row_vals
            } else {
                let non_gen_indices: Vec<usize> = table_cols
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.generated_expr.is_none())
                    .map(|(i, _)| i)
                    .collect();
                if values.len() == non_gen_indices.len() {
                    let mut row_vals = vec![Value::Null; table_cols.len()];
                    for (idx, val) in non_gen_indices.into_iter().zip(values) {
                        row_vals[idx] = val;
                    }
                    row_vals
                } else {
                    return Err(Error::SqlSyntax(format!(
                        "INSERT requires {} values, got {}",
                        non_gen_indices.len(),
                        values.len()
                    )));
                }
            };

            // Determine row_id (from primary key column or auto-increment)
            let pk_idx = table_def.primary_key_index();
            let (rid, may_exist) = if let Some(idx) = pk_idx {
                match &aligned_values[idx] {
                    Value::Integer(i) => {
                        let id_val = *i as u64;
                        if id_val >= table_def.next_row_id {
                            table_def.next_row_id = id_val + 1;
                        }
                        (id_val, true)
                    }
                    Value::Null => {
                        let rid = table_def.next_row_id;
                        table_def.next_row_id += 1;
                        aligned_values[idx] = Value::Integer(rid as i64);
                        (rid, false)
                    }
                    _ => {
                        let rid = table_def.next_row_id;
                        table_def.next_row_id += 1;
                        (rid, false)
                    }
                }
            } else {
                let rid = table_def.next_row_id;
                table_def.next_row_id += 1;
                (rid, false)
            };
            row_id = rid;
            key_may_exist = may_exist;
            table_def_snapshot = table_def.clone();
        }

        // Enforce column data type validation and coercion for supplied values, then compute generated columns
        for (col_idx, col_def) in table_cols.iter().enumerate() {
            if col_def.generated_expr.is_some() {
                continue;
            }
            if let Some(val) = aligned_values.get_mut(col_idx) {
                let orig = std::mem::replace(val, Value::Null);
                *val = col_def.data_type.coerce_value(orig).map_err(|e| match e {
                    Error::DimensionMismatch(exp, got) => Error::DimensionMismatch(exp, got),
                    Error::ConstraintViolation(msg) => {
                        Error::ConstraintViolation(format!("{table}.{}: {msg}", col_def.name))
                    }
                    other => other,
                })?;
            }
        }
        self.compute_generated_columns(&table, &table_cols, &mut aligned_values)?;

        // Enforce NOT NULL constraints
        for (col_idx, col_def) in table_cols.iter().enumerate() {
            if col_def.not_null
                && let Some(val) = aligned_values.get(col_idx)
                && val.is_null()
            {
                return Err(Error::ConstraintViolation(format!(
                    "NOT NULL constraint failed: {table}.{}",
                    col_def.name
                )));
            }
        }

        let all_col_names: Vec<String> = table_cols.iter().map(|c| c.name.clone()).collect();
        let candidate_row = Row::new(all_col_names.clone(), aligned_values.clone());

        // BEFORE INSERT triggers
        if !self.fire_triggers(
            pager,
            &table,
            TriggerTiming::Before,
            TriggerEvent::Insert,
            None,
            Some(&candidate_row),
        )? {
            return Ok((0, Vec::new()));
        }

        // If table has a primary key or specific row_id and it already exists, enforce UNIQUE constraint or handle Upsert
        let existing_payload = if key_may_exist {
            self.btree.search(pager, root_page, row_id)?
        } else {
            None
        };
        if let Some(existing_payload) = existing_payload {
            match conflict_action {
                OnConflict::Abort => {
                    let pk_col_name = table_cols
                        .iter()
                        .find(|c| c.primary_key)
                        .map(|c| c.name.as_str())
                        .unwrap_or("id");
                    return Err(Error::ConstraintViolation(format!(
                        "UNIQUE constraint failed: {table}.{pk_col_name} (key {row_id} already exists)"
                    )));
                }
                OnConflict::Ignore => {
                    return Ok((0, Vec::new()));
                }
                OnConflict::Replace => {
                    self.validate_check_constraints_for_row(&table_def_snapshot, &candidate_row)?;
                    self.validate_unique_constraints(
                        pager,
                        &table_def_snapshot,
                        &candidate_row,
                        Some(row_id),
                    )?;
                    self.validate_child_foreign_keys(pager, &table_def_snapshot, &candidate_row)?;
                    if let Ok(old_row) = decode_row(&existing_payload, &all_col_names) {
                        self.remove_row_from_all_indexes(pager, &table, &old_row, row_id)?;
                    }
                    if let Some(index) = self.vector_indexes.get_mut(&table.to_lowercase()) {
                        index.remove_vector(row_id);
                    }
                    self.fts_remove_row(&table_def_snapshot, row_id);
                    let now_ts = self.next_temporal_timestamp();
                    self.close_temporal_version(pager, &table, row_id, now_ts)?;
                    self.btree.delete(pager, root_page, row_id)?;
                }
                OnConflict::DoUpdate(_) | OnConflict::DoUpdateExprs { .. } => {
                    let (updates, exprs) = match conflict_action {
                        OnConflict::DoUpdate(u) => (u, Vec::new()),
                        OnConflict::DoUpdateExprs { values, exprs } => (values, exprs),
                        _ => unreachable!(),
                    };
                    let old_row = decode_row(&existing_payload, &all_col_names)?;
                    let mut values: Vec<Value> = old_row.values().to_vec();
                    let eval_row = if !exprs.is_empty() {
                        let mut eval_cols = Vec::with_capacity(all_col_names.len() * 2);
                        let mut eval_vals = Vec::with_capacity(all_col_names.len() * 2);
                        for (col_name, old_val) in all_col_names.iter().zip(old_row.values().iter())
                        {
                            eval_cols.push(format!("{table}.{col_name}"));
                            eval_vals.push(old_val.clone());
                        }
                        for (col_name, new_val) in
                            all_col_names.iter().zip(candidate_row.values().iter())
                        {
                            eval_cols.push(format!("EXCLUDED.{col_name}"));
                            eval_vals.push(new_val.clone());
                        }
                        Some(Row::new(eval_cols, eval_vals))
                    } else {
                        None
                    };
                    let apply_col_assignment =
                        |col_name: &str, new_val: Value, vals: &mut Vec<Value>| -> Result<()> {
                            if let Some(col_idx) = table_cols
                                .iter()
                                .position(|c| c.name.eq_ignore_ascii_case(col_name))
                                && col_idx < vals.len()
                            {
                                if table_cols[col_idx].generated_expr.is_some() {
                                    return Err(Error::ConstraintViolation(format!(
                                        "Cannot explicitly update generated column '{table}.{}'",
                                        table_cols[col_idx].name
                                    )));
                                }
                                vals[col_idx] = new_val;
                            }
                            Ok(())
                        };
                    for (col_name, new_val) in updates {
                        apply_col_assignment(&col_name, new_val, &mut values)?;
                    }
                    if let Some(ref erow) = eval_row {
                        for (col_name, expr_sql) in exprs {
                            let computed = eval_projected_expr(
                                erow,
                                &expr_sql,
                                &expr_sql,
                                Some(&self.udf_registry),
                            )?;
                            apply_col_assignment(&col_name, computed, &mut values)?;
                        }
                    }
                    for (col_idx, col_def) in table_cols.iter().enumerate() {
                        if col_def.generated_expr.is_some() {
                            continue;
                        }
                        if let Some(val) = values.get_mut(col_idx) {
                            let orig = std::mem::replace(val, Value::Null);
                            *val = col_def.data_type.coerce_value(orig).map_err(|e| match e {
                                Error::DimensionMismatch(exp, got) => {
                                    Error::DimensionMismatch(exp, got)
                                }
                                Error::ConstraintViolation(msg) => Error::ConstraintViolation(
                                    format!("{table}.{}: {msg}", col_def.name),
                                ),
                                other => other,
                            })?;
                        }
                    }
                    self.compute_generated_columns(&table, &table_cols, &mut values)?;
                    for (col_idx, col_def) in table_cols.iter().enumerate() {
                        if col_def.not_null && values.get(col_idx).is_some_and(Value::is_null) {
                            return Err(Error::ConstraintViolation(format!(
                                "NOT NULL constraint failed: {table}.{}",
                                col_def.name
                            )));
                        }
                    }
                    let updated_row = Row::new(all_col_names.clone(), values.clone());
                    self.validate_check_constraints_for_row(&table_def_snapshot, &updated_row)?;
                    self.validate_unique_constraints(
                        pager,
                        &table_def_snapshot,
                        &updated_row,
                        Some(row_id),
                    )?;
                    self.validate_child_foreign_keys(pager, &table_def_snapshot, &updated_row)?;
                    self.handle_parent_foreign_key_on_update(
                        pager,
                        &table,
                        &old_row,
                        &updated_row,
                    )?;
                    self.remove_row_from_all_indexes(pager, &table, &old_row, row_id)?;
                    if let Some(index) = self.vector_indexes.get_mut(&table.to_lowercase()) {
                        index.remove_vector(row_id);
                    }
                    self.fts_remove_row(&table_def_snapshot, row_id);
                    if is_temporal {
                        let now_ts = self.next_temporal_timestamp();
                        self.close_temporal_version(pager, &table, row_id, now_ts)?;
                    }
                    self.btree.delete(pager, root_page, row_id)?;

                    aligned_values = values;
                }
            }
        } else {
            // Fresh insert: enforce CHECK, UNIQUE, and child FOREIGN KEY
            self.validate_check_constraints_for_row(&table_def_snapshot, &candidate_row)?;
            if let Err(err) =
                self.validate_unique_constraints(pager, &table_def_snapshot, &candidate_row, None)
            {
                if conflict_action == OnConflict::Ignore {
                    return Ok((0, Vec::new()));
                }
                return Err(err);
            }
            self.validate_child_foreign_keys(pager, &table_def_snapshot, &candidate_row)?;
        }

        // Encode row to binary record payload
        let payload = encode_row(&aligned_values);

        // Insert into B+Tree
        self.btree.insert(pager, root_page, row_id, &payload)?;

        // Record temporal snapshot for time-travel queries if enabled
        if is_temporal {
            let now_ts = self.next_temporal_timestamp();
            self.record_temporal_version(pager, &table, row_id, &payload, now_ts, u64::MAX)?;
        }

        // Update vector index if present
        if let Some((v_idx, _)) = vector_col
            && let Some(Value::Vector(vec_data)) = aligned_values.get(v_idx)
        {
            let index = self
                .vector_indexes
                .entry(table.to_lowercase())
                .or_insert_with(|| HnswIndex::new(vec_data.len(), DistanceMetric::Cosine));
            index.insert_vector(row_id, vec_data)?;
        }

        let final_row = Row::new(all_col_names, aligned_values);

        // Update secondary indexes and FTS5 index
        self.add_row_to_all_indexes(pager, &table, &final_row, row_id)?;
        self.fts_upsert_row(pager, &table_def_snapshot, &final_row, row_id)?;
        self.last_insert_rowid = row_id as i64;

        // AFTER INSERT triggers
        self.fire_triggers(
            pager,
            &table,
            TriggerTiming::After,
            TriggerEvent::Insert,
            None,
            Some(&final_row),
        )?;

        let returned_rows = if let Some(ref ret_cols) = returning {
            if ret_cols.len() == 1 && ret_cols[0] == "*" {
                vec![final_row]
            } else {
                vec![self.project_row(&final_row, ret_cols)?]
            }
        } else {
            Vec::new()
        };

        Ok((1, returned_rows))
    }

    /// Collect matching `(row_id, full_row)` pairs for DML (`UPDATE` / `DELETE`),
    /// pushing down integer PK point/range scans or secondary-index probes when possible
    /// Collect `(row_id, full_row)` pairs matching an optional `WHERE` clause using PK/index pushdown
    /// and streaming projected `WHERE` filtering via `scan_with_ref` on fallback, plus memoized
    /// correlated subquery filtering when present.
    fn collect_matching_rows_for_dml(
        &mut self,
        pager: &mut Pager,
        table_def: &TableDef,
        where_clause: Option<&WhereExpr>,
        resolved_where: Option<&ResolvedWhereExpr>,
    ) -> Result<Vec<(u64, Row)>> {
        if let Some(r_expr) = resolved_where
            && resolved_where_has_correlated(r_expr)
        {
            let (non_corr, corr_opt) = split_correlated_and_predicates(r_expr);
            let mut candidates = self.collect_matching_rows_scan_for_dml(
                pager,
                table_def,
                where_clause,
                non_corr.as_ref(),
            )?;
            if let Some(corr_expr) = corr_opt {
                let mut memo = CorrelatedSubqueryMemo::default();
                let outer_tables = [table_def];
                let old = std::mem::take(&mut candidates);
                candidates.reserve(old.len());
                for (rid, row) in old {
                    if self.eval_resolved_where_with_subqueries(
                        pager,
                        &row,
                        &corr_expr,
                        &outer_tables,
                        &mut memo,
                    )? {
                        candidates.push((rid, row));
                    }
                }
            }
            return Ok(candidates);
        }
        self.collect_matching_rows_scan_for_dml(pager, table_def, where_clause, resolved_where)
    }

    fn collect_matching_rows_scan_for_dml(
        &self,
        pager: &mut Pager,
        table_def: &TableDef,
        where_clause: Option<&WhereExpr>,
        resolved_where: Option<&ResolvedWhereExpr>,
    ) -> Result<Vec<(u64, Row)>> {
        let root_page = table_def.root_page;
        let all_col_names = table_def.column_names();
        let shared_cols = Arc::new(all_col_names);

        // 1. Integer Primary Key point / range lookup pushdown: O(log N) or O(log N + K)
        let pk_idx = table_def.primary_key_index();
        let is_int_pk = pk_idx.is_some_and(|idx| {
            table_def.columns[idx].data_type == crate::sql::catalog::DataType::Integer
        });
        if is_int_pk
            && let Some(pk_col_idx) = pk_idx
            && let Some(w) = where_clause
        {
            let pk_name = &table_def.columns[pk_col_idx].name;
            let (mn, mx, has) = extract_pk_signed_range(w, pk_name);
            if has {
                if let (Some(a), Some(b)) = (mn, mx) {
                    if a > b {
                        return Ok(Vec::new());
                    }
                    if a == b {
                        let mut matched = Vec::new();
                        if let Some(payload) = self.btree.search(pager, root_page, a as u64)? {
                            let row = decode_row_shared(&payload, &shared_cols)?;
                            if resolved_where.is_none_or(|r| row_matches_resolved(&row, r)) {
                                matched.push((a as u64, row));
                            }
                        }
                        return Ok(matched);
                    }
                }
                let mut matched = Vec::new();
                self.btree.scan_signed_i64_with_ref(
                    pager,
                    root_page,
                    mn,
                    mx,
                    false,
                    |row_id, payload| {
                        let row = decode_row_shared(payload, &shared_cols)?;
                        if resolved_where.is_none_or(|r| row_matches_resolved(&row, r)) {
                            matched.push((row_id as u64, row));
                        }
                        Ok(true)
                    },
                )?;
                return Ok(matched);
            }
        }

        // 2. Secondary index (single-column or composite) equality lookup: O(log N)
        if let Some(w) = where_clause {
            let mut and_eqs = HashMap::new();
            collect_and_eq_conditions(w, &mut and_eqs);
            if !and_eqs.is_empty() {
                let matched_idx = self
                    .catalog
                    .indexes()
                    .into_iter()
                    .filter(|idx| {
                        idx.table.eq_ignore_ascii_case(&table_def.name)
                            && partial_index_matches_query(idx, Some(w))
                    })
                    .filter_map(|idx| {
                        let tcols = idx.target_columns();
                        if !tcols.is_empty()
                            && tcols
                                .iter()
                                .all(|c| and_eqs.contains_key(&c.to_lowercase()))
                        {
                            Some((tcols.len(), idx))
                        } else {
                            None
                        }
                    })
                    .max_by_key(|(len, _)| *len)
                    .map(|(_, idx)| idx);

                if let Some(index_def) = matched_idx {
                    let tcols = index_def.target_columns();
                    let dummy_cols: Vec<String> = tcols.iter().map(|c| (*c).to_string()).collect();
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
                    if let Some((idx_val, _)) = extract_index_key_value(&dummy_row, &tcols) {
                        let key = value_to_index_key(&idx_val);
                        let mut matched = Vec::new();
                        if let Some(bucket) =
                            IndexBucket::load(&self.btree, pager, index_def.root_page, key)?
                            && let Some(rids) = bucket.get_row_ids(&idx_val)
                        {
                            for &rid in rids {
                                if let Some(payload) = self.btree.search(pager, root_page, rid)? {
                                    let row = decode_row_shared(&payload, &shared_cols)?;
                                    if resolved_where.is_none_or(|r| row_matches_resolved(&row, r))
                                    {
                                        matched.push((rid, row));
                                    }
                                }
                            }
                        }
                        return Ok(matched);
                    }
                    // `col = NULL` never matches rows under ANSI SQL three-valued logic.
                    return Ok(Vec::new());
                }
            }
        }

        // 3. Streaming fallback with projected WHERE evaluation when all filter columns are direct.
        let mut matched = Vec::new();
        if let Some(r_expr) = resolved_where {
            let mut filter_mask = vec![false; table_def.columns.len()];
            if mark_resolved_where_columns(r_expr, table_def, &mut filter_mask) {
                let mut row_vals = Vec::with_capacity(table_def.columns.len());
                self.btree
                    .scan_with_ref(pager, root_page, |row_id, payload| {
                        decode_row_values_into_projected(payload, &filter_mask, &mut row_vals)?;
                        if slice_matches_resolved(&row_vals, table_def, r_expr) {
                            let full_row = decode_row_shared(payload, &shared_cols)?;
                            matched.push((row_id, full_row));
                        }
                        Ok(true)
                    })?;
            } else {
                self.btree
                    .scan_with_ref(pager, root_page, |row_id, payload| {
                        let full_row = decode_row_shared(payload, &shared_cols)?;
                        if row_matches_resolved(&full_row, r_expr) {
                            matched.push((row_id, full_row));
                        }
                        Ok(true)
                    })?;
            }
        } else {
            self.btree
                .scan_with_ref(pager, root_page, |row_id, payload| {
                    let full_row = decode_row_shared(payload, &shared_cols)?;
                    matched.push((row_id, full_row));
                    Ok(true)
                })?;
        }
        Ok(matched)
    }

    pub(super) fn execute_update(
        &mut self,
        pager: &mut Pager,
        table: String,
        assignments: Vec<(String, Value)>,
        assignment_exprs: Vec<(String, String)>,
        where_clause: Option<WhereExpr>,
        returning: Option<Vec<String>>,
    ) -> Result<(usize, Vec<Row>)> {
        let table_def = self
            .catalog
            .get_table(&table)
            .cloned()
            .ok_or_else(|| Error::TableNotFound(table.clone()))?;

        let root_page = table_def.root_page;
        let all_col_names = table_def.column_names();
        let shared_col_names = Arc::new(all_col_names);

        // Pre-resolve assignment column indices once and reject writes to generated columns
        let mut resolved_assignments = Vec::with_capacity(assignments.len());
        for (col_name, val) in assignments {
            let idx = table_def
                .column_index(&col_name)
                .ok_or_else(|| Error::Corrupted(format!("Unknown column in SET: {col_name}")))?;
            if table_def.columns[idx].generated_expr.is_some() {
                return Err(Error::ConstraintViolation(format!(
                    "Cannot explicitly update generated column '{table}.{}'",
                    table_def.columns[idx].name
                )));
            }
            resolved_assignments.push((idx, val));
        }

        let mut resolved_expr_indices = Vec::with_capacity(assignment_exprs.len());
        let mut raw_expr_cols = Vec::with_capacity(assignment_exprs.len());
        for (col_name, expr_sql) in assignment_exprs {
            let idx = table_def
                .column_index(&col_name)
                .ok_or_else(|| Error::Corrupted(format!("Unknown column in SET: {col_name}")))?;
            if table_def.columns[idx].generated_expr.is_some() {
                return Err(Error::ConstraintViolation(format!(
                    "Cannot explicitly update generated column '{table}.{}'",
                    table_def.columns[idx].name
                )));
            }
            resolved_expr_indices.push(idx);
            raw_expr_cols.push(expr_sql);
        }

        let expr_plan = if raw_expr_cols.is_empty() {
            Vec::new()
        } else {
            self.prepare_projection_plan(pager, &raw_expr_cols, &[&table_def])?
        };
        let mut expr_memo = CorrelatedSubqueryMemo::default();

        let resolved_where = if let Some(ref expr) = where_clause {
            Some(self.resolve_where_expr_for_tables(pager, expr, &[&table_def])?)
        } else {
            None
        };

        let matched_rows = self.collect_matching_rows_for_dml(
            pager,
            &table_def,
            where_clause.as_ref(),
            resolved_where.as_ref(),
        )?;
        let mut updated_count = 0;
        let mut returned_rows = Vec::new();
        let mut encode_buf = Vec::with_capacity(256);

        for (cell_row_id, full_row) in matched_rows {
            {
                let mut values: Vec<Value> = full_row.values().to_vec();
                for &(col_idx, ref new_val) in &resolved_assignments {
                    if col_idx < values.len() {
                        values[col_idx] = new_val.clone();
                    }
                }
                for (plan_idx, col_plan) in expr_plan.iter().enumerate() {
                    let target_idx = resolved_expr_indices[plan_idx];
                    if target_idx < values.len() {
                        values[target_idx] = self.eval_prepared_projection_col(
                            pager,
                            &full_row,
                            col_plan,
                            &[&table_def],
                            &mut expr_memo,
                        )?;
                    }
                }

                // Enforce column data type validation and coercion for assigned columns, then compute generated columns
                for (col_idx, col_def) in table_def.columns.iter().enumerate() {
                    if col_def.generated_expr.is_some() {
                        continue;
                    }
                    if let Some(val) = values.get_mut(col_idx) {
                        let orig = std::mem::replace(val, Value::Null);
                        *val = col_def.data_type.coerce_value(orig).map_err(|e| match e {
                            Error::DimensionMismatch(exp, got) => {
                                Error::DimensionMismatch(exp, got)
                            }
                            Error::ConstraintViolation(msg) => Error::ConstraintViolation(format!(
                                "{table}.{}: {msg}",
                                col_def.name
                            )),
                            other => other,
                        })?;
                    }
                }
                self.compute_generated_columns(&table, &table_def.columns, &mut values)?;

                // Enforce NOT NULL constraints
                for (col_idx, col_def) in table_def.columns.iter().enumerate() {
                    if col_def.not_null
                        && let Some(val) = values.get(col_idx)
                        && val.is_null()
                    {
                        return Err(Error::ConstraintViolation(format!(
                            "NOT NULL constraint failed: {table}.{}",
                            col_def.name
                        )));
                    }
                }

                let pk_col_idx = table_def.primary_key_index();
                let new_row_id = if let Some(pk_idx) = pk_col_idx {
                    if let Some(Value::Integer(id_val)) = values.get(pk_idx) {
                        *id_val as u64
                    } else {
                        cell_row_id
                    }
                } else {
                    cell_row_id
                };

                // If primary key was modified to a different ID, ensure it does not collide
                if new_row_id != cell_row_id
                    && let Some(_existing) = self.btree.search(pager, root_page, new_row_id)?
                {
                    let pk_col_name = table_def
                        .columns
                        .iter()
                        .find(|c| c.primary_key)
                        .map(|c| c.name.as_str())
                        .unwrap_or("id");
                    return Err(Error::ConstraintViolation(format!(
                        "UNIQUE constraint failed: {table}.{pk_col_name} (key {new_row_id} already exists)"
                    )));
                }

                let updated_row =
                    Row::with_shared_columns(Arc::clone(&shared_col_names), values.clone());
                if !self.fire_triggers(
                    pager,
                    &table,
                    TriggerTiming::Before,
                    TriggerEvent::Update,
                    Some(&full_row),
                    Some(&updated_row),
                )? {
                    continue;
                }

                self.validate_check_constraints_for_row(&table_def, &updated_row)?;
                self.validate_unique_constraints(
                    pager,
                    &table_def,
                    &updated_row,
                    Some(cell_row_id),
                )?;
                self.validate_child_foreign_keys(pager, &table_def, &updated_row)?;

                encode_row_into(&values, &mut encode_buf);
                self.btree.delete(pager, root_page, cell_row_id)?;
                self.btree
                    .insert(pager, root_page, new_row_id, &encode_buf)?;

                self.handle_parent_foreign_key_on_update(pager, &table, &full_row, &updated_row)?;

                // Update temporal snapshot for time-travel queries if enabled
                if table_def.system_versioning || self.default_system_versioning {
                    let now_ts = self.next_temporal_timestamp();
                    self.close_temporal_version(pager, &table, cell_row_id, now_ts)?;
                    self.record_temporal_version(
                        pager,
                        &table,
                        new_row_id,
                        &encode_buf,
                        now_ts,
                        u64::MAX,
                    )?;
                }

                // Update vector index if vector column was updated or row_id changed
                if let Some((v_idx, _)) = table_def.vector_column() {
                    if let Some(Value::Vector(vec_data)) = values.get(v_idx) {
                        let index = self
                            .vector_indexes
                            .entry(table.to_lowercase())
                            .or_insert_with(|| {
                                HnswIndex::new(vec_data.len(), DistanceMetric::Cosine)
                            });
                        index.remove_vector(cell_row_id);
                        index.insert_vector(new_row_id, vec_data)?;
                    } else if new_row_id != cell_row_id
                        && let Some(index) = self.vector_indexes.get_mut(&table.to_lowercase())
                    {
                        index.remove_vector(cell_row_id);
                    }
                }

                // Update secondary indexes & FTS5 index
                self.remove_row_from_all_indexes(pager, &table, &full_row, cell_row_id)?;
                self.add_row_to_all_indexes(pager, &table, &updated_row, new_row_id)?;
                if new_row_id != cell_row_id {
                    self.fts_remove_row(&table_def, cell_row_id);
                }
                self.fts_upsert_row(pager, &table_def, &updated_row, new_row_id)?;

                if new_row_id >= table_def.next_row_id
                    && let Some(tdef_mut) = self.catalog.get_table_mut(&table)
                {
                    tdef_mut.next_row_id = new_row_id + 1;
                }

                self.fire_triggers(
                    pager,
                    &table,
                    TriggerTiming::After,
                    TriggerEvent::Update,
                    Some(&full_row),
                    Some(&updated_row),
                )?;

                if let Some(ref ret_cols) = returning {
                    if ret_cols.len() == 1 && ret_cols[0] == "*" {
                        returned_rows.push(updated_row);
                    } else {
                        returned_rows.push(self.project_row(&updated_row, ret_cols)?);
                    }
                }

                updated_count += 1;
            }
        }

        Ok((updated_count, returned_rows))
    }

    pub(super) fn execute_delete(
        &mut self,
        pager: &mut Pager,
        table: String,
        where_clause: Option<WhereExpr>,
        returning: Option<Vec<String>>,
    ) -> Result<(usize, Vec<Row>)> {
        let table_def = self
            .catalog
            .get_table(&table)
            .cloned()
            .ok_or_else(|| Error::TableNotFound(table.clone()))?;

        let root_page = table_def.root_page;
        let resolved_where = if let Some(ref expr) = where_clause {
            Some(self.resolve_where_expr_for_tables(pager, expr, &[&table_def])?)
        } else {
            None
        };

        let matched_rows = self.collect_matching_rows_for_dml(
            pager,
            &table_def,
            where_clause.as_ref(),
            resolved_where.as_ref(),
        )?;
        let mut deleted_count = 0;
        let mut returned_rows = Vec::new();

        for (cell_row_id, full_row) in matched_rows {
            if !self.fire_triggers(
                pager,
                &table,
                TriggerTiming::Before,
                TriggerEvent::Delete,
                Some(&full_row),
                None,
            )? {
                continue;
            }

            self.handle_parent_foreign_key_on_delete(pager, &table, &full_row)?;

            if let Some(ref ret_cols) = returning {
                if ret_cols.len() == 1 && ret_cols[0] == "*" {
                    returned_rows.push(full_row.clone());
                } else {
                    returned_rows.push(self.project_row(&full_row, ret_cols)?);
                }
            }

            self.btree.delete(pager, root_page, cell_row_id)?;
            if table_def.system_versioning || self.default_system_versioning {
                let now_ts = self.next_temporal_timestamp();
                self.close_temporal_version(pager, &table, cell_row_id, now_ts)?;
            }
            if let Some(index) = self.vector_indexes.get_mut(&table.to_lowercase()) {
                index.remove_vector(cell_row_id);
            }
            self.fts_remove_row(&table_def, cell_row_id);
            self.remove_row_from_all_indexes(pager, &table, &full_row, cell_row_id)?;

            self.fire_triggers(
                pager,
                &table,
                TriggerTiming::After,
                TriggerEvent::Delete,
                Some(&full_row),
                None,
            )?;
            deleted_count += 1;
        }

        Ok((deleted_count, returned_rows))
    }
}
