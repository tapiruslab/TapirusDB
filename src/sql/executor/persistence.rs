#[allow(unused_imports)]
use super::*;

impl SQLExecutor {
    /// Project a relational SQL table as graph nodes into the Knowledge Graph
    pub fn project_table_as_nodes(
        &mut self,
        pager: &mut Pager,
        table_name: &str,
        id_col: &str,
        label_col: Option<&str>,
        prop_cols: Option<&[&str]>,
    ) -> Result<usize> {
        let rows = self.query(
            pager,
            Statement::Select {
                distinct: false,
                columns: Vec::new(),
                table: table_name.to_string(),
                from_subquery: None,
                join: None,
                extra_joins: Vec::new(),
                where_clause: None,
                group_by: None,
                having: None,
                order_by: Vec::new(),
                limit: None,
                offset: None,
                as_of_timestamp: None,
            },
        )?;

        let mut count = 0;
        for row in rows {
            let id_val = row
                .get_value(id_col)
                .ok_or_else(|| Error::ColumnNotFound(id_col.to_string()))?;
            let id = match id_val {
                Value::Integer(i) => *i as u64,
                Value::Text(s) => s
                    .parse::<u64>()
                    .unwrap_or_else(|_| crate::stable_hash::hash_text(s)),
                other => {
                    return Err(Error::ConstraintViolation(format!(
                        "Invalid ID type '{other:?}' for graph node projection"
                    )));
                }
            };

            let label = if let Some(lc) = label_col {
                row.get_value(lc)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| table_name.to_string())
            } else {
                table_name.to_string()
            };

            let mut props_map = serde_json::Map::new();
            if let Some(cols) = prop_cols {
                for &col in cols.iter() {
                    if let Some(v) = row.get_value(col) {
                        props_map.insert(col.to_string(), serde_json::json!(v.to_string()));
                    }
                }
            } else {
                for (name, val) in row.columns().iter().zip(row.values().iter()) {
                    if name != id_col && label_col.map(|lc| lc != name).unwrap_or(true) {
                        props_map.insert(name.clone(), serde_json::json!(val.to_string()));
                    }
                }
            }
            let props_json = serde_json::Value::Object(props_map).to_string();

            let vector = row.values().iter().find_map(|v| {
                if let Value::Vector(vec) = v {
                    Some(vec.clone())
                } else {
                    None
                }
            });

            let node = crate::graph::Node {
                id,
                label,
                properties: props_json,
                vector,
            };
            self.graph.restore_node(node.clone());
            self.persist_graph_node(pager, &node)?;
            count += 1;
        }

