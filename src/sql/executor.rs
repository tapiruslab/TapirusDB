//! SQL Statement Executor connecting the AST, Catalog, B+Tree, Vector, and Graph engines.

use crate::btree::{engine, BTreeStorage};
use crate::error::{Error, Result};
use crate::graph::GraphEngine;
use crate::pager::Pager;
use crate::sql::catalog::{Catalog, ColumnDef, DataType, IndexDef, TableDef};
use crate::sql::codec::{decode_row, encode_row};
use crate::sql::parser::{BinaryOp, JoinType, OnConflict, Statement, WhereCondition, WhereExpr};
use crate::traits::{Row, Value, VectorIndexEngine};
use crate::vector::{DistanceMetric, HnswIndex};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Resolved Boolean expression tree with subqueries evaluated
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedWhereExpr {
    /// Basic comparison condition
    Condition(WhereCondition),
    /// Logical AND
    And(Box<ResolvedWhereExpr>, Box<ResolvedWhereExpr>),
    /// Logical OR
    Or(Box<ResolvedWhereExpr>, Box<ResolvedWhereExpr>),
    /// Resolved IN list
    InList {
        /// Target column name
        column: String,
        /// Set of resolved candidate values
        values: Vec<Value>,
        /// Whether the condition is NOT IN
        negated: bool,
    },
}

/// A temporal version record for point-in-time SQL time-travel queries
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalRecord {
    /// Unique temporal version ID
    pub id: u64,
    /// Target table name
    pub table_name: String,
    /// Target row ID
    pub row_id: u64,
    /// Inclusive epoch timestamp from which this version became valid
    pub valid_from: u64,
    /// Exclusive epoch timestamp up to which this version was valid (u64::MAX if currently active)
    pub valid_to: u64,
    /// Encoded row binary payload
    pub payload: Vec<u8>,
}

/// Deterministic 64-bit hash of a SQL Value for B+Tree secondary index key
pub fn value_to_index_key(val: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match val {
        Value::Null => {
            0u8.hash(&mut hasher);
        }
        Value::Integer(i) => {
            1u8.hash(&mut hasher);
            i.hash(&mut hasher);
        }
        Value::Real(f) => {
            2u8.hash(&mut hasher);
            f.to_bits().hash(&mut hasher);
        }
        Value::Text(s) => {
            3u8.hash(&mut hasher);
            s.hash(&mut hasher);
        }
        Value::Blob(b) => {
            4u8.hash(&mut hasher);
            b.hash(&mut hasher);
        }
        Value::Vector(v) => {
            5u8.hash(&mut hasher);
            for x in v {
                x.to_bits().hash(&mut hasher);
            }
        }
    }
    let h = hasher.finish();
    if h == 0 { 1 } else { h }
}

/// A posting entry associating a specific Value with its matching row IDs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexPostingEntry {
    /// Indexed value key
    pub value: Value,
    /// List of row IDs containing this indexed value
    pub row_ids: Vec<u64>,
}

/// An inverted index posting list bucket stored in B+Tree leaf cells
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexBucket {
    /// List of indexed value posting entries
    pub entries: Vec<IndexPostingEntry>,
}

impl IndexBucket {
    /// Create a new empty index posting bucket
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Retrieve the row IDs matching the specified value, if present
    pub fn get_row_ids(&self, val: &Value) -> Option<&[u64]> {
        self.entries
            .iter()
            .find(|e| &e.value == val)
            .map(|e| e.row_ids.as_slice())
    }

    /// Associate a row ID with the specified indexed value
    pub fn add_row_id(&mut self, val: Value, row_id: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.value == val) {
            if !entry.row_ids.contains(&row_id) {
                entry.row_ids.push(row_id);
            }
        } else {
            self.entries.push(IndexPostingEntry {
                value: val,
                row_ids: vec![row_id],
            });
        }
    }

    /// Remove a row ID from an indexed value, returning whether it was found and removed
    pub fn remove_row_id(&mut self, val: &Value, row_id: u64) -> bool {
        let mut changed = false;
        if let Some(entry) = self.entries.iter_mut().find(|e| &e.value == val) {
            let before = entry.row_ids.len();
            entry.row_ids.retain(|&id| id != row_id);
            changed = entry.row_ids.len() != before;
        }
        self.entries.retain(|e| !e.row_ids.is_empty());
        changed
    }

    /// Returns true if this bucket has no indexed entries
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for IndexBucket {
    fn default() -> Self {
        Self::new()
    }
}

/// Persistent disk snapshot record of an HNSW multi-layer graph topology stored in system table
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSnapshotRecord {
    /// Target table name
    pub table_name: String,
    /// Serialized HNSW snapshot graph topology
    pub snapshot: crate::vector::HnswSnapshot,
}