        Ok(count)
    }

    /// Project a relational SQL table as graph edges into the Knowledge Graph
    #[allow(
        clippy::too_many_arguments,
        reason = "Preserve the public graph projection API"
    )]
    pub fn project_table_as_edges(
        &mut self,
        pager: &mut Pager,
        table_name: &str,
        from_col: &str,
        to_col: &str,
        label_col: Option<&str>,
        weight_col: Option<&str>,
        prop_cols: Option<&[&str]>,
    ) -> Result<usize> {
        let rows = self.query(
            pager,
            Statement::Select {
                distinct: false,
                columns: Vec::new(),
                table: table_name.to_string(),
                from_subquery: None,
                join: None,
                extra_joins: Vec::new(),
                where_clause: None,
                group_by: None,
                having: None,
                order_by: Vec::new(),
                limit: None,
                offset: None,
                as_of_timestamp: None,
            },
        )?;

        let mut count = 0;
        for row in rows {
            let from_val = row
                .get_value(from_col)
                .ok_or_else(|| Error::ColumnNotFound(from_col.to_string()))?;
            let to_val = row
                .get_value(to_col)
                .ok_or_else(|| Error::ColumnNotFound(to_col.to_string()))?;

            let from_id = match from_val {
                Value::Integer(i) => *i as u64,
                Value::Text(s) => s
                    .parse::<u64>()
                    .unwrap_or_else(|_| crate::stable_hash::hash_text(s)),
                _ => return Err(Error::ConstraintViolation("Invalid from_id type".into())),
            };

            let to_id = match to_val {
                Value::Integer(i) => *i as u64,
                Value::Text(s) => s
                    .parse::<u64>()
                    .unwrap_or_else(|_| crate::stable_hash::hash_text(s)),
                _ => return Err(Error::ConstraintViolation("Invalid to_id type".into())),
            };

            let label = if let Some(lc) = label_col {
                row.get_value(lc)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "RELATION".to_string())
            } else {
                "RELATION".to_string()
            };

            let weight = if let Some(wc) = weight_col {
                match row.get_value(wc) {
                    Some(Value::Real(r)) => *r as f32,
                    Some(Value::Integer(i)) => *i as f32,
                    _ => 1.0,
                }
            } else {
                1.0
            };

            let mut props_map = serde_json::Map::new();
            if let Some(cols) = prop_cols {
                for &col in cols.iter() {
                    if let Some(v) = row.get_value(col) {
                        props_map.insert(col.to_string(), serde_json::json!(v.to_string()));
                    }
                }
            } else {
                for (name, val) in row.columns().iter().zip(row.values().iter()) {
                    if name != from_col && name != to_col {
                        props_map.insert(name.clone(), serde_json::json!(val.to_string()));
                    }
                }
            }
            let props_json = serde_json::Value::Object(props_map).to_string();

            let edge_id = self
                .graph
                .add_edge(from_id, to_id, label, weight, props_json)?;
            if let Some(edge) = self.graph.get_edge(edge_id) {
                let edge_clone = edge.clone();
                self.persist_graph_edge(pager, &edge_clone)?;
            }
            count += 1;
        }

        Ok(count)
    }

    /// Return reference to all active in-memory HNSW vector indexes
    pub fn vector_indexes(&self) -> &HashMap<String, HnswIndex> {
        &self.vector_indexes
    }

    /// Return mutable reference to in-memory HNSW vector indexes
    pub fn vector_indexes_mut(&mut self) -> &mut HashMap<String, HnswIndex> {
        &mut self.vector_indexes
    }

    /// Set or update the HNSW index for a specific table with custom configuration
    pub fn set_vector_index(&mut self, table: &str, index: HnswIndex) {
        self.vector_indexes.insert(table.to_lowercase(), index);
    }

    /// Return all table definitions
    pub fn tables(&self) -> Vec<TableDef> {
        self.catalog.tables()
    }

    /// Return table definition by name
    pub fn get_table(&self, name: &str) -> Option<&TableDef> {
        self.catalog.get_table(name)
    }

    /// Return all view definitions
    pub fn views(&self) -> Vec<crate::sql::catalog::ViewDef> {
        self.catalog.views()
    }

    /// Return view definition by name
    pub fn get_view(&self, name: &str) -> Option<&crate::sql::catalog::ViewDef> {
        self.catalog.get_view(name)
    }

    /// Ensure an internal system table exists and return its root page
    pub fn ensure_system_table(
        &mut self,
        pager: &mut Pager,
        name: &str,
        columns: Vec<ColumnDef>,
    ) -> Result<u32> {
        let name_lower = name.to_lowercase();
        if let Some(table) = self.catalog.get_table(&name_lower) {
            return Ok(table.root_page);
        }

        let root_page = pager.allocate_page()?;
        let mut root_buf = vec![0u8; pager.page_size()];
        engine::init_leaf_page(&mut root_buf, root_page);
        pager.write_page(root_page, &root_buf)?;

        let table_def = TableDef::new(name.to_string(), root_page, columns);
        self.catalog.create_table(pager, table_def)?;
        Ok(root_page)
    }

    /// Persist a Knowledge Graph Node to B+Tree disk storage
    pub fn persist_graph_node(
        &mut self,
        pager: &mut Pager,
        node: &crate::graph::Node,
    ) -> Result<()> {
        let cols = vec![
            ColumnDef::new("id", DataType::Integer).primary_key(),
            ColumnDef::new("payload", DataType::Text),
        ];
        let root_page = self.ensure_system_table(pager, "__sys_graph_nodes", cols)?;
        let payload = serde_json::to_vec(node)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize graph node: {e}")))?;
        self.btree.insert(pager, root_page, node.id, &payload)
    }

    /// Helper to load a node's incident edge adjacency lists from __sys_graph_adj table
    pub fn load_graph_node_adjacency(
        &mut self,
        pager: &mut Pager,
        adj_root: u32,
        node_id: u64,
    ) -> Result<crate::graph::GraphAdjacencyRecord> {
        if let Some(payload) = self.btree.search(pager, adj_root, node_id)? {
            serde_json::from_slice::<crate::graph::GraphAdjacencyRecord>(&payload).map_err(|e| {
                Error::Corrupted(format!(
                    "Failed to deserialize adjacency for node #{node_id}: {e}"
                ))
            })
        } else {
            Ok(crate::graph::GraphAdjacencyRecord::default())
        }
    }

    /// Persist a Knowledge Graph Edge to B+Tree disk storage and update adjacency index
    pub fn persist_graph_edge(
        &mut self,
        pager: &mut Pager,
        edge: &crate::graph::Edge,
    ) -> Result<()> {
        let cols = vec![
            ColumnDef::new("id", DataType::Integer).primary_key(),
            ColumnDef::new("payload", DataType::Text),
        ];
        let root_page = self.ensure_system_table(pager, "__sys_graph_edges", cols)?;
        let payload = serde_json::to_vec(edge)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize graph edge: {e}")))?;
        self.btree.insert(pager, root_page, edge.id, &payload)?;

        // Maintain __sys_graph_adj table for scalable on-disk neighbor traversal
        let adj_cols = vec![
            ColumnDef::new("id", DataType::Integer).primary_key(),
            ColumnDef::new("payload", DataType::Text),
        ];
        let adj_root = self.ensure_system_table(pager, "__sys_graph_adj", adj_cols)?;

        // Update outgoing adjacency for source node (edge.from_id)
        let mut from_adj = self.load_graph_node_adjacency(pager, adj_root, edge.from_id)?;
        if !from_adj.outgoing_edges.contains(&edge.id) {
            from_adj.outgoing_edges.push(edge.id);
            let from_bytes = serde_json::to_vec(&from_adj).map_err(|e| {
                Error::Corrupted(format!("Failed to serialize node #{}: {e}", edge.from_id))
            })?;
            self.btree
                .insert(pager, adj_root, edge.from_id, &from_bytes)?;
        }

        // Update incoming adjacency for target node (edge.to_id)
        let mut to_adj = self.load_graph_node_adjacency(pager, adj_root, edge.to_id)?;
        if !to_adj.incoming_edges.contains(&edge.id) {
            to_adj.incoming_edges.push(edge.id);
            let to_bytes = serde_json::to_vec(&to_adj).map_err(|e| {
                Error::Corrupted(format!("Failed to serialize node #{}: {e}", edge.to_id))
            })?;
            self.btree.insert(pager, adj_root, edge.to_id, &to_bytes)?;
        }

        Ok(())
    }

    /// Delete a Knowledge Graph Node from B+Tree disk storage and clean up its adjacency
    pub fn delete_graph_node(&mut self, pager: &mut Pager, id: u64) -> Result<()> {
        if let Some(table) = self.catalog.get_table("__sys_graph_nodes") {
            let _ = self.btree.delete(pager, table.root_page, id);
        }
        if let Some(adj_table) = self.catalog.get_table("__sys_graph_adj") {
            let _ = self.btree.delete(pager, adj_table.root_page, id);
        }
        Ok(())
    }

    /// Delete a Knowledge Graph Edge from B+Tree disk storage and update incident adjacency lists
    pub fn delete_graph_edge(&mut self, pager: &mut Pager, id: u64) -> Result<()> {
        let edges_root = match self.catalog.get_table("__sys_graph_edges") {
            Some(t) => t.root_page,
            None => return Ok(()),
        };
        let edge_opt = if let Some(bytes) = self.btree.search(pager, edges_root, id)? {
            serde_json::from_slice::<crate::graph::Edge>(&bytes).ok()
        } else {
            None
        };

        if let Some(edge) = edge_opt {
            let _ = self.btree.delete(pager, edges_root, id);

            if let Some(adj_table) = self.catalog.get_table("__sys_graph_adj") {
                let adj_root = adj_table.root_page;
                // Remove from from_id
                let mut from_adj = self.load_graph_node_adjacency(pager, adj_root, edge.from_id)?;
                if from_adj.outgoing_edges.contains(&id) {
                    from_adj.outgoing_edges.retain(|&e| e != id);
                    let bytes = serde_json::to_vec(&from_adj).map_err(|e| {
                        Error::Corrupted(format!("Failed to serialize node #{}: {e}", edge.from_id))
                    })?;
                    self.btree.insert(pager, adj_root, edge.from_id, &bytes)?;
                }
                // Remove from to_id
                let mut to_adj = self.load_graph_node_adjacency(pager, adj_root, edge.to_id)?;
                if to_adj.incoming_edges.contains(&id) {
                    to_adj.incoming_edges.retain(|&e| e != id);
                    let bytes = serde_json::to_vec(&to_adj).map_err(|e| {
                        Error::Corrupted(format!("Failed to serialize node #{}: {e}", edge.to_id))
                    })?;
                    self.btree.insert(pager, adj_root, edge.to_id, &bytes)?;
                }
            }
        }
        Ok(())
    }

    /// Retrieve adjacent neighbor (Node, Edge) pairs directly from B+Tree disk storage
    /// with O(d) page reads without requiring the entire graph topology to be loaded in RAM.
    pub fn get_disk_graph_neighbors(
        &mut self,
        pager: &mut Pager,
        node_id: u64,
        direction: crate::graph::Direction,
        edge_label: Option<&str>,
    ) -> Result<Vec<(crate::graph::Node, crate::graph::Edge)>> {
        let adj_root = match self.catalog.get_table("__sys_graph_adj") {
            Some(t) => t.root_page,
            None => return Ok(Vec::new()),
        };
        let edges_root = match self.catalog.get_table("__sys_graph_edges") {
            Some(t) => t.root_page,
            None => return Ok(Vec::new()),
        };
        let nodes_root = match self.catalog.get_table("__sys_graph_nodes") {
            Some(t) => t.root_page,
            None => return Ok(Vec::new()),
        };

        let adj = self.load_graph_node_adjacency(pager, adj_root, node_id)?;
        let mut candidate_edges = Vec::new();

        match direction {
            crate::graph::Direction::Outgoing => {
                candidate_edges.extend(adj.outgoing_edges);
            }
            crate::graph::Direction::Incoming => {
                candidate_edges.extend(adj.incoming_edges);
            }
            crate::graph::Direction::Both => {
                candidate_edges.extend(adj.outgoing_edges);
                for e in adj.incoming_edges {
                    if !candidate_edges.contains(&e) {
                        candidate_edges.push(e);
                    }
                }
            }
        }

        let mut results = Vec::new();
        for eid in candidate_edges {
            if let Some(edge_bytes) = self.btree.search(pager, edges_root, eid)?
                && let Ok(edge) = serde_json::from_slice::<crate::graph::Edge>(&edge_bytes)
            {
                if let Some(filter_label) = edge_label
                    && !edge.label.eq_ignore_ascii_case(filter_label)
                {
                    continue;
                }

                let neighbor_id = if edge.from_id == node_id {
                    edge.to_id
                } else {
                    edge.from_id
                };

                if let Some(node_bytes) = self.btree.search(pager, nodes_root, neighbor_id)?
                    && let Ok(node) = serde_json::from_slice::<crate::graph::Node>(&node_bytes)
                {
                    results.push((node, edge));
                }
            }
        }

        Ok(results)
    }

    /// Load persisted graph nodes and edges from B+Tree disk storage
    pub fn load_graph_from_disk(&mut self, pager: &mut Pager) -> Result<()> {
        if let Some(table) = self.catalog.get_table("__sys_graph_nodes") {
            let root_page = table.root_page;
            let btree = &self.btree;
            let graph = &mut self.graph;
            btree.scan_with_ref(pager, root_page, |_row_id, payload| {
                if let Ok(node) = serde_json::from_slice::<crate::graph::Node>(payload) {
                    graph.restore_node(node);
                }
                Ok(true)
            })?;
        }

        if let Some(table) = self.catalog.get_table("__sys_graph_edges") {
            let root_page = table.root_page;
            let btree = &self.btree;
            let graph = &mut self.graph;
            btree.scan_with_ref(pager, root_page, |_row_id, payload| {
                if let Ok(edge) = serde_json::from_slice::<crate::graph::Edge>(payload) {
                    graph.restore_edge(edge);
                }
                Ok(true)
            })?;
        }

        Ok(())
    }

    /// Persist an AI Agent MemoryEntry to B+Tree disk storage in compact binary format
    pub fn persist_memory_entry(
        &mut self,
        pager: &mut Pager,
        entry: &crate::memory::MemoryEntry,
    ) -> Result<()> {
        let cols = vec![
            ColumnDef::new("id", DataType::Integer).primary_key(),
            ColumnDef::new("payload", DataType::Blob),
        ];
        let root_page = self.ensure_system_table(pager, "__sys_memory", cols)?;
        let payload = crate::memory::binary::encode_memory_entry_binary(entry)?;
        self.btree.insert(pager, root_page, entry.id, &payload)
    }

    /// Delete an AI Agent MemoryEntry from B+Tree disk storage
    pub fn delete_memory_entry(&mut self, pager: &mut Pager, id: u64) -> Result<bool> {
        if let Some(table) = self.catalog.get_table("__sys_memory") {
            let root_page = table.root_page;
            self.btree.delete(pager, root_page, id)
        } else {
            Ok(false)
        }
    }

    /// Load persisted memory entries from B+Tree disk storage into MemoryEngine,
    /// restoring any valid persisted HNSW topology cache from `__sys_memory_index`.
    pub fn load_memory_from_disk(
        &self,
        pager: &mut Pager,
        memory_engine: &mut crate::memory::MemoryEngine,
    ) -> Result<()> {
        if let Some(table) = self.catalog.get_table("__sys_memory") {
            let root_page = table.root_page;
            self.btree.scan_with_ref(pager, root_page, |id, payload| {
                let entry = crate::memory::binary::decode_memory_entry_auto(id, payload)?;
                if entry.id != id {
                    return Err(Error::Corrupted(format!(
                        "Memory record key {id} does not match payload ID {}",
                        entry.id
                    )));
                }
                memory_engine.restore_entry(entry)?;
                Ok(true)
            })?;
        }
        if let Some(idx_table) = self.catalog.get_table("__sys_memory_index")
            && let Ok(Some(payload)) = self.btree.search(pager, idx_table.root_page, 1)
        {
            // Derived HNSW cache is best-effort: fall back cleanly to lazy rebuild if corrupted/stale.
            let _ = memory_engine.import_cached_indexes(&payload);
        }
        Ok(())
    }

    /// Persist AI Memory HNSW index cache to `__sys_memory_index` when dirty or newly populated.
    pub fn persist_memory_index_to_disk(
        &mut self,
        pager: &mut Pager,
        memory_engine: &mut crate::memory::MemoryEngine,
    ) -> Result<()> {
        if memory_engine.stats().documents == 0 && !self.catalog.table_exists("__sys_memory_index")
        {
            return Ok(());
        }
        if let Some(payload) = memory_engine.export_cached_indexes()? {
            let cols = vec![
                ColumnDef::new("id", DataType::Integer).primary_key(),
                ColumnDef::new("payload", DataType::Blob),
            ];
            let root_page = self.ensure_system_table(pager, "__sys_memory_index", cols)?;
            self.btree.insert(pager, root_page, 1, &payload)?;
            memory_engine.mark_index_cache_synced();
        }
        Ok(())
    }

    /// Return monotonic timestamp in seconds (or sequential increment if within same second)
    pub fn next_temporal_timestamp(&mut self) -> u64 {
        let now = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let ts = if now <= self.last_temporal_timestamp {
            self.last_temporal_timestamp + 1
        } else {
            now
        };
        self.last_temporal_timestamp = ts;
        ts
    }

    /// Record a new temporal version in `__sys_time_travel`
    pub fn record_temporal_version(
        &mut self,
        pager: &mut Pager,
        table: &str,
        row_id: u64,
        payload: &[u8],
        valid_from: u64,
        valid_to: u64,
    ) -> Result<()> {
        if table.starts_with("__sys_") {
            return Ok(());
        }
        let cols = vec![
            ColumnDef::new("id", DataType::Integer).primary_key(),
            ColumnDef::new("payload", DataType::Blob),
        ];
        let root_page = self.ensure_system_table(pager, "__sys_time_travel", cols)?;
        let version_id = {
            let tdef = self
                .catalog
                .get_table_mut("__sys_time_travel")
                .ok_or_else(|| Error::TableNotFound("__sys_time_travel".into()))?;
            let vid = tdef.next_row_id;
            tdef.next_row_id += 1;
            vid
        };
        let record = TemporalRecord {
            id: version_id,
            table_name: table.to_string(),
            row_id,
            valid_from,
            valid_to,
            payload: payload.to_vec(),
        };
        let rec_payload = serde_json::to_vec(&record)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize temporal record: {e}")))?;
        self.btree
            .insert(pager, root_page, version_id, &rec_payload)?;
        Ok(())
    }

    /// Close the active temporal version for a given table and row_id at `end_time`
    pub fn close_temporal_version(
        &mut self,
        pager: &mut Pager,
        table: &str,
        row_id: u64,
        end_time: u64,
    ) -> Result<()> {
        if table.starts_with("__sys_") {
            return Ok(());
        }
        if let Some(tdef) = self.catalog.get_table("__sys_time_travel") {
            let root_page = tdef.root_page;
            let mut match_update: Option<(u64, Vec<u8>)> = None;
            self.btree
                .scan_with_ref(pager, root_page, |cell_row_id, payload| {
                    if let Ok(mut record) = serde_json::from_slice::<TemporalRecord>(payload)
                        && record.table_name.eq_ignore_ascii_case(table)
                        && record.row_id == row_id
                        && record.valid_to == u64::MAX
                    {
                        record.valid_to = end_time;
                        let updated_payload = serde_json::to_vec(&record).map_err(|e| {
                            Error::Corrupted(format!("Failed to serialize temporal record: {e}"))
                        })?;
                        match_update = Some((cell_row_id, updated_payload));
                        return Ok(false);
                    }
                    Ok(true)
                })?;
            if let Some((cell_row_id, updated_payload)) = match_update {
                self.btree
                    .insert(pager, root_page, cell_row_id, &updated_payload)?;
            }
        }
        Ok(())
    }

    /// Persist all active in-memory HNSW vector index topologies into `__sys_vector_snapshots`
    pub fn persist_vector_indexes_to_disk(&mut self, pager: &mut Pager) -> Result<()> {
        if self.vector_indexes.is_empty() {
            return Ok(());
        }
        let cols = vec![
            ColumnDef::new("id", DataType::Integer).primary_key(),
            ColumnDef::new("table_name", DataType::Text),
            ColumnDef::new("snapshot", DataType::Blob),
        ];
        let root_page = self.ensure_system_table(pager, "__sys_vector_snapshots", cols)?;
        pager.set_vector_index_page(root_page as u32)?;

        for (table_name, index) in &self.vector_indexes {
            let key = crate::stable_hash::key_for_name(table_name);
            let record = VectorSnapshotRecord {
                table_name: table_name.clone(),
                snapshot: index.export_snapshot(),
                table_fingerprint: Some(self.table_fingerprint(pager, table_name)?),
            };
            let payload = serde_json::to_vec(&record)
                .map_err(|e| Error::Corrupted(format!("Failed to serialize HNSW snapshot: {e}")))?;
            self.btree.insert(pager, root_page, key, &payload)?;
        }
        Ok(())
    }

    /// Automatically warm up and restore in-memory HNSW vector indexes from persisted snapshot or B+Tree tables
    pub(super) fn table_fingerprint(&self, pager: &mut Pager, table: &str) -> Result<u32> {
        let definition = self
            .catalog
            .get_table(table)
            .ok_or_else(|| Error::TableNotFound(table.into()))?;
        let mut hash = crc32fast::Hasher::new();
        self.btree
            .scan_with_ref(pager, definition.root_page, |row_id, payload| {
                hash.update(&row_id.to_le_bytes());
                hash.update(payload);
                Ok(true)
            })?;
        Ok(hash.finalize())
    }

    /// Restore snapshots only when their source rows still match.
    pub fn load_vector_indexes_from_disk(&mut self, pager: &mut Pager) -> Result<()> {
        self.vector_indexes.clear();
        let mut loaded_tables = HashSet::new();

        // 1. Attempt instantaneous O(N) restoration from persistent topology snapshot or header pointer
        let snap_root = self
            .catalog
            .get_table("__sys_vector_snapshots")
            .map(|t| t.root_page)
            .or_else(|| {
                let p = pager.vector_index_page();
                if p != 0 { Some(p) } else { None }
            });
        if let Some(root_page) = snap_root {
            let mut snapshots = Vec::new();
            if self
                .btree
                .scan_with_ref(pager, root_page, |_row_id, payload| {
                    if let Ok(rec) = serde_json::from_slice::<VectorSnapshotRecord>(payload) {
                        snapshots.push(rec);
                    }
                    Ok(true)
                })
                .is_ok()
            {
                for rec in snapshots {
                    if !self.catalog.table_exists(&rec.table_name)
                        || rec.table_fingerprint
                            != Some(self.table_fingerprint(pager, &rec.table_name)?)
                    {
                        continue;
                    }
                    let index = HnswIndex::import_snapshot(rec.snapshot);
                    self.vector_indexes
                        .insert(rec.table_name.to_lowercase(), index);
                    loaded_tables.insert(rec.table_name.to_lowercase());
                }
            }
        }

        // 2. Fallback: for any vector table lacking a persistent snapshot, construct from rows
        let missing_vector_tables: Vec<_> = self
            .catalog
            .tables()
            .into_iter()
            .filter_map(|table| {
                let tname = table.name.to_lowercase();
                if !loaded_tables.contains(&tname)
                    && let Some((vec_idx, dims)) = table.vector_column()
                {
                    Some((tname, table.root_page, vec_idx, dims, table.column_names()))
                } else {
                    None
                }
            })
            .collect();
        for (tname, tbl_root, vec_idx, dims, all_col_names) in missing_vector_tables {
            let mut index = HnswIndex::new(dims, DistanceMetric::Cosine);
            self.btree
                .scan_with_ref(pager, tbl_root, |row_id, payload| {
                    if let Ok(row) = decode_row(payload, &all_col_names)
                        && let Some(crate::traits::Value::Vector(vec)) = row.values().get(vec_idx)
                    {
                        index.insert_vector(row_id, vec)?;
                    }
                    Ok(true)
                })?;
            self.vector_indexes.insert(tname, index);
        }
        Ok(())
    }

    /// Persist a disk-backed PagedHnswIndex into dedicated B+Tree system tables
    pub fn persist_paged_vector_index(
        &mut self,
        pager: &mut Pager,
        table_name: &str,
        paged_index: &crate::vector::PagedHnswIndex,
    ) -> Result<()> {
        let meta_cols = vec![
            ColumnDef::new("id", DataType::Integer).primary_key(),
            ColumnDef::new("table_name", DataType::Text),
            ColumnDef::new("header", DataType::Blob),
        ];
        let meta_root = self.ensure_system_table(pager, "__sys_paged_vec_meta", meta_cols)?;

        let key = crate::stable_hash::key_for_name(table_name);

        let header = paged_index.export_header();
        let header_bytes = serde_json::to_vec(&header)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize PagedHnswHeader: {e}")))?;
        self.btree.insert(pager, meta_root, key, &header_bytes)?;

        // Store each page in dedicated table __sys_paged_vec_<table_name>
        let vec_table_name = format!("__sys_paged_vec_{}", table_name.to_lowercase());
        let page_cols = vec![
            ColumnDef::new("id", DataType::Integer).primary_key(),
            ColumnDef::new("payload", DataType::Blob),
        ];
        let page_root = self.ensure_system_table(pager, &vec_table_name, page_cols)?;

        for (node_id, page_bytes) in paged_index.store.dump_pages() {
            self.btree.insert(pager, page_root, node_id, &page_bytes)?;
        }

        Ok(())
    }

    /// Load a disk-backed PagedHnswIndex from dedicated B+Tree system tables with LRU cache
    pub fn load_paged_vector_index(
        &mut self,
        pager: &mut Pager,
        table_name: &str,
        cache_capacity: usize,
    ) -> Result<Option<crate::vector::PagedHnswIndex>> {
        let meta_root = match self.catalog.get_table("__sys_paged_vec_meta") {
            Some(t) => t.root_page,
            None => return Ok(None),
        };

        let key = crate::stable_hash::key_for_name(table_name);

        let header_opt = self.btree.search(pager, meta_root, key)?;
        let header_bytes = match header_opt {
            Some(b) => b,
            None => return Ok(None),
        };

        let header = serde_json::from_slice::<crate::vector::PagedHnswHeader>(&header_bytes)
            .map_err(|e| Error::Corrupted(format!("Failed to deserialize PagedHnswHeader: {e}")))?;

        let store = crate::vector::PagedVectorStore::new(cache_capacity);
        let vec_table_name = format!("__sys_paged_vec_{}", table_name.to_lowercase());
        if let Some(t) = self.catalog.get_table(&vec_table_name) {
            self.btree
                .scan_with_ref(pager, t.root_page, |row_id, payload| {
                    store.load_page(row_id, payload.to_vec());
                    Ok(true)
                })?;
        }

        Ok(Some(crate::vector::PagedHnswIndex::import_header(
            header, store,
        )))
    }
}