/// The SQL and multi-model execution engine for TapirusDB
#[derive(Debug, Clone)]
pub struct SQLExecutor {
    catalog: Catalog,
    btree: BTreeStorage,
    vector_indexes: HashMap<String, HnswIndex>,
    graph: GraphEngine,
    last_temporal_timestamp: u64,
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
            graph: GraphEngine::new(),
            last_temporal_timestamp: 0,
        })
    }

    /// Return reference to graph engine
    pub fn graph(&self) -> &GraphEngine {
        &self.graph
    }

    /// Return mutable reference to graph engine
    pub fn graph_mut(&mut self) -> &mut GraphEngine {
        &mut self.graph
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
    pub fn persist_graph_node(&mut self, pager: &mut Pager, node: &crate::graph::Node) -> Result<()> {
        let cols = vec![
            ColumnDef::new("id", DataType::Integer).primary_key(),
            ColumnDef::new("payload", DataType::Text),
        ];
        let root_page = self.ensure_system_table(pager, "__sys_graph_nodes", cols)?;
        let payload = serde_json::to_vec(node)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize graph node: {e}")))?;
        self.btree.insert(pager, root_page, node.id, &payload)
    }

    /// Persist a Knowledge Graph Edge to B+Tree disk storage
    pub fn persist_graph_edge(&mut self, pager: &mut Pager, edge: &crate::graph::Edge) -> Result<()> {
        let cols = vec![
            ColumnDef::new("id", DataType::Integer).primary_key(),
            ColumnDef::new("payload", DataType::Text),
        ];
        let root_page = self.ensure_system_table(pager, "__sys_graph_edges", cols)?;
        let payload = serde_json::to_vec(edge)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize graph edge: {e}")))?;
        self.btree.insert(pager, root_page, edge.id, &payload)
    }

    /// Delete a Knowledge Graph Node from B+Tree disk storage
    pub fn delete_graph_node(&mut self, pager: &mut Pager, id: u64) -> Result<()> {
        if let Some(table) = self.catalog.get_table("__sys_graph_nodes") {
            let _ = self.btree.delete(pager, table.root_page, id);
        }
        Ok(())
    }

    /// Delete a Knowledge Graph Edge from B+Tree disk storage
    pub fn delete_graph_edge(&mut self, pager: &mut Pager, id: u64) -> Result<()> {
        if let Some(table) = self.catalog.get_table("__sys_graph_edges") {
            let _ = self.btree.delete(pager, table.root_page, id);
        }
        Ok(())
    }

    /// Load persisted graph nodes and edges from B+Tree disk storage
    pub fn load_graph_from_disk(&mut self, pager: &mut Pager) -> Result<()> {
        if let Some(table) = self.catalog.get_table("__sys_graph_nodes") {
            let root_page = table.root_page;
            let cells = self.btree.scan(pager, root_page)?;
            for cell in cells {
                if let Ok(node) = serde_json::from_slice::<crate::graph::Node>(&cell.payload) {
                    self.graph.restore_node(node);
                }
            }
        }

        if let Some(table) = self.catalog.get_table("__sys_graph_edges") {
            let root_page = table.root_page;
            let cells = self.btree.scan(pager, root_page)?;
            for cell in cells {
                if let Ok(edge) = serde_json::from_slice::<crate::graph::Edge>(&cell.payload) {
                    self.graph.restore_edge(edge);
                }
            }
        }

        Ok(())
    }

    /// Persist an AI Agent MemoryEntry to B+Tree disk storage
    pub fn persist_memory_entry(
        &mut self,
        pager: &mut Pager,
        entry: &crate::memory::MemoryEntry,
    ) -> Result<()> {
        let cols = vec![
            ColumnDef::new("id", DataType::Integer).primary_key(),
            ColumnDef::new("payload", DataType::Text),
        ];
        let root_page = self.ensure_system_table(pager, "__sys_memory", cols)?;
        let payload = serde_json::to_vec(entry)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize memory entry: {e}")))?;
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

    /// Load persisted memory entries from B+Tree disk storage into MemoryEngine
    pub fn load_memory_from_disk(
        &self,
        pager: &mut Pager,
        memory_engine: &mut crate::memory::MemoryEngine,
    ) -> Result<()> {
        if let Some(table) = self.catalog.get_table("__sys_memory") {
            let root_page = table.root_page;
            let cells = self.btree.scan(pager, root_page)?;
            for cell in cells {
                if let Ok(entry) = serde_json::from_slice::<crate::memory::MemoryEntry>(&cell.payload) {
                    memory_engine.restore_entry(entry);
                }
            }
        }
        Ok(())
    }

    /// Return monotonic timestamp in seconds (or sequential increment if within same second)
    pub fn next_temporal_timestamp(&mut self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
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
        let tdef = self.catalog.get_table_mut("__sys_time_travel")
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
        self.btree.insert(pager, root_page, version_id, &rec_payload)?;
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
            let cells = self.btree.scan(pager, root_page)?;
            for cell in cells {
                if let Ok(mut record) = serde_json::from_slice::<TemporalRecord>(&cell.payload) {
                    if record.table_name.eq_ignore_ascii_case(table)
                        && record.row_id == row_id
                        && record.valid_to == u64::MAX
                    {
                        record.valid_to = end_time;
                        let updated_payload = serde_json::to_vec(&record)
                            .map_err(|e| Error::Corrupted(format!("Failed to serialize temporal record: {e}")))?;
                        self.btree.insert(pager, root_page, cell.row_id, &updated_payload)?;
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// Persist all active in-memory HNSW vector index topologies into `__sys_vector_snapshots`
    pub fn persist_vector_indexes_to_disk(&mut self, pager: &mut Pager) -> Result<()> {
        let cols = vec![
            ColumnDef::new("id", DataType::Integer).primary_key(),
            ColumnDef::new("table_name", DataType::Text),
            ColumnDef::new("snapshot", DataType::Blob),
        ];
        let root_page = self.ensure_system_table(pager, "__sys_vector_snapshots", cols)?;
        let _ = pager.set_vector_index_page(root_page as u32);

        for (table_name, index) in &self.vector_indexes {
            let key = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                table_name.to_lowercase().hash(&mut hasher);
                let h = hasher.finish();
                if h == 0 { 1 } else { h }
            };
            let record = VectorSnapshotRecord {
                table_name: table_name.clone(),
                snapshot: index.export_snapshot(),
            };
            let payload = serde_json::to_vec(&record)
                .map_err(|e| Error::Corrupted(format!("Failed to serialize HNSW snapshot: {e}")))?;
            self.btree.insert(pager, root_page, key, &payload)?;
        }
        Ok(())
    }

    /// Automatically warm up and restore in-memory HNSW vector indexes from persisted snapshot or B+Tree tables
    pub fn load_vector_indexes_from_disk(&mut self, pager: &mut Pager) -> Result<()> {
        let mut loaded_tables = HashSet::new();

        // 1. Attempt instantaneous O(N) restoration from persistent topology snapshot or header pointer
        let snap_root = self.catalog.get_table("__sys_vector_snapshots").map(|t| t.root_page)
            .or_else(|| {
                let p = pager.vector_index_page();
                if p != 0 { Some(p as u32) } else { None }
            });
        if let Some(root_page) = snap_root {
            if let Ok(cells) = self.btree.scan(pager, root_page) {
                for cell in cells {
                    if let Ok(rec) = serde_json::from_slice::<VectorSnapshotRecord>(&cell.payload) {
                        let index = HnswIndex::import_snapshot(rec.snapshot);
                        self.vector_indexes.insert(rec.table_name.to_lowercase(), index);
                        loaded_tables.insert(rec.table_name.to_lowercase());
                    }
                }
            }
        }

        // 2. Fallback: for any vector table lacking a persistent snapshot, construct from rows
        for table in self.catalog.tables() {
            let tname = table.name.to_lowercase();
            if !loaded_tables.contains(&tname) {
                if let Some((vec_idx, dims)) = table.vector_column() {
                    let mut index = HnswIndex::new(dims, DistanceMetric::Cosine);
                    let cells = self.btree.scan(pager, table.root_page)?;
                    let all_col_names = table.column_names();
                    for cell in cells {
                        if let Ok(row) = decode_row(&cell.payload, &all_col_names) {
                            if let Some(crate::traits::Value::Vector(vec)) = row.values().get(vec_idx) {
                                let _ = index.insert_vector(cell.row_id, vec);
                            }
                        }
                    }
                    self.vector_indexes.insert(tname, index);
                }
            }
        }
        Ok(())
    }

    /// Recursively resolve a WhereExpr into a ResolvedWhereExpr, executing subqueries
    pub fn resolve_where_expr(
        &mut self,
        pager: &mut Pager,
        expr: &WhereExpr,
    ) -> Result<ResolvedWhereExpr> {
        match expr {
            WhereExpr::Condition(cond) => Ok(ResolvedWhereExpr::Condition(cond.clone())),
            WhereExpr::And(left, right) => {
                let l = self.resolve_where_expr(pager, left)?;
                let r = self.resolve_where_expr(pager, right)?;
                Ok(ResolvedWhereExpr::And(Box::new(l), Box::new(r)))
            }
            WhereExpr::Or(left, right) => {
                let l = self.resolve_where_expr(pager, left)?;
                let r = self.resolve_where_expr(pager, right)?;
                Ok(ResolvedWhereExpr::Or(Box::new(l), Box::new(r)))
            }
            WhereExpr::InList { column, values, negated } => {
                Ok(ResolvedWhereExpr::InList {
                    column: column.clone(),
                    values: values.clone(),
                    negated: *negated,
                })
            }
            WhereExpr::InSubquery { column, subquery, negated } => {
                let sub_rows = self.query(pager, *subquery.clone())?;
                let mut values = Vec::new();
                for r in sub_rows {
                    if let Some(v) = r.values().first() {
                        values.push(v.clone());
                    }
                }
                Ok(ResolvedWhereExpr::InList {
                    column: column.clone(),
                    values,
                    negated: *negated,
                })
            }
        }
    }

    /// Execute a non-query SQL or Graph command (CREATE, INSERT, etc.)
    pub fn execute(&mut self, pager: &mut Pager, stmt: Statement) -> Result<usize> {
        match stmt {
            Statement::CreateTable {
                name,
                if_not_exists,
                columns,
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

                let table_def = TableDef::new(name.clone(), root_page, columns);

                // If table has a vector column, prepare vector index
                if let Some((_, dims)) = table_def.vector_column() {
                    let index = HnswIndex::new(dims, DistanceMetric::Cosine);
                    self.vector_indexes.insert(name.to_lowercase(), index);
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
                let (affected, _) = self.execute_insert(pager, table, columns, values, conflict_action, returning)?;
                Ok(affected)
            }

            Statement::GraphInsertNode { id, label, properties } => {
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
                let edge_id = self.graph.add_edge(from_id, to_id, label, weight, properties)?;
                if let Some(edge) = self.graph.get_edge(edge_id) {
                    let edge_clone = edge.clone();
                    self.persist_graph_edge(pager, &edge_clone)?;
                }
                Ok(1)
            }

            Statement::Update {
                table,
                assignments,
                where_clause,
                returning,
            } => {
                let (affected, _) = self.execute_update(pager, table, assignments, where_clause, returning)?;
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
                let _ = self.persist_vector_indexes_to_disk(pager);
                pager.commit_transaction()?;
                Ok(0)
            }

            Statement::RollbackTransaction => {
                pager.rollback_transaction()?;
                self.catalog = Catalog::load(pager)?;
                self.vector_indexes.clear();
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

                let base_col = if let Some(pos) = column.find('.') {
                    &column[..pos]
                } else {
                    &column
                };
                if table_def.column_index(base_col).is_none() && table_def.column_index(&column).is_none() {
                    return Err(Error::Corrupted(format!(
                        "Column '{column}' not found in table '{table}' for index '{name}'"
                    )));
                }

                let index_root = pager.allocate_page()?;
                let mut index_buf = vec![0u8; pager.page_size()];
                engine::init_leaf_page(&mut index_buf, index_root);
                pager.write_page(index_root, &index_buf)?;

                // Populate inverted index posting list with existing rows from table
                let cells = self.btree.scan(pager, table_def.root_page)?;
                let all_col_names = table_def.column_names();
                let mut buckets: HashMap<u64, IndexBucket> = HashMap::new();

                for cell in cells {
                    let full_row = decode_row(&cell.payload, &all_col_names)?;
                    if let Some(col_val) = full_row.get_field_or_json_path(&column) {
                        if !col_val.is_null() {
                            let key = value_to_index_key(&col_val);
                            buckets
                                .entry(key)
                                .or_insert_with(IndexBucket::new)
                                .add_row_id(col_val, cell.row_id);
                        }
                    }
                }

                for (key, bucket) in buckets {
                    let idx_payload = serde_json::to_vec(&bucket)
                        .map_err(|e| Error::Corrupted(format!("Failed to serialize index bucket: {e}")))?;
                    self.btree.insert(pager, index_root, key, &idx_payload)?;
                }

                let index_def = IndexDef::new(name, table, column, index_root);
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

                    self.catalog.drop_table(pager, &table)?;
                    self.vector_indexes.remove(&name_lower);
                    Ok(1)
                } else if if_exists {
                    Ok(0)
                } else {
                    Err(Error::TableNotFound(table))
                }
            }

            Statement::AlterTableAddColumn { table, column } => {
                self.catalog.add_column(pager, &table, column)?;
                Ok(1)
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
                    return Err(Error::TableExists(format!("Table with name '{name}' already exists")));
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
                        let cells = self.btree.scan(pager, tdef.root_page)?;
                        let total_rows = cells.len();
                        let all_cols = tdef.column_names();
                        let mut distinct_map = HashMap::new();
                        let mut null_map = HashMap::new();
                        let mut total_bytes = 0;

                        for col in &all_cols {
                            distinct_map.insert(col.clone(), std::collections::HashSet::new());
                            null_map.insert(col.clone(), 0usize);
                        }

                        for cell in &cells {
                            total_bytes += cell.payload.len();
                            if let Ok(row) = decode_row(&cell.payload, &all_cols) {
                                for col in &all_cols {
                                    if let Some(v) = row.get_field_or_json_path(col) {
                                        if v.is_null() {
                                            if let Some(n) = null_map.get_mut(col) {
                                                *n += 1;
                                            }
                                        } else {
                                            if let Some(set) = distinct_map.get_mut(col) {
                                                let key = format!("{v:?}");
                                                set.insert(key);
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        let mut column_distinct = HashMap::new();
                        for (k, v) in distinct_map {
                            column_distinct.insert(k, v.len());
                        }

                        let page_count = (total_bytes / pager.page_size()).max(1);
                        let mut stats = crate::sql::planner::TableStats::new(&tname, total_rows, page_count);
                        stats.column_distinct = column_distinct;
                        stats.column_nulls = null_map;
                        self.catalog.set_table_stats(&tname, stats);
                        analyzed_count += 1;
                    }
                }
                Ok(analyzed_count)
            }

            Statement::Select { .. }
            | Statement::VectorSearch { .. }
            | Statement::Explain { .. }
            | Statement::GraphTraverse { .. }
            | Statement::GraphShortestPath { .. }
            | Statement::GraphMatch { .. }
            | Statement::GraphAlgorithm { .. }
            | Statement::WithCte { .. } => {
                Err(Error::SqlSyntax(
                    "Query statement passed to execute(); use query() instead".into(),
                ))
            }
        }
    }

    /// Execute a query returning rows (SELECT, VECTOR NEAR, EXPLAIN, RETURNING, etc.)
    pub fn query(&mut self, pager: &mut Pager, stmt: Statement) -> Result<Vec<Row>> {
        match stmt {
            Statement::Insert {
                table,
                columns,
                values,
                conflict_action,
                returning,
            } => {
                let (_, rows) = self.execute_insert(pager, table, columns, values, conflict_action, returning)?;
                Ok(rows)
            }
            Statement::Update {
                table,
                assignments,
                where_clause,
                returning,
            } => {
                let (_, rows) = self.execute_update(pager, table, assignments, where_clause, returning)?;
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
            Statement::Explain { statement, query_plan } => {
                self.explain_query_plan(pager, *statement, query_plan)
            }

            Statement::Select {
                distinct,
                columns,
                table,
                join,
                where_clause,
                group_by,
                having,
                order_by,
                limit,
                offset,
                as_of_timestamp,
            } => {
                // If target table is a persistent View, evaluate view query and apply outer filters/order/limit
                if let Some(view_def) = self.catalog.get_view(&table).cloned() {
                    let mut view_stmt = crate::sql::parse_sql(&view_def.query_sql)?;
                    if let Statement::Select { as_of_timestamp: ref mut view_ts, .. } = view_stmt {
                        *view_ts = as_of_timestamp;
                    }
                    let mut rows = self.query(pager, view_stmt)?;

                    // Check outer WHERE filter
                    if let Some(ref expr) = where_clause {
                        let resolved_where = self.resolve_where_expr(pager, expr)?;
                        rows.retain(|r| row_matches_resolved(r, &resolved_where));
                    }

                    // Apply outer ORDER BY
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

                    // Apply outer column projection and LIMIT
                    let output_col_names = if columns.is_empty() {
                        if let Some(first) = rows.first() {
                            first.columns().to_vec()
                        } else {
                            Vec::new()
                        }
                    } else {
                        columns.clone()
                    };

                    let mut result_rows = Vec::new();
                    let skip_count = offset.unwrap_or(0);
                    let mut skipped = 0;
                    for row in rows {
                        let projected = project_row(&row, &output_col_names)?;
                        if distinct && result_rows.contains(&projected) {
                            continue;
                        }
                        if skipped < skip_count {
                            skipped += 1;
                            continue;
                        }
                        result_rows.push(projected);
                        if let Some(lim) = limit {
                            if result_rows.len() >= lim {
                                break;
                            }
                        }
                    }

                    return Ok(result_rows);
                }

                let table_def = self
                    .catalog
                    .get_table(&table)
                    .cloned()
                    .ok_or_else(|| Error::TableNotFound(table.clone()))?;

                let root_page = table_def.root_page;
                let all_col_names = table_def.column_names();

                let resolved_where = if let Some(ref expr) = where_clause {
                    Some(self.resolve_where_expr(pager, expr)?)
                } else {
                    None
                };

                let resolved_having = if let Some(ref h_expr) = having {
                    Some(self.resolve_where_expr(pager, h_expr)?)
                } else {
                    None
                };

                // Fast Point Lookup Optimization if WHERE matches single Primary Key column equals and no JOIN, ORDER BY, GROUP BY
                let pk_idx = table_def.primary_key_index();
                if as_of_timestamp.is_none() && join.is_none() && order_by.is_none() && group_by.is_none() {
                    if let (Some(pk_col_idx), Some(WhereExpr::Condition(cond))) = (pk_idx, &where_clause) {
                        if cond.op == BinaryOp::Equals {
                            let pk_name = &table_def.columns[pk_col_idx].name;
                            if pk_name.eq_ignore_ascii_case(&cond.column) {
                                if let Value::Integer(target_key) = cond.value {
                                    if offset.unwrap_or(0) > 0 || limit == Some(0) {
                                        return Ok(Vec::new());
                                    }
                                    if let Some(payload) = self.btree.search(pager, root_page, target_key as u64)? {
                                        let full_row = decode_row(&payload, &all_col_names)?;
                                        let output_col_names = if columns.is_empty() {
                                            all_col_names.clone()
                                        } else {
                                            columns.clone()
                                        };
                                        let projected = project_row(&full_row, &output_col_names)?;
                                        return Ok(vec![projected]);
                                    } else {
                                        return Ok(Vec::new());
                                    }
                                }
                            }
                        }
                    }
                }

                // Fast Secondary Index Direct B+Tree Search (Zero Scan Inverted Posting List Optimization)
                if as_of_timestamp.is_none() && join.is_none() && order_by.is_none() && group_by.is_none() {
                    if let Some(WhereExpr::Condition(cond)) = &where_clause {
                        if cond.op == BinaryOp::Equals {
                            if let Some(index_def) = self.catalog.find_index_for_column(&table, &cond.column).cloned() {
                                let key = value_to_index_key(&cond.value);
                                let mut matched_row_ids = Vec::new();
                                if let Some(payload) = self.btree.search(pager, index_def.root_page, key)? {
                                    if let Ok(bucket) = serde_json::from_slice::<IndexBucket>(&payload) {
                                        if let Some(rids) = bucket.get_row_ids(&cond.value) {
                                            matched_row_ids.extend_from_slice(rids);
                                        }
                                    }
                                }
                                let output_col_names = if columns.is_empty() {
                                    all_col_names.clone()
                                } else {
                                    columns.clone()
                                };
                                let mut result_rows = Vec::new();
                                let skip_count = offset.unwrap_or(0);
                                let mut skipped = 0;
                                for rid in matched_row_ids {
                                    if let Some(payload) = self.btree.search(pager, root_page, rid)? {
                                        let full_row = decode_row(&payload, &all_col_names)?;
                                        let projected = project_row(&full_row, &output_col_names)?;
                                        if distinct && result_rows.contains(&projected) {
                                            continue;
                                        }
                                        if skipped < skip_count {
                                            skipped += 1;
                                            continue;
                                        }
                                        result_rows.push(projected);
                                        if let Some(lim) = limit {
                                            if result_rows.len() >= lim {
                                                break;
                                            }
                                        }
                                    }
                                }
                                return Ok(result_rows);
                            }
                        }
                    }
                }

                // Table scan left table
                let mut left_rows = Vec::new();
                if let Some(ts) = as_of_timestamp {
                    if let Some(sys_table) = self.catalog.get_table("__sys_time_travel") {
                        let cells = self.btree.scan(pager, sys_table.root_page)?;
                        for cell in cells {
                            if let Ok(rec) = serde_json::from_slice::<TemporalRecord>(&cell.payload) {
                                if rec.table_name.eq_ignore_ascii_case(&table)
                                    && rec.valid_from <= ts
                                    && rec.valid_to > ts
                                {
                                    let full_row = decode_row(&rec.payload, &all_col_names)?;
                                    if let Some(ref r_expr) = resolved_where {
                                        if !row_matches_resolved(&full_row, r_expr) {
                                            continue;
                                        }
                                    }
                                    left_rows.push(full_row);
                                }
                            }
                        }
                    }
                } else {
                    let cells = self.btree.scan(pager, root_page)?;
                    let can_early_terminate = join.is_none() && order_by.is_none() && group_by.is_none() && !distinct && !is_aggregate_query(&columns);
                    let needed_rows = if can_early_terminate {
                        limit.map(|lim| offset.unwrap_or(0).saturating_add(lim))
                    } else {
                        None
                    };

                    for cell in cells {
                        let full_row = decode_row(&cell.payload, &all_col_names)?;

                        // Check WHERE filter on left table
                        if let Some(ref r_expr) = resolved_where {
                            if !row_matches_resolved(&full_row, r_expr) {
                                continue;
                            }
                        }

                        left_rows.push(full_row);
                        if let Some(needed) = needed_rows {
                            if left_rows.len() >= needed {
                                break;
                            }
                        }
                    }
                }

                // Execute JOIN if present
                if let Some(join_clause) = join {
                    let right_table_def = self
                        .catalog
                        .get_table(&join_clause.table)
                        .ok_or_else(|| Error::TableNotFound(join_clause.table.clone()))?;
                    let right_col_names = right_table_def.column_names();

                    let mut right_rows = Vec::new();
                    if let Some(ts) = as_of_timestamp {
                        if let Some(sys_table) = self.catalog.get_table("__sys_time_travel") {
                            let cells = self.btree.scan(pager, sys_table.root_page)?;
                            for cell in cells {
                                if let Ok(rec) = serde_json::from_slice::<TemporalRecord>(&cell.payload) {
                                    if rec.table_name.eq_ignore_ascii_case(&join_clause.table)
                                        && rec.valid_from <= ts
                                        && rec.valid_to > ts
                                    {
                                        let r_row = decode_row(&rec.payload, &right_col_names)?;
                                        right_rows.push(r_row);
                                    }
                                }
                            }
                        }
                    } else {
                        let right_cells = self.btree.scan(pager, right_table_def.root_page)?;
                        for r_cell in right_cells {
                            let r_row = decode_row(&r_cell.payload, &right_col_names)?;
                            right_rows.push(r_row);
                        }
                    }

                    let mut joined_rows = Vec::new();
                    let mut matched_right_indices = HashSet::new();

                    for l_row in &left_rows {
                        let l_val = l_row.get_value(&join_clause.left_col);
                        let mut matched = false;

                        for (r_idx, r_row) in right_rows.iter().enumerate() {
                            let r_val = r_row.get_value(&join_clause.right_col);
                            if let (Some(lv), Some(rv)) = (l_val, r_val) {
                                if lv == rv {
                                    matched = true;
                                    matched_right_indices.insert(r_idx);
                                    let mut merged_cols = Vec::new();
                                    let mut merged_vals = Vec::new();

                                    for (c, v) in l_row.columns().iter().zip(l_row.values().iter()) {
                                        merged_cols.push(format!("{}.{}", table, c));
                                        merged_vals.push(v.clone());
                                    }
                                    for (c, v) in r_row.columns().iter().zip(r_row.values().iter()) {
                                        merged_cols.push(format!("{}.{}", join_clause.table, c));
                                        merged_vals.push(v.clone());
                                    }

                                    joined_rows.push(Row::new(merged_cols, merged_vals));
                                }
                            }
                        }

                        if !matched && (join_clause.join_type == JoinType::Left || join_clause.join_type == JoinType::Full) {
                            let mut merged_cols = Vec::new();
                            let mut merged_vals = Vec::new();

                            for (c, v) in l_row.columns().iter().zip(l_row.values().iter()) {
                                merged_cols.push(format!("{}.{}", table, c));
                                merged_vals.push(v.clone());
                            }
                            for c in &right_col_names {
                                merged_cols.push(format!("{}.{}", join_clause.table, c));
                                merged_vals.push(Value::Null);
                            }

                            joined_rows.push(Row::new(merged_cols, merged_vals));
                        }
                    }

                    if join_clause.join_type == JoinType::Right || join_clause.join_type == JoinType::Full {
                        for (r_idx, r_row) in right_rows.iter().enumerate() {
                            if !matched_right_indices.contains(&r_idx) {
                                let mut merged_cols = Vec::new();
                                let mut merged_vals = Vec::new();

                                for c in &all_col_names {
                                    merged_cols.push(format!("{}.{}", table, c));
                                    merged_vals.push(Value::Null);
                                }
                                for (c, v) in r_row.columns().iter().zip(r_row.values().iter()) {
                                    merged_cols.push(format!("{}.{}", join_clause.table, c));
                                    merged_vals.push(v.clone());
                                }

                                joined_rows.push(Row::new(merged_cols, merged_vals));
                            }
                        }
                    }

                    left_rows = joined_rows;
                }

                // Check for GROUP BY
                if let Some(ref gb_cols) = group_by {
                    let mut groups: Vec<(Vec<Value>, Vec<Row>)> = Vec::new();
                    for row in left_rows {
                        let key: Vec<Value> = gb_cols
                            .iter()
                            .map(|c| row.get_value(c).cloned().unwrap_or(Value::Null))
                            .collect();
                        if let Some(pos) = groups.iter().position(|(k, _)| k == &key) {
                            groups[pos].1.push(row);
                        } else {
                            groups.push((key, vec![row]));
                        }
                    }

                    let mut grouped_rows = Vec::new();
                    let output_cols = if columns.is_empty() {
                        gb_cols.clone()
                    } else {
                        columns.clone()
                    };

                    for (key, g_rows) in groups {
                        let mut g_cols = Vec::new();
                        let mut g_vals = Vec::new();

                        for col in &output_cols {
                            let upper = col.to_uppercase();
                            if let Some(open_p) = upper.find('(') {
                                if let Some(close_p) = upper.rfind(')') {
                                    let func = &upper[..open_p];
                                    let arg = col[open_p + 1..close_p].trim();
                                    let computed = compute_aggregate(func, arg, &g_rows);
                                    g_cols.push(col.clone());
                                    g_vals.push(computed);
                                    continue;
                                }
                            }
                            if let Some(pos) = gb_cols.iter().position(|c| c == col) {
                                g_cols.push(col.clone());
                                g_vals.push(key[pos].clone());
                            } else {
                                let val = g_rows.first().and_then(|r| r.get_value(col)).cloned().unwrap_or(Value::Null);
                                g_cols.push(col.clone());
                                g_vals.push(val);
                            }
                        }

                        let g_row = Row::new(g_cols, g_vals);
                        if let Some(ref h_expr) = resolved_having {
                            if !row_matches_resolved(&g_row, h_expr) {
                                continue;
                            }
                        }
                        grouped_rows.push(g_row);
                    }

                    left_rows = grouped_rows;
                } else if is_aggregate_query(&columns) {
                    let mut agg_cols = Vec::new();
                    let mut agg_vals = Vec::new();

                    for col in &columns {
                        let upper = col.to_uppercase();
                        if let Some(open_p) = upper.find('(') {
                            if let Some(close_p) = upper.rfind(')') {
                                let func = &upper[..open_p];
                                let arg = col[open_p + 1..close_p].trim();
                                let computed = compute_aggregate(func, arg, &left_rows);
                                agg_cols.push(col.clone());
                                agg_vals.push(computed);
                                continue;
                            }
                        }
                        agg_cols.push(col.clone());
                        agg_vals.push(Value::Null);
                    }

                    return Ok(vec![Row::new(agg_cols, agg_vals)]);
                }

                // Evaluate window functions if present in columns
                if columns.iter().any(|c| has_window_function(c)) {
                    evaluate_window_functions(&mut left_rows, &columns)?;
                }

                // Apply ORDER BY sorting if requested
                if let Some(order) = order_by {
                    left_rows.sort_by(|a, b| {
                        let val_a = a.get_value(&order.column).unwrap_or(&Value::Null);
                        let val_b = b.get_value(&order.column).unwrap_or(&Value::Null);
                        if order.is_ascending {
                            val_a.compare(val_b)
                        } else {
                            val_b.compare(val_a)
                        }
                    });
                }

                // Project columns and apply LIMIT
                let output_col_names = if columns.is_empty() {
                    if let Some(first_row) = left_rows.first() {
                        first_row.columns().to_vec()
                    } else {
                        all_col_names.clone()
                    }
                } else {
                    columns.clone()
                };

                let mut rows = Vec::new();
                let skip_count = offset.unwrap_or(0);
                let mut skipped = 0;
                for row in left_rows {
                    let projected = project_row(&row, &output_col_names)?;
                    if distinct && rows.contains(&projected) {
                        continue;
                    }
                    if skipped < skip_count {
                        skipped += 1;
                        continue;
                    }
                    rows.push(projected);

                    if let Some(lim) = limit {
                        if rows.len() >= lim {
                            break;
                        }
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
                let table_def = self
                    .catalog
                    .get_table(&table)
                    .cloned()
                    .ok_or_else(|| Error::TableNotFound(table.clone()))?;

                let root_page = table_def.root_page;
                let all_col_names = table_def.column_names();
                let output_col_names = if columns.is_empty() {
                    all_col_names.clone()
                } else {
                    columns.clone()
                };

                // Compute candidate row IDs if where_clause is present (Pre-Filtering)
                let candidate_ids = if let Some(ref expr) = where_clause {
                    let resolved = self.resolve_where_expr(pager, expr)?;
                    let mut matched = std::collections::HashSet::new();

                    let mut used_index = false;
                    if let ResolvedWhereExpr::Condition(cond) = &resolved {
                        if cond.op == BinaryOp::Equals {
                            if let Some(index_def) = self.catalog.find_index_for_column(&table, &cond.column).cloned() {
                                let key = value_to_index_key(&cond.value);
                                if let Some(payload) = self.btree.search(pager, index_def.root_page, key)? {
                                    if let Ok(bucket) = serde_json::from_slice::<IndexBucket>(&payload) {
                                        if let Some(rids) = bucket.get_row_ids(&cond.value) {
                                            for &rid in rids {
                                                matched.insert(rid);
                                            }
                                            used_index = true;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if !used_index {
                        let cells = self.btree.scan(pager, root_page)?;
                        for cell in cells {
                            if let Ok(full_row) = decode_row(&cell.payload, &all_col_names) {
                                if row_matches_resolved(&full_row, &resolved) {
                                    matched.insert(cell.row_id);
                                }
                            }
                        }
                    }
                    Some(matched)
                } else {
                    None
                };

                let mut rows = Vec::new();
                if let Some(index) = self.vector_indexes.get(&table.to_lowercase()) {
                    let neighbors = index.search_knn_filtered(&query_vector, top_k, DistanceMetric::Cosine, candidate_ids.as_ref())?;
                    for (row_id, _) in neighbors {
                        if let Some(payload) = self.btree.search(pager, root_page, row_id)? {
                            let full_row = decode_row(&payload, &all_col_names)?;
                            let projected = project_row(&full_row, &output_col_names)?;
                            rows.push(projected);
                        }
                    }
                } else {
                    // Exact Flat KNN Scan: Calculate true mathematical cosine distances across filtered candidate records
                    let cells = self.btree.scan(pager, root_page)?;
                    if let Some((vec_idx, _)) = table_def.vector_column() {
                        let mut scored_rows: Vec<(f32, Row)> = Vec::new();
                        for cell in cells {
                            if let Some(ref allowed) = candidate_ids {
                                if !allowed.contains(&cell.row_id) {
                                    continue;
                                }
                            }
                            if let Ok(full_row) = decode_row(&cell.payload, &all_col_names) {
                                if let Some(crate::traits::Value::Vector(v)) = full_row.values().get(vec_idx) {
                                    let dist = crate::vector::cosine_distance(&query_vector, v);
                                    let projected = project_row(&full_row, &output_col_names)?;
                                    scored_rows.push((dist, projected));
                                }
                            }
                        }
                        scored_rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                        for (_, row) in scored_rows.into_iter().take(top_k) {
                            rows.push(row);
                        }
                    } else {
                        for cell in cells.into_iter() {
                            if let Some(ref allowed) = candidate_ids {
                                if !allowed.contains(&cell.row_id) {
                                    continue;
                                }
                            }
                            let full_row = decode_row(&cell.payload, &all_col_names)?;
                            let projected = project_row(&full_row, &output_col_names)?;
                            rows.push(projected);
                            if rows.len() >= top_k {
                                break;
                            }
                        }
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
                    rows.push(Row::new(cols.clone(), vec![
                        Value::Integer(start_node.id as i64),
                        Value::Text(start_node.label.clone()),
                        Value::Text(start_node.properties.clone()),
                        Value::Integer(0),
                    ]));
                    queue.push_back((start_id, 0usize));
                }

                while let Some((curr_id, depth)) = queue.pop_front() {
                    if depth >= max_depth {
                        continue;
                    }
                    let neighbors = self.graph.neighbor_refs(curr_id, direction, label.as_deref());
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
                            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&neighbor_node.properties) {
                                if let serde_json::Value::Object(map) = json_val {
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
                                        test_cols.push(k.clone());
                                        test_cols.push(format!("target.{k}"));
                                        test_vals.push(db_val.clone());
                                        test_vals.push(db_val);
                                    }
                                }
                            }

                            let test_row = Row::new(test_cols, test_vals);
                            if !row_matches_resolved(&test_row, rw) {
                                continue;
                            }
                        }

                        if visited.insert(neighbor_node.id) {
                            rows.push(Row::new(cols.clone(), vec![
                                Value::Integer(neighbor_node.id as i64),
                                Value::Text(neighbor_node.label.clone()),
                                Value::Text(neighbor_node.properties.clone()),
                                Value::Integer((depth + 1) as i64),
                            ]));
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
                if let Some((path, total_cost)) = self.graph.dijkstra_shortest_path(start_id, end_id) {
                    for (step_idx, edge) in path.into_iter().enumerate() {
                        rows.push(Row::new(cols.clone(), vec![
                            Value::Integer((step_idx + 1) as i64),
                            Value::Integer(edge.from_id as i64),
                            Value::Integer(edge.to_id as i64),
                            Value::Text(edge.label),
                            Value::Real(edge.weight as f64),
                            Value::Real(total_cost as f64),
                        ]));
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
                    if let Some(ref rl) = rel_label {
                        if !edge.label.eq_ignore_ascii_case(rl) {
                            continue;
                        }
                    }

                    // Source node
                    let source_node = match self.graph.get_node(edge.from_id) {
                        Some(n) => n,
                        None => continue,
                    };
                    if let Some(ref sl) = source_label {
                        if !source_node.label.eq_ignore_ascii_case(sl) {
                            continue;
                        }
                    }

                    // Target node
                    let target_node = match self.graph.get_node(edge.to_id) {
                        Some(n) => n,
                        None => continue,
                    };
                    if let Some(ref tl) = target_label {
                        if !target_node.label.eq_ignore_ascii_case(tl) {
                            continue;
                        }
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
                    if let Ok(json_src) = serde_json::from_str::<serde_json::Value>(&source_node.properties) {
                        if let serde_json::Value::Object(map) = json_src {
                            for (k, v) in map {
                                let db_val = match v {
                                    serde_json::Value::Number(n) => {
                                        if let Some(i) = n.as_i64() { Value::Integer(i) }
                                        else if let Some(f) = n.as_f64() { Value::Real(f) }
                                        else { Value::Null }
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
                    }

                    // Parse JSON properties for target node
                    if let Ok(json_tgt) = serde_json::from_str::<serde_json::Value>(&target_node.properties) {
                        if let serde_json::Value::Object(map) = json_tgt {
                            for (k, v) in map {
                                let db_val = match v {
                                    serde_json::Value::Number(n) => {
                                        if let Some(i) = n.as_i64() { Value::Integer(i) }
                                        else if let Some(f) = n.as_f64() { Value::Real(f) }
                                        else { Value::Null }
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
                    }

                    let test_row = Row::new(test_cols, test_vals);

                    if let Some(ref rw) = resolved_where {
                        if !row_matches_resolved(&test_row, rw) {
                            continue;
                        }
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
                            row_vals.push(Value::Text(format!("(:{} {{id: {}, properties: {}}})", source_node.label, source_node.id, source_node.properties)));
                        } else if ret_col == &target_var {
                            row_vals.push(Value::Text(format!("(:{} {{id: {}, properties: {}}})", target_node.label, target_node.id, target_node.properties)));
                        } else if rel_var.as_ref() == Some(ret_col) {
                            row_vals.push(Value::Text(format!("[:{} {{id: {}, weight: {}}}]", edge.label, edge.id, edge.weight)));
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

            Statement::GraphAlgorithm { algorithm, options } => {
                match algorithm.as_str() {
                    "PAGERANK" => {
                        let damping: f32 = options.get("damping").and_then(|v| v.parse().ok()).unwrap_or(0.85);
                        let iterations: usize = options.get("iterations").and_then(|v| v.parse().ok()).unwrap_or(20);
                        let tol: f32 = options.get("tolerance").and_then(|v| v.parse().ok()).unwrap_or(1e-4);
                        let ranks = self.graph.pagerank(damping, iterations, tol);

                        let cols = vec!["node_id".to_string(), "label".to_string(), "pagerank".to_string()];
                        let mut rows = Vec::new();
                        let mut sorted: Vec<(u64, f32)> = ranks.into_iter().collect();
                        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

                        for (id, rank) in sorted {
                            let label = self.graph.get_node(id).map(|n| n.label.clone()).unwrap_or_default();
                            rows.push(Row::new(cols.clone(), vec![
                                Value::Integer(id as i64),
                                Value::Text(label),
                                Value::Real(rank as f64),
                            ]));
                        }
                        Ok(rows)
                    }
                    "CONNECTED_COMPONENTS" | "COMPONENTS" | "WCC" => {
                        let components = self.graph.connected_components();
                        let cols = vec!["node_id".to_string(), "label".to_string(), "component_id".to_string()];
                        let mut rows = Vec::new();
                        let mut sorted: Vec<(u64, usize)> = components.into_iter().collect();
                        sorted.sort_by_key(|&(id, comp)| (comp, id));

                        for (id, comp) in sorted {
                            let label = self.graph.get_node(id).map(|n| n.label.clone()).unwrap_or_default();
                            rows.push(Row::new(cols.clone(), vec![
                                Value::Integer(id as i64),
                                Value::Text(label),
                                Value::Integer(comp as i64),
                            ]));
                        }
                        Ok(rows)
                    }
                    "BETWEENNESS" | "BETWEENNESS_CENTRALITY" => {
                        let normalized = options.get("normalized").map(|v| v != "false" && v != "0").unwrap_or(true);
                        let scores = self.graph.betweenness_centrality(normalized);
                        let cols = vec!["node_id".to_string(), "label".to_string(), "betweenness".to_string()];
                        let mut rows = Vec::new();
                        let mut sorted: Vec<(u64, f32)> = scores.into_iter().collect();
                        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

                        for (id, score) in sorted {
                            let label = self.graph.get_node(id).map(|n| n.label.clone()).unwrap_or_default();
                            rows.push(Row::new(cols.clone(), vec![
                                Value::Integer(id as i64),
                                Value::Text(label),
                                Value::Real(score as f64),
                            ]));
                        }
                        Ok(rows)
                    }
                    "LOUVAIN" | "COMMUNITIES" => {
                        let communities = self.graph.louvain_communities();
                        let cols = vec!["node_id".to_string(), "label".to_string(), "community_id".to_string()];
                        let mut rows = Vec::new();
                        let mut sorted: Vec<(u64, usize)> = communities.into_iter().collect();
                        sorted.sort_by_key(|&(id, comm)| (comm, id));

                        for (id, comm) in sorted {
                            let label = self.graph.get_node(id).map(|n| n.label.clone()).unwrap_or_default();
                            rows.push(Row::new(cols.clone(), vec![
                                Value::Integer(id as i64),
                                Value::Text(label),
                                Value::Integer(comm as i64),
                            ]));
                        }
                        Ok(rows)
                    }
                    unknown => Err(Error::SqlSyntax(format!(
                        "Unknown graph algorithm '{unknown}'. Supported: PAGERANK, CONNECTED_COMPONENTS, BETWEENNESS, LOUVAIN"
                    ))),
                }
            }

            other => Err(Error::SqlSyntax(format!(
                "Non-query statement {other:?} passed to query()"
            ))),
        }
    }

    /// Generate an EXPLAIN / EXPLAIN QUERY PLAN representation
    fn explain_query_plan(
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
                where_clause,
                order_by,
                limit,
                ..
            } => {
                let table_def = self.catalog.get_table(&table);
                let pk_idx = table_def.and_then(|t| t.primary_key_index());
                let mut id = 0;

                let optimizer = crate::sql::planner::CostOptimizer::new();
                let default_stats = crate::sql::planner::TableStats::new(&table, 100, 10);
                let stats = self.catalog.get_table_stats(&table).unwrap_or(&default_stats);

                // Check if fast point lookup by PK is used
                let mut is_pk_search = false;
                if join.is_none() && order_by.is_none() {
                    if let (Some(pk_col_idx), Some(WhereExpr::Condition(cond))) = (pk_idx, &where_clause) {
                        if cond.op == BinaryOp::Equals {
                            if let Some(tdef) = table_def {
                                if tdef.columns[pk_col_idx].name.eq_ignore_ascii_case(&cond.column) {
                                    is_pk_search = true;
                                    let cost = optimizer.estimate_pk_lookup(stats);
                                    rows.push(Row::new(
                                        cols.clone(),
                                        vec![
                                            Value::Integer(id),
                                            Value::Integer(0),
                                            Value::Text(format!(
                                                "SEARCH TABLE {} USING PRIMARY KEY ({} = {:?}) {}",
                                                table, cond.column, cond.value, cost
                                            )),
                                        ],
                                    ));
                                    id += 1;
                                }
                            }
                        }
                    }
                }

                let mut is_index_search = false;
                if !is_pk_search && join.is_none() && order_by.is_none() {
                    if let Some(WhereExpr::Condition(cond)) = &where_clause {
                        if cond.op == BinaryOp::Equals {
                            if let Some(idx_def) = self.catalog.find_index_for_column(&table, &cond.column) {
                                is_index_search = true;
                                let sel = stats.estimate_selectivity(&cond.column, true);
                                let cost = optimizer.estimate_index_scan(stats, &idx_def.name, &cond.column, sel);
                                rows.push(Row::new(
                                    cols.clone(),
                                    vec![
                                        Value::Integer(id),
                                        Value::Integer(0),
                                        Value::Text(format!(
                                            "SEARCH TABLE {} USING INDEX {} ({} = {:?}) {}",
                                            table, idx_def.name, cond.column, cond.value, cost
                                        )),
                                    ],
                                ));
                                id += 1;
                            }
                        }
                    }
                }

                if !is_pk_search && !is_index_search {
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
                    id += 1;
                }

                if let Some(j) = join {
                    rows.push(Row::new(
                        cols.clone(),
                        vec![
                            Value::Integer(id),
                            Value::Integer(0),
                            Value::Text(format!(
                                "SEARCH SUBQUERY / JOIN TABLE {} ON {}.{} = {}.{}",
                                j.table, table, j.left_col, j.table, j.right_col
                            )),
                        ],
                    ));
                    id += 1;
                }

                if let Some(ord) = order_by {
                    let dir = if ord.is_ascending { "ASC" } else { "DESC" };
                    rows.push(Row::new(
                        cols.clone(),
                        vec![
                            Value::Integer(id),
                            Value::Integer(0),
                            Value::Text(format!("USE TEMP B-TREE FOR ORDER BY {} {}", ord.column, dir)),
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
            Statement::VectorSearch { table, vector_col, top_k, query_vector, .. } => {
                let optimizer = crate::sql::planner::CostOptimizer::new();
                let default_stats = crate::sql::planner::TableStats::new(&table, 100, 10);
                let stats = self.catalog.get_table_stats(&table).unwrap_or(&default_stats);
                let has_hnsw = self.vector_indexes.contains_key(&table.to_lowercase());
                let cost = optimizer.estimate_vector_search(stats, top_k, query_vector.len(), has_hnsw);
                rows.push(Row::new(
                    cols.clone(),
                    vec![
                        Value::Integer(0),
                        Value::Integer(0),
                        Value::Text(format!(
                            "SEARCH TABLE {} USING {} ON {} (TOP {}) {}",
                            table,
                            if has_hnsw { "HNSW VECTOR INDEX" } else { "BRUTE-FORCE SCAN" },
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

    /// Execute a query with Common Table Expressions (WITH cte AS (...) SELECT ...)
    fn query_with_cte(
        &mut self,
        pager: &mut Pager,
        ctes: Vec<crate::sql::parser::CteClause>,
        main_query: Statement,
    ) -> Result<Vec<Row>> {
        let mut cte_store: HashMap<String, Vec<Row>> = HashMap::new();

        for cte in ctes {
            // Check if cte.query is a Select referencing previous CTEs
            let rows = match *cte.query {
                Statement::Select { ref table, .. } if cte_store.contains_key(table) => {
                    self.query_on_ephemeral_rows(pager, *cte.query, &cte_store)?
                }
                stmt => self.query(pager, stmt)?,
            };

            let mapped_rows = if let Some(ref explicit_cols) = cte.columns {
                rows.into_iter()
                    .map(|r| {
                        let mut vals = Vec::new();
                        for col in explicit_cols {
                            vals.push(r.get_value(col).cloned().unwrap_or(Value::Null));
                        }
                        Row::new(explicit_cols.clone(), vals)
                    })
                    .collect()
            } else {
                rows
            };

            cte_store.insert(cte.name, mapped_rows);
        }

        // Execute main query against ephemeral CTE store
        match main_query {
            Statement::Select { ref table, .. } if cte_store.contains_key(table) => {
                self.query_on_ephemeral_rows(pager, main_query, &cte_store)
            }
            stmt => self.query(pager, stmt),
        }
    }

    /// Evaluate a SELECT statement against in-memory ephemeral CTE tables
    fn query_on_ephemeral_rows(
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
            where_clause,
            having: _,
            order_by,
            limit,
            offset,
            ..
        } = stmt
        {
            let base_rows = cte_store
                .get(&table)
                .cloned()
                .ok_or_else(|| Error::TableNotFound(table.clone()))?;

            let mut rows = base_rows;

            // Handle optional JOIN
            if let Some(ref j) = join {
                let right_rows = if let Some(r_cte) = cte_store.get(&j.table) {
                    r_cte.clone()
                } else if self.catalog.table_exists(&j.table) {
                    self.query(
                        pager,
                        Statement::Select {
                            distinct: false,
                            columns: vec![],
                            table: j.table.clone(),
                            join: None,
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
                        let left_v = left_r.get_value(&j.left_col);
                        let right_v = right_r.get_value(&j.right_col);
                        if let (Some(lv), Some(rv)) = (left_v, right_v) {
                            if lv == rv && !lv.is_null() {
                                matched = true;
                                let mut j_cols = left_r.columns().to_vec();
                                let mut j_vals = left_r.values().to_vec();
                                for (rc, rv) in
                                    right_r.columns().iter().zip(right_r.values().iter())
                                {
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

            // Apply WHERE clause
            if let Some(ref expr) = where_clause {
                let resolved = self.resolve_where_expr(pager, expr)?;
                rows.retain(|r| row_matches_resolved(r, &resolved));
            }

            // Apply ORDER BY
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

            // Apply Aggregates / Projection
            if is_aggregate_query(&columns) {
                let mut agg_cols = Vec::new();
                let mut agg_vals = Vec::new();

                for col in &columns {
                    let upper = col.to_uppercase();
                    if let Some(open_p) = upper.find('(') {
                        if let Some(close_p) = upper.rfind(')') {
                            let func = &upper[..open_p];
                            let arg = col[open_p + 1..close_p].trim();
                            let computed = compute_aggregate(func, arg, &rows);
                            agg_cols.push(col.clone());
                            agg_vals.push(computed);
                            continue;
                        }
                    }
                    agg_cols.push(col.clone());
                    agg_vals.push(Value::Null);
                }

                return Ok(vec![Row::new(agg_cols, agg_vals)]);
            } else if !columns.is_empty() && columns[0] != "*" {
                // Projection
                rows = rows
                    .into_iter()
                    .map(|r| {
                        let mut vals = Vec::new();
                        for c in &columns {
                            vals.push(r.get_value(c).cloned().unwrap_or(Value::Null));
                        }
                        Row::new(columns.clone(), vals)
                    })
                    .collect();
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

    fn execute_insert(
        &mut self,
        pager: &mut Pager,
        table: String,
        columns: Option<Vec<String>>,
        values: Vec<Value>,
        conflict_action: OnConflict,
        returning: Option<Vec<String>>,
    ) -> Result<(usize, Vec<Row>)> {
        let table_cols;
        let root_page;
        let row_id;
        let mut aligned_values;
        let vector_col;

        {
            let table_def = self
                .catalog
                .get_table_mut(&table)
                .ok_or_else(|| Error::TableNotFound(table.clone()))?;

            root_page = table_def.root_page;
            table_cols = table_def.columns.clone();
            vector_col = table_def.vector_column();

            // Align values to table columns
            aligned_values = if let Some(cols) = columns {
                if cols.len() != values.len() {
                    return Err(Error::SqlSyntax(
                        "Column count does not match value count in INSERT".into(),
                    ));
                }
                let mut row_vals = vec![Value::Null; table_cols.len()];
                for (c_name, val) in cols.iter().zip(values) {
                    let idx = table_def
                        .column_index(c_name)
                        .ok_or_else(|| Error::Corrupted(format!("Unknown column: {c_name}")))?;
                    row_vals[idx] = val;
                }
                row_vals
            } else {
                if values.len() != table_cols.len() {
                    return Err(Error::SqlSyntax(format!(
                        "INSERT requires {} values, got {}",
                        table_cols.len(),
                        values.len()
                    )));
                }
                values
            };

            // Determine row_id (from primary key column or auto-increment)
            let pk_idx = table_def.primary_key_index();
            row_id = if let Some(idx) = pk_idx {
                match &aligned_values[idx] {
                    Value::Integer(i) => {
                        let id_val = *i as u64;
                        if id_val >= table_def.next_row_id {
                            table_def.next_row_id = id_val + 1;
                        }
                        id_val
                    }
                    Value::Null => {
                        let rid = table_def.next_row_id;
                        table_def.next_row_id += 1;
                        aligned_values[idx] = Value::Integer(rid as i64);
                        rid
                    }
                    _ => {
                        let rid = table_def.next_row_id;
                        table_def.next_row_id += 1;
                        rid
                    }
                }
            } else {
                let rid = table_def.next_row_id;
                table_def.next_row_id += 1;
                rid
            };
        }

        // Enforce column data type validation and coercion
        for (col_idx, col_def) in table_cols.iter().enumerate() {
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

        // Enforce NOT NULL constraints
        for (col_idx, col_def) in table_cols.iter().enumerate() {
            if col_def.not_null {
                if let Some(val) = aligned_values.get(col_idx) {
                    if val.is_null() {
                        return Err(Error::ConstraintViolation(format!(
                            "NOT NULL constraint failed: {table}.{}",
                            col_def.name
                        )));
                    }
                }
            }
        }

        // If table has a primary key or specific row_id and it already exists, enforce UNIQUE constraint or handle Upsert
        if let Some(existing_payload) = self.btree.search(pager, root_page, row_id)? {
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
                    let all_col_names: Vec<String> = table_cols.iter().map(|c| c.name.clone()).collect();
                    if let Ok(old_row) = decode_row(&existing_payload, &all_col_names) {
                        for index_def in self.catalog.indexes() {
                            if index_def.table.eq_ignore_ascii_case(&table) {
                                if let Some(val) = old_row.get_field_or_json_path(&index_def.column) {
                                    if !val.is_null() {
                                        let key = value_to_index_key(&val);
                                        if let Some(payload) = self.btree.search(pager, index_def.root_page, key)? {
                                            if let Ok(mut bucket) = serde_json::from_slice::<IndexBucket>(&payload) {
                                                bucket.remove_row_id(&val, row_id);
                                                if bucket.is_empty() {
                                                    let _ = self.btree.delete(pager, index_def.root_page, key);
                                                } else if let Ok(new_pl) = serde_json::to_vec(&bucket) {
                                                    let _ = self.btree.insert(pager, index_def.root_page, key, &new_pl);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(index) = self.vector_indexes.get_mut(&table.to_lowercase()) {
                        index.remove_vector(row_id);
                    }
                    let now_ts = self.next_temporal_timestamp();
                    self.close_temporal_version(pager, &table, row_id, now_ts)?;
                    self.btree.delete(pager, root_page, row_id)?;
                }
                OnConflict::DoUpdate(updates) => {
                    let all_col_names: Vec<String> = table_cols.iter().map(|c| c.name.clone()).collect();
                    let old_row = decode_row(&existing_payload, &all_col_names)?;
                    let mut values: Vec<Value> = old_row.values().to_vec();
                    for (col_name, new_val) in updates {
                        if let Some(col_idx) = table_cols.iter().position(|c| c.name.eq_ignore_ascii_case(&col_name)) {
                            if col_idx < values.len() {
                                values[col_idx] = new_val;
                            }
                        }
                    }
                    for (col_idx, col_def) in table_cols.iter().enumerate() {
                        if let Some(val) = values.get_mut(col_idx) {
                            let orig = std::mem::replace(val, Value::Null);
                            *val = col_def.data_type.coerce_value(orig).map_err(|e| match e {
                                Error::DimensionMismatch(exp, got) => Error::DimensionMismatch(exp, got),
                                Error::ConstraintViolation(msg) => {
                                    Error::ConstraintViolation(format!("{table}.{}: {msg}", col_def.name))
                                }
                                other => other,
                            })?;
                            if col_def.not_null && val.is_null() {
                                return Err(Error::ConstraintViolation(format!(
                                    "NOT NULL constraint failed: {table}.{}",
                                    col_def.name
                                )));
                            }
                        }
                    }
                    for index_def in self.catalog.indexes() {
                        if index_def.table.eq_ignore_ascii_case(&table) {
                            if let Some(val) = old_row.get_field_or_json_path(&index_def.column) {
                                if !val.is_null() {
                                    let key = value_to_index_key(&val);
                                    if let Some(payload) = self.btree.search(pager, index_def.root_page, key)? {
                                        if let Ok(mut bucket) = serde_json::from_slice::<IndexBucket>(&payload) {
                                            bucket.remove_row_id(&val, row_id);
                                            if bucket.is_empty() {
                                                let _ = self.btree.delete(pager, index_def.root_page, key);
                                            } else if let Ok(new_pl) = serde_json::to_vec(&bucket) {
                                                let _ = self.btree.insert(pager, index_def.root_page, key, &new_pl);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(index) = self.vector_indexes.get_mut(&table.to_lowercase()) {
                        index.remove_vector(row_id);
                    }
                    let now_ts = self.next_temporal_timestamp();
                    self.close_temporal_version(pager, &table, row_id, now_ts)?;
                    self.btree.delete(pager, root_page, row_id)?;

                    aligned_values = values;
                }
            }
        }

        // Encode row to binary record payload
        let payload = encode_row(&aligned_values);

        // Insert into B+Tree
        self.btree.insert(pager, root_page, row_id, &payload)?;

        // Record temporal snapshot for time-travel queries
        let now_ts = self.next_temporal_timestamp();
        self.record_temporal_version(pager, &table, row_id, &payload, now_ts, u64::MAX)?;

        // Update vector index if present
        if let Some((v_idx, _)) = vector_col {
            if let Some(Value::Vector(vec_data)) = aligned_values.get(v_idx) {
                let index = self
                    .vector_indexes
                    .entry(table.to_lowercase())
                    .or_insert_with(|| HnswIndex::new(vec_data.len(), DistanceMetric::Cosine));
                index.insert_vector(row_id, vec_data)?;
                if !pager.is_in_transaction() {
                    let _ = self.persist_vector_indexes_to_disk(pager);
                }
            }
        }

        // Update secondary indexes with inverted posting lists
        let temp_row = Row::new(
            table_cols.iter().map(|c| c.name.clone()).collect(),
            aligned_values.clone(),
        );
        for index_def in self.catalog.indexes() {
            if index_def.table.eq_ignore_ascii_case(&table) {
                if let Some(val) = temp_row.get_field_or_json_path(&index_def.column) {
                    if !val.is_null() {
                        let key = value_to_index_key(&val);
                        let mut bucket = if let Some(payload) = self.btree.search(pager, index_def.root_page, key)? {
                            serde_json::from_slice::<IndexBucket>(&payload).unwrap_or_else(|_| IndexBucket::new())
                        } else {
                            IndexBucket::new()
                        };
                        bucket.add_row_id(val, row_id);
                        let idx_payload = serde_json::to_vec(&bucket)
                            .map_err(|e| Error::Corrupted(format!("Failed to serialize index bucket: {e}")))?;
                        self.btree.insert(pager, index_def.root_page, key, &idx_payload)?;
                    }
                }
            }
        }

        let returned_rows = if let Some(ref ret_cols) = returning {
            let full_row = Row::new(
                table_cols.iter().map(|c| c.name.clone()).collect(),
                aligned_values,
            );
            if ret_cols.len() == 1 && ret_cols[0] == "*" {
                vec![full_row]
            } else {
                vec![project_row(&full_row, ret_cols)?]
            }
        } else {
            Vec::new()
        };

        Ok((1, returned_rows))
    }

    fn execute_update(
        &mut self,
        pager: &mut Pager,
        table: String,
        assignments: Vec<(String, Value)>,
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

        // Validate assignment column names
        for (col_name, _) in &assignments {
            if table_def.column_index(col_name).is_none() {
                return Err(Error::Corrupted(format!("Unknown column in SET: {col_name}")));
            }
        }

        let resolved_where = if let Some(ref expr) = where_clause {
            Some(self.resolve_where_expr(pager, expr)?)
        } else {
            None
        };

        let cells = self.btree.scan(pager, root_page)?;
        let mut updated_count = 0;
        let mut returned_rows = Vec::new();

        for cell in cells {
            let full_row = decode_row(&cell.payload, &all_col_names)?;
            let matches = if let Some(ref r_expr) = resolved_where {
                row_matches_resolved(&full_row, r_expr)
            } else {
                true
            };

            if matches {
                let mut values: Vec<Value> = full_row.values().to_vec();
                for (col_name, new_val) in &assignments {
                    if let Some(col_idx) = table_def.column_index(col_name) {
                        if col_idx < values.len() {
                            values[col_idx] = new_val.clone();
                        }
                    }
                }

                // Enforce column data type validation and coercion
                for (col_idx, col_def) in table_def.columns.iter().enumerate() {
                    if let Some(val) = values.get_mut(col_idx) {
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

                // Enforce NOT NULL constraints
                for (col_idx, col_def) in table_def.columns.iter().enumerate() {
                    if col_def.not_null {
                        if let Some(val) = values.get(col_idx) {
                            if val.is_null() {
                                return Err(Error::ConstraintViolation(format!(
                                    "NOT NULL constraint failed: {table}.{}",
                                    col_def.name
                                )));
                            }
                        }
                    }
                }

                let pk_col_idx = table_def.primary_key_index();
                let new_row_id = if let Some(pk_idx) = pk_col_idx {
                    if let Some(Value::Integer(id_val)) = values.get(pk_idx) {
                        *id_val as u64
                    } else {
                        cell.row_id
                    }
                } else {
                    cell.row_id
                };

                // If primary key was modified to a different ID, ensure it does not collide
                if new_row_id != cell.row_id {
                    if let Some(_existing) = self.btree.search(pager, root_page, new_row_id)? {
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
                }

                let new_payload = encode_row(&values);
                self.btree.delete(pager, root_page, cell.row_id)?;
                self.btree.insert(pager, root_page, new_row_id, &new_payload)?;

                // Update temporal snapshot for time-travel queries
                let now_ts = self.next_temporal_timestamp();
                self.close_temporal_version(pager, &table, cell.row_id, now_ts)?;
                self.record_temporal_version(pager, &table, new_row_id, &new_payload, now_ts, u64::MAX)?;

                // Update vector index if vector column was updated or row_id changed
                if let Some((v_idx, _)) = table_def.vector_column() {
                    if let Some(Value::Vector(vec_data)) = values.get(v_idx) {
                        let index = self
                            .vector_indexes
                            .entry(table.to_lowercase())
                            .or_insert_with(|| HnswIndex::new(vec_data.len(), DistanceMetric::Cosine));
                        index.remove_vector(cell.row_id);
                        index.insert_vector(new_row_id, vec_data)?;
                    } else if new_row_id != cell.row_id {
                        if let Some(index) = self.vector_indexes.get_mut(&table.to_lowercase()) {
                            index.remove_vector(cell.row_id);
                        }
                    }
                }

                // Update secondary indexes with inverted posting lists
                for index_def in self.catalog.indexes() {
                    if index_def.table.eq_ignore_ascii_case(&table) {
                        if let Some(old_val) = full_row.get_field_or_json_path(&index_def.column) {
                            if !old_val.is_null() {
                                let old_key = value_to_index_key(&old_val);
                                if let Some(payload) = self.btree.search(pager, index_def.root_page, old_key)? {
                                    if let Ok(mut bucket) = serde_json::from_slice::<IndexBucket>(&payload) {
                                        bucket.remove_row_id(&old_val, cell.row_id);
                                        if bucket.is_empty() {
                                            let _ = self.btree.delete(pager, index_def.root_page, old_key);
                                        } else if let Ok(new_pl) = serde_json::to_vec(&bucket) {
                                            let _ = self.btree.insert(pager, index_def.root_page, old_key, &new_pl);
                                        }
                                    }
                                }
                            }
                        }
                        let updated_temp_row = Row::new(all_col_names.clone(), values.clone());
                        if let Some(new_val) = updated_temp_row.get_field_or_json_path(&index_def.column) {
                            if !new_val.is_null() {
                                let new_key = value_to_index_key(&new_val);
                                let mut bucket = if let Some(payload) = self.btree.search(pager, index_def.root_page, new_key)? {
                                    serde_json::from_slice::<IndexBucket>(&payload).unwrap_or_else(|_| IndexBucket::new())
                                } else {
                                    IndexBucket::new()
                                };
                                bucket.add_row_id(new_val, new_row_id);
                                if let Ok(new_pl) = serde_json::to_vec(&bucket) {
                                    let _ = self.btree.insert(pager, index_def.root_page, new_key, &new_pl);
                                }
                            }
                        }
                    }
                }

                if new_row_id >= table_def.next_row_id {
                    if let Some(tdef_mut) = self.catalog.get_table_mut(&table) {
                        tdef_mut.next_row_id = new_row_id + 1;
                    }
                }

                if let Some(ref ret_cols) = returning {
                    let updated_row = Row::new(all_col_names.clone(), values.clone());
                    if ret_cols.len() == 1 && ret_cols[0] == "*" {
                        returned_rows.push(updated_row);
                    } else {
                        returned_rows.push(project_row(&updated_row, ret_cols)?);
                    }
                }

                updated_count += 1;
            }
        }

        Ok((updated_count, returned_rows))
    }

    fn execute_delete(
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
        let all_col_names = table_def.column_names();
        let cells = self.btree.scan(pager, root_page)?;
        let mut deleted_count = 0;
        let mut returned_rows = Vec::new();

        let resolved_where = if let Some(ref expr) = where_clause {
            Some(self.resolve_where_expr(pager, expr)?)
        } else {
            None
        };

        for cell in cells {
            let full_row = decode_row(&cell.payload, &all_col_names)?;
            let matches = if let Some(ref r_expr) = resolved_where {
                row_matches_resolved(&full_row, r_expr)
            } else {
                true
            };

            if matches {
                if let Some(ref ret_cols) = returning {
                    if ret_cols.len() == 1 && ret_cols[0] == "*" {
                        returned_rows.push(full_row.clone());
                    } else {
                        returned_rows.push(project_row(&full_row, ret_cols)?);
                    }
                }

                self.btree.delete(pager, root_page, cell.row_id)?;
                let now_ts = self.next_temporal_timestamp();
                self.close_temporal_version(pager, &table, cell.row_id, now_ts)?;
                if let Some(index) = self.vector_indexes.get_mut(&table.to_lowercase()) {
                    index.remove_vector(cell.row_id);
                }
                // Delete from secondary indexes with inverted posting lists
                for index_def in self.catalog.indexes() {
                    if index_def.table.eq_ignore_ascii_case(&table) {
                        if let Some(val) = full_row.get_field_or_json_path(&index_def.column) {
                            if !val.is_null() {
                                let key = value_to_index_key(&val);
                                if let Some(payload) = self.btree.search(pager, index_def.root_page, key)? {
                                    if let Ok(mut bucket) = serde_json::from_slice::<IndexBucket>(&payload) {
                                        bucket.remove_row_id(&val, cell.row_id);
                                        if bucket.is_empty() {
                                            let _ = self.btree.delete(pager, index_def.root_page, key);
                                        } else if let Ok(new_pl) = serde_json::to_vec(&bucket) {
                                            let _ = self.btree.insert(pager, index_def.root_page, key, &new_pl);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                deleted_count += 1;
            }
        }

        Ok((deleted_count, returned_rows))
    }
}

/// Match a SQL LIKE pattern with '%' (wildcard multi-char) and '_' (wildcard single-char), case-insensitively
pub fn sql_like_match(pattern: &str, text: &str) -> bool {
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
        BinaryOp::IsNull    => return actual.is_null(),
        BinaryOp::IsNotNull => return !actual.is_null(),
        _ => {}
    }

    if actual.is_null() || target.is_null() {
        return false;
    }

    match op {
        BinaryOp::Equals    => actual == target,
        BinaryOp::NotEquals => actual != target,
        BinaryOp::GreaterThan => actual.compare(target) == std::cmp::Ordering::Greater,
        BinaryOp::LessThan    => actual.compare(target) == std::cmp::Ordering::Less,
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
    match expr {
        ResolvedWhereExpr::Condition(cond) => {
            let val = match row.get_field_or_json_path(&cond.column) {
                Some(v) => v,
                None => return false,
            };
            matches_condition(&val, &cond.op, &cond.value)
        }
        ResolvedWhereExpr::And(left, right) => {
            row_matches_resolved(row, left) && row_matches_resolved(row, right)
        }
        ResolvedWhereExpr::Or(left, right) => {
            row_matches_resolved(row, left) || row_matches_resolved(row, right)
        }
        ResolvedWhereExpr::InList { column, values, negated } => {
            let val = match row.get_field_or_json_path(column) {
                Some(v) => v,
                None => return false,
            };
            if val.is_null() {
                return false;
            }
            let has_null = values.iter().any(|v| v.is_null());
            let in_list = values.iter().any(|target| val.compare(target).is_eq());
            if *negated {
                if has_null {
                    false
                } else {
                    !in_list
                }
            } else {
                in_list
            }
        }
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

fn parse_col_and_alias(col: &str) -> (&str, Option<&str>) {
    let trimmed = col.trim();
    if let Some(pos) = trimmed.to_ascii_uppercase().rfind(" AS ") {
        let expr = trimmed[..pos].trim();
        let alias = trimmed[pos + 4..].trim();
        if !alias.is_empty() {
            return (expr, Some(alias));
        }
    }
    (trimmed, None)
}

fn has_window_function(col: &str) -> bool {
    let u = col.to_ascii_uppercase();
    u.contains(" OVER ") || u.contains(" OVER(")
}

#[derive(Debug, Clone)]
struct ParsedWindowCol {
    raw: String,
    expr: String,
    alias: Option<String>,
    func: String,
    arg: String,
    offset: usize,
    partition_by: Vec<String>,
    order_by: Option<(String, bool)>,
}

fn parse_window_function(col_str: &str) -> Option<ParsedWindowCol> {
    let (expr, alias) = parse_col_and_alias(col_str);
    let u = expr.to_ascii_uppercase();
    let over_pos = u.find(" OVER ")
        .or_else(|| u.find(" OVER("))?;

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
    } else if func == "NTILE" {
        if let Ok(n) = arg_inner.parse::<usize>() {
            offset = n.max(1);
        }
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
        let end_p = after_p.to_ascii_uppercase().find("ORDER BY")
            .unwrap_or_else(|| after_p.len());
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
            let is_asc = if parts.len() > 1 && parts[1].eq_ignore_ascii_case("DESC") {
                false
            } else {
                true
            };
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

fn evaluate_window_functions(rows: &mut Vec<Row>, columns: &[String]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }

    let n_rows = rows.len();

    for col in columns {
        if let Some(w) = parse_window_function(col) {
            let mut partitions: HashMap<Vec<String>, Vec<usize>> = HashMap::new();
            for i in 0..n_rows {
                let r = &rows[i];
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
                                let cur_val = rows[row_idx].get_value(o_col).cloned().unwrap_or(Value::Null);
                                if pos > 0 {
                                    if let Some(ref prev) = prev_order_val {
                                        if prev != &cur_val {
                                            current_rank = pos + 1;
                                        }
                                    }
                                }
                                prev_order_val = Some(cur_val);
                            } else {
                                current_rank = pos + 1;
                            }
                            Value::Integer(current_rank as i64)
                        }
                        "DENSE_RANK" => {
                            if let Some((ref o_col, _)) = w.order_by {
                                let cur_val = rows[row_idx].get_value(o_col).cloned().unwrap_or(Value::Null);
                                if pos > 0 {
                                    if let Some(ref prev) = prev_order_val {
                                        if prev != &cur_val {
                                            current_dense_rank += 1;
                                        }
                                    }
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
                                rows[target_row_idx].get_value(&w.arg).cloned().unwrap_or(Value::Null)
                            } else {
                                Value::Null
                            }
                        }
                        "LEAD" => {
                            if pos + w.offset < p_len {
                                let target_row_idx = p_indices[pos + w.offset];
                                rows[target_row_idx].get_value(&w.arg).cloned().unwrap_or(Value::Null)
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
                                    if let Some(v) = rows[idx].get_value(&w.arg) {
                                        if v != &Value::Null {
                                            count += 1;
                                        }
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
                                if let Some(v) = rows[idx].get_value(&w.arg) {
                                    if v != &Value::Null {
                                        if min_val.is_none() || min_val.as_ref().map(|m| v.compare(m).is_lt()).unwrap_or(false) {
                                            min_val = Some(v.clone());
                                        }
                                    }
                                }
                            }
                            min_val.unwrap_or(Value::Null)
                        }
                        "MAX" => {
                            let mut max_val: Option<Value> = None;
                            for &idx in &p_indices {
                                if let Some(v) = rows[idx].get_value(&w.arg) {
                                    if v != &Value::Null {
                                        if max_val.is_none() || max_val.as_ref().map(|m| v.compare(m).is_gt()).unwrap_or(false) {
                                            max_val = Some(v.clone());
                                        }
                                    }
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

fn project_row(row: &Row, requested_cols: &[String]) -> Result<Row> {
    let mut names = Vec::with_capacity(requested_cols.len());
    let mut vals = Vec::with_capacity(requested_cols.len());
    for col in requested_cols {
        let (expr, alias) = parse_col_and_alias(col);
        let val = row.get_field_or_json_path(expr)
            .or_else(|| row.get_field_or_json_path(col))
            .unwrap_or(Value::Null);
        names.push(alias.unwrap_or(expr).to_string());
        vals.push(val);
    }
    Ok(Row::new(names, vals))
}

fn is_aggregate_query(columns: &[String]) -> bool {
    columns.iter().any(|c| {
        let (expr, _) = parse_col_and_alias(c);
        let upper = expr.to_ascii_uppercase();
        if upper.contains(" OVER ") || upper.contains(" OVER(") {
            return false;
        }
        upper.starts_with("COUNT(")
            || upper.starts_with("SUM(")
            || upper.starts_with("AVG(")
            || upper.starts_with("MIN(")
            || upper.starts_with("MAX(")
    })
}

fn compute_aggregate(func: &str, arg: &str, rows: &[Row]) -> Value {
    let is_count_all = arg == "*"
        || arg == "1"
        || (!arg.is_empty() && arg.chars().all(|c| c.is_ascii_digit()));

    if func == "COUNT" && is_count_all {
        return Value::Integer(rows.len() as i64);
    }

    let mut acc = crate::sql::vectorized::VectorizedAccumulator::new();
    for r in rows {
        if let Some(v) = r.get_value(arg) {
            acc.accumulate_values(std::iter::once(v));
        }
    }

    match func {
        "COUNT" => acc.finalize_count(is_count_all),
        "SUM" => acc.finalize_sum(),
        "AVG" => acc.finalize_avg(),
        "MIN" => acc.finalize_min(),
        "MAX" => acc.finalize_max(),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::parser::parse_sql;

    #[test]
    fn test_executor_create_insert_select_and_vector() {
        let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
        let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

        // 1. CREATE TABLE
        let create_sql = "CREATE TABLE articles (id INTEGER PRIMARY KEY, title TEXT, embedding VECTOR(3));";
        let res = executor
            .execute(&mut pager, parse_sql(create_sql).unwrap())
            .expect("Execute CREATE TABLE");
        assert_eq!(res, 1);

        // 2. INSERT records
        executor
            .execute(
                &mut pager,
                parse_sql("INSERT INTO articles (id, title, embedding) VALUES (1, 'Safe Rust Systems', [1.0, 0.0, 0.0]);").unwrap(),
            )
            .expect("Insert 1");

        executor
            .execute(
                &mut pager,
                parse_sql("INSERT INTO articles (id, title, embedding) VALUES (2, 'AI Database Engines', [0.0, 1.0, 0.0]);").unwrap(),
            )
            .expect("Insert 2");

        // 3. SELECT *
        let rows = executor
            .query(&mut pager, parse_sql("SELECT id, title FROM articles;").unwrap())
            .expect("Query all");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get::<i64>("id").unwrap(), 1);
        assert_eq!(rows[0].get::<String>("title").unwrap(), "Safe Rust Systems");

        // 4. POINT LOOKUP by Primary Key (WHERE id = 2)
        let point_rows = executor
            .query(&mut pager, parse_sql("SELECT title FROM articles WHERE id = 2;").unwrap())
            .expect("Query point");
        assert_eq!(point_rows.len(), 1);
        assert_eq!(point_rows[0].get::<String>("title").unwrap(), "AI Database Engines");

        // 5. VECTOR SIMILARITY SEARCH
        let vector_rows = executor
            .query(
                &mut pager,
                parse_sql("SELECT id, title FROM articles VECTOR NEAR embedding = [0.95, 0.05, 0.0] TOP 1;").unwrap(),
            )
            .expect("Vector search");
        assert_eq!(vector_rows.len(), 1);
        assert_eq!(vector_rows[0].get::<String>("title").unwrap(), "Safe Rust Systems");
    }

    #[test]
    fn test_executor_order_by_aggregates_and_join() {
        let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
        let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

        executor
            .execute(&mut pager, parse_sql("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);").unwrap())
            .unwrap();
        executor
            .execute(&mut pager, parse_sql("CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, amount REAL);").unwrap())
            .unwrap();

        executor
            .execute(&mut pager, parse_sql("INSERT INTO users (id, name) VALUES (1, 'Alice');").unwrap())
            .unwrap();
        executor
            .execute(&mut pager, parse_sql("INSERT INTO users (id, name) VALUES (2, 'Bob');").unwrap())
            .unwrap();

        executor
            .execute(&mut pager, parse_sql("INSERT INTO orders (id, user_id, amount) VALUES (10, 1, 150.0);").unwrap())
            .unwrap();
        executor
            .execute(&mut pager, parse_sql("INSERT INTO orders (id, user_id, amount) VALUES (20, 1, 50.0);").unwrap())
            .unwrap();
        executor
            .execute(&mut pager, parse_sql("INSERT INTO orders (id, user_id, amount) VALUES (30, 2, 200.0);").unwrap())
            .unwrap();

        // 1. ORDER BY DESC
        let ordered = executor
            .query(&mut pager, parse_sql("SELECT id, amount FROM orders ORDER BY amount DESC;").unwrap())
            .unwrap();
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].get::<f64>("amount").unwrap(), 200.0);
        assert_eq!(ordered[1].get::<f64>("amount").unwrap(), 150.0);
        assert_eq!(ordered[2].get::<f64>("amount").unwrap(), 50.0);

        // 2. Aggregates: COUNT, SUM, AVG, MIN, MAX
        let agg = executor
            .query(
                &mut pager,
                parse_sql("SELECT COUNT(*), SUM(amount), AVG(amount), MIN(amount), MAX(amount) FROM orders;").unwrap(),
            )
            .unwrap();
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].get::<i64>("COUNT(*)").unwrap(), 3);
        assert_eq!(agg[0].get::<f64>("SUM(amount)").unwrap(), 400.0);
        assert_eq!(agg[0].get::<f64>("AVG(amount)").unwrap(), 400.0 / 3.0);
        assert_eq!(agg[0].get::<f64>("MIN(amount)").unwrap(), 50.0);
        assert_eq!(agg[0].get::<f64>("MAX(amount)").unwrap(), 200.0);

        // 3. INNER JOIN
        let joined = executor
            .query(
                &mut pager,
                parse_sql("SELECT users.name, orders.amount FROM users INNER JOIN orders ON users.id = orders.user_id ORDER BY orders.amount ASC;").unwrap(),
            )
            .unwrap();
        assert_eq!(joined.len(), 3);
        assert_eq!(joined[0].get::<String>("users.name").unwrap(), "Alice");
        assert_eq!(joined[0].get::<f64>("orders.amount").unwrap(), 50.0);
        assert_eq!(joined[2].get::<String>("users.name").unwrap(), "Bob");
        assert_eq!(joined[2].get::<f64>("orders.amount").unwrap(), 200.0);
    }

    #[test]
    fn test_graph_match_cypher_query() {
        let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
        let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

        // Insert nodes and edge
        executor
            .execute(
                &mut pager,
                parse_sql(r#"GRAPH INSERT NODE 1 LABEL "Person" PROPERTIES "{\"name\":\"Faiz\",\"role\":\"founder\"}";"#).unwrap(),
            )
            .unwrap();
        executor
            .execute(
                &mut pager,
                parse_sql(r#"GRAPH INSERT NODE 2 LABEL "Project" PROPERTIES "{\"title\":\"TapirusDB\"}";"#).unwrap(),
            )
            .unwrap();
        executor
            .execute(
                &mut pager,
                parse_sql(r#"GRAPH INSERT EDGE 1 2 LABEL "CREATOR_OF";"#).unwrap(),
            )
            .unwrap();

        // 1. Declarative Cypher-style GRAPH MATCH
        let query_sql = "GRAPH MATCH (a:Person)-[r:CREATOR_OF]->(b:Project) WHERE a.id = 1 RETURN a.name, b.title;";
        let rows = executor
            .query(&mut pager, parse_sql(query_sql).unwrap())
            .expect("Execute graph match");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<String>("a.name").unwrap(), "Faiz");
        assert_eq!(rows[0].get::<String>("b.title").unwrap(), "TapirusDB");

        // 2. Direct MATCH without GRAPH keyword
        let match_sql = "MATCH (a)-[r]->(b) RETURN a.name, b.title;";
        let rows2 = executor
            .query(&mut pager, parse_sql(match_sql).unwrap())
            .expect("Execute direct match");
        assert_eq!(rows2.len(), 1);
        assert_eq!(rows2[0].get::<String>("a.name").unwrap(), "Faiz");
    }

    #[test]
    fn test_executor_window_functions() {
        let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
        let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

        executor
            .execute(&mut pager, parse_sql("CREATE TABLE emp (id INTEGER PRIMARY KEY, name TEXT, dept TEXT, salary INTEGER);").unwrap())
            .unwrap();

        executor.execute(&mut pager, parse_sql("INSERT INTO emp (id, name, dept, salary) VALUES (1, 'Alice', 'Eng', 9000);").unwrap()).unwrap();
        executor.execute(&mut pager, parse_sql("INSERT INTO emp (id, name, dept, salary) VALUES (2, 'Bob', 'Eng', 8000);").unwrap()).unwrap();
        executor.execute(&mut pager, parse_sql("INSERT INTO emp (id, name, dept, salary) VALUES (3, 'Charlie', 'Eng', 8000);").unwrap()).unwrap();
        executor.execute(&mut pager, parse_sql("INSERT INTO emp (id, name, dept, salary) VALUES (4, 'David', 'Sales', 7000);").unwrap()).unwrap();
        executor.execute(&mut pager, parse_sql("INSERT INTO emp (id, name, dept, salary) VALUES (5, 'Eve', 'Sales', 6000);").unwrap()).unwrap();

        // 1. ROW_NUMBER with PARTITION BY and ORDER BY
        let sql_rn = "SELECT id, name, dept, ROW_NUMBER() OVER (PARTITION BY dept ORDER BY salary DESC) AS rn FROM emp ORDER BY id ASC;";
        let rows_rn = executor.query(&mut pager, parse_sql(sql_rn).unwrap()).expect("Query ROW_NUMBER");
        assert_eq!(rows_rn.len(), 5);
        // Alice is rank 1 in Eng
        assert_eq!(rows_rn[0].get::<i64>("rn").unwrap(), 1);
        // David is rank 1 in Sales
        assert_eq!(rows_rn[3].get::<i64>("rn").unwrap(), 1);
        // Eve is rank 2 in Sales
        assert_eq!(rows_rn[4].get::<i64>("rn").unwrap(), 2);

        // 2. DENSE_RANK
        let sql_dr = "SELECT id, salary, DENSE_RANK() OVER (ORDER BY salary DESC) AS drank FROM emp ORDER BY salary DESC;";
        let rows_dr = executor.query(&mut pager, parse_sql(sql_dr).unwrap()).expect("Query DENSE_RANK");
        assert_eq!(rows_dr[0].get::<i64>("drank").unwrap(), 1); // 9000
        assert_eq!(rows_dr[1].get::<i64>("drank").unwrap(), 2); // 8000
        assert_eq!(rows_dr[2].get::<i64>("drank").unwrap(), 2); // 8000 tie
        assert_eq!(rows_dr[3].get::<i64>("drank").unwrap(), 3); // 7000

        // 3. LAG and LEAD
        let sql_lag = "SELECT id, salary, LAG(salary, 1) OVER (ORDER BY id ASC) AS prev_sal, LEAD(salary, 1) OVER (ORDER BY id ASC) AS next_sal FROM emp ORDER BY id ASC;";
        let rows_lag = executor.query(&mut pager, parse_sql(sql_lag).unwrap()).expect("Query LAG and LEAD");
        assert_eq!(rows_lag[0].get_value("prev_sal"), Some(&Value::Null));
        assert_eq!(rows_lag[0].get::<i64>("next_sal").unwrap(), 8000);
        assert_eq!(rows_lag[1].get::<i64>("prev_sal").unwrap(), 9000);

        // 4. Window SUM with PARTITION BY
        let sql_sum = "SELECT id, dept, SUM(salary) OVER (PARTITION BY dept) AS total_dept FROM emp ORDER BY id ASC;";
        let rows_sum = executor.query(&mut pager, parse_sql(sql_sum).unwrap()).expect("Query Window SUM");
        assert_eq!(rows_sum[0].get::<i64>("total_dept").unwrap(), 25000); // Eng: 9000+8000+8000
        assert_eq!(rows_sum[3].get::<i64>("total_dept").unwrap(), 13000); // Sales: 7000+6000
    }

    #[test]
    fn test_executor_graph_algorithms() {
        let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
        let mut executor = SQLExecutor::new(&mut pager).expect("Init executor");

        executor.execute(&mut pager, parse_sql("GRAPH INSERT NODE 1 LABEL \"Server\" PROPERTIES \"{}\";").unwrap()).unwrap();
        executor.execute(&mut pager, parse_sql("GRAPH INSERT NODE 2 LABEL \"Database\" PROPERTIES \"{}\";").unwrap()).unwrap();
        executor.execute(&mut pager, parse_sql("GRAPH INSERT NODE 3 LABEL \"Cache\" PROPERTIES \"{}\";").unwrap()).unwrap();
        executor.execute(&mut pager, parse_sql("GRAPH INSERT EDGE 1 2 LABEL \"CONNECTS\";").unwrap()).unwrap();
        executor.execute(&mut pager, parse_sql("GRAPH INSERT EDGE 2 3 LABEL \"CONNECTS\";").unwrap()).unwrap();
        executor.execute(&mut pager, parse_sql("GRAPH INSERT EDGE 3 1 LABEL \"CONNECTS\";").unwrap()).unwrap();

        // 1. GRAPH ALGORITHM PAGERANK
        let pr_rows = executor.query(&mut pager, parse_sql("GRAPH ALGORITHM PAGERANK;").unwrap()).expect("PageRank query");
        assert_eq!(pr_rows.len(), 3);
        assert_eq!(pr_rows[0].columns(), &["node_id", "label", "pagerank"]);

        // 2. GRAPH ALGORITHM CONNECTED_COMPONENTS
        let cc_rows = executor.query(&mut pager, parse_sql("GRAPH ALGORITHM CONNECTED_COMPONENTS;").unwrap()).expect("WCC query");
        assert_eq!(cc_rows.len(), 3);
        assert_eq!(cc_rows[0].get::<i64>("component_id").unwrap(), cc_rows[1].get::<i64>("component_id").unwrap());

        // 3. GRAPH ALGORITHM BETWEENNESS
        let bc_rows = executor.query(&mut pager, parse_sql("GRAPH ALGORITHM BETWEENNESS;").unwrap()).expect("BC query");
        assert_eq!(bc_rows.len(), 3);

        // 4. GRAPH ALGORITHM LOUVAIN
        let lv_rows = executor.query(&mut pager, parse_sql("GRAPH ALGORITHM LOUVAIN;").unwrap()).expect("Louvain query");
        assert_eq!(lv_rows.len(), 3);
    }
}
