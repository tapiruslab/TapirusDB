//! Database Schema Catalog managing table definitions and column metadata.
//!
//! Master schema records are persisted in the B+Tree rooted at Page 1 (after the 100-byte DatabaseHeader).

use crate::btree::{BTreeStorage, engine};
use crate::error::{Error, Result};
use crate::pager::{PageId, Pager};
use crate::traits::Value;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Supported column data types in TapirusDB
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataType {
    /// 64-bit integer
    Integer,
    /// 64-bit float
    Real,
    /// UTF-8 string
    Text,
    /// Binary blob
    Blob,
    /// High-dimensional dense vector with specified dimensions
    Vector(usize),
}

impl DataType {
    /// Validates or coerces a `Value` to conform to this column's declared `DataType`.
    ///
    /// # Errors
    /// - Returns `Error::DimensionMismatch` if a vector's dimension does not match the declared `Vector(dims)`.
    /// - Returns `Error::ConstraintViolation` if a value cannot be safely stored in the column's data type.
    pub fn coerce_value(&self, val: Value) -> Result<Value> {
        match self {
            DataType::Integer => match val {
                Value::Integer(i) => Ok(Value::Integer(i)),
                Value::Real(r) => {
                    if r.fract() == 0.0 && r >= (i64::MIN as f64) && r <= (i64::MAX as f64) {
                        Ok(Value::Integer(r as i64))
                    } else {
                        Err(Error::ConstraintViolation(format!(
                            "Cannot store floating-point {r} in INTEGER column without truncation"
                        )))
                    }
                }
                Value::Text(ref s) => {
                    if let Ok(i) = s.parse::<i64>() {
                        Ok(Value::Integer(i))
                    } else {
                        Err(Error::ConstraintViolation(format!(
                            "Cannot convert text '{s}' to INTEGER"
                        )))
                    }
                }
                Value::Null => Ok(Value::Null),
                other => Err(Error::ConstraintViolation(format!(
                    "Invalid value {other:?} for INTEGER column"
                ))),
            },
            DataType::Real => match val {
                Value::Real(r) => Ok(Value::Real(r)),
                Value::Integer(i) => Ok(Value::Real(i as f64)),
                Value::Text(ref s) => {
                    if let Ok(r) = s.parse::<f64>() {
                        Ok(Value::Real(r))
                    } else {
                        Err(Error::ConstraintViolation(format!(
                            "Cannot convert text '{s}' to REAL"
                        )))
                    }
                }
                Value::Null => Ok(Value::Null),
                other => Err(Error::ConstraintViolation(format!(
                    "Invalid value {other:?} for REAL column"
                ))),
            },
            DataType::Text => match val {
                Value::Text(s) => Ok(Value::Text(s)),
                Value::Integer(i) => Ok(Value::Text(i.to_string())),
                Value::Real(r) => Ok(Value::Text(r.to_string())),
                Value::Null => Ok(Value::Null),
                other => Err(Error::ConstraintViolation(format!(
                    "Invalid value {other:?} for TEXT column"
                ))),
            },
            DataType::Blob => match val {
                Value::Blob(b) => Ok(Value::Blob(b)),
                Value::Null => Ok(Value::Null),
                other => Err(Error::ConstraintViolation(format!(
                    "Invalid value {other:?} for BLOB column"
                ))),
            },
            DataType::Vector(expected_dim) => match val {
                Value::Vector(v) => {
                    if v.len() != *expected_dim {
                        Err(Error::DimensionMismatch(*expected_dim, v.len()))
                    } else {
                        Ok(Value::Vector(v))
                    }
                }
                Value::Text(ref s) => {
                    let trimmed = s.trim();
                    if trimmed.starts_with('[') && trimmed.ends_with(']') {
                        let inner = &trimmed[1..trimmed.len() - 1];
                        let parsed: std::result::Result<Vec<f32>, _> = inner
                            .split(',')
                            .map(|part| part.trim().parse::<f32>())
                            .collect();
                        if let Ok(vec) = parsed {
                            if vec.len() != *expected_dim {
                                return Err(Error::DimensionMismatch(*expected_dim, vec.len()));
                            }
                            return Ok(Value::Vector(vec));
                        }
                    }
                    Err(Error::ConstraintViolation(format!(
                        "Expected VECTOR({expected_dim}), got Text(\"{s}\")"
                    )))
                }
                Value::Null => Ok(Value::Null),
                other => Err(Error::ConstraintViolation(format!(
                    "Expected VECTOR({expected_dim}), got {other:?}"
                ))),
            },
        }
    }
}

/// Referential action for FOREIGN KEY ON DELETE / ON UPDATE clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ForeignKeyAction {
    /// Reject mutation if child rows reference parent key (SQLite default).
    #[default]
    NoAction,
    /// Reject mutation immediately if child rows reference parent key.
    Restrict,
    /// Propagate DELETE or UPDATE to matching child rows.
    Cascade,
    /// Set referencing columns in matching child rows to NULL.
    SetNull,
}

/// Column-level FOREIGN KEY reference specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKeyRef {
    /// Referenced parent table name.
    pub table: String,
    /// Referenced parent column name (defaults to parent's primary key when None).
    pub column: Option<String>,
    /// Action when parent row is deleted.
    #[serde(default)]
    pub on_delete: ForeignKeyAction,
    /// Action when parent referenced key is updated.
    #[serde(default)]
    pub on_update: ForeignKeyAction,
}

/// Table-level or composite FOREIGN KEY constraint definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableForeignKey {
    /// Optional constraint name.
    #[serde(default)]
    pub name: Option<String>,
    /// Child column names.
    pub columns: Vec<String>,
    /// Referenced parent table name.
    pub ref_table: String,
    /// Referenced parent column names (empty means parent primary key).
    #[serde(default)]
    pub ref_columns: Vec<String>,
    /// Action when parent row is deleted.
    #[serde(default)]
    pub on_delete: ForeignKeyAction,
    /// Action when parent referenced key is updated.
    #[serde(default)]
    pub on_update: ForeignKeyAction,
}

/// Table-level CHECK constraint definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckConstraint {
    /// Optional constraint name.
    #[serde(default)]
    pub name: Option<String>,
    /// Raw SQL boolean expression evaluated against candidate rows.
    pub expr_sql: String,
}

/// Column DEFAULT value expression (comparable with Eq via float bit encoding).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefaultValue {
    /// Explicit NULL default.
    Null,
    /// 64-bit signed integer literal.
    Integer(i64),
    /// 64-bit IEEE-754 float literal stored as raw bits for Eq compatibility.
    RealBits(u64),
    /// UTF-8 string literal.
    Text(String),
    /// Binary blob literal.
    Blob(Vec<u8>),
    /// Evaluates current UTC UNIX/ISO timestamp string at INSERT time.
    CurrentTimestamp,
}

impl DefaultValue {
    /// Construct a DefaultValue from a runtime Value literal.
    pub fn from_value(val: &Value) -> Self {
        match val {
            Value::Null => Self::Null,
            Value::Integer(i) => Self::Integer(*i),
            Value::Real(f) => Self::RealBits(f.to_bits()),
            Value::Text(s) => Self::Text(s.clone()),
            Value::Blob(b) => Self::Blob(b.clone()),
            Value::Vector(v) => Self::Text(format!("{v:?}")),
        }
    }

    /// Materialize this DEFAULT specification into a concrete runtime Value.
    pub fn materialize(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Integer(i) => Value::Integer(*i),
            Self::RealBits(bits) => Value::Real(f64::from_bits(*bits)),
            Self::Text(s) => Value::Text(s.clone()),
            Self::Blob(b) => Value::Blob(b.clone()),
            Self::CurrentTimestamp => {
                let secs = web_time::SystemTime::now()
                    .duration_since(web_time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                Value::Text(secs.to_string())
            }
        }
    }
}

/// Configuration for an FTS5 full-text virtual table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FtsTableConfig {
    /// Columns indexed for BM25 full-text search.
    pub columns: Vec<String>,
    /// Tokenizer name (e.g. "unicode61" or "default").
    #[serde(default)]
    pub tokenizer: String,
}

/// Trigger execution timing relative to DML statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerTiming {
    /// Execute before the target row mutation is applied.
    Before,
    /// Execute after the target row mutation succeeds.
    After,
}

/// Trigger DML event kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerEvent {
    /// Fires on INSERT.
    Insert,
    /// Fires on UPDATE.
    Update,
    /// Fires on DELETE.
    Delete,
}

/// Persisted SQL trigger definition stored in the schema catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerDef {
    /// Trigger name.
    pub name: String,
    /// Target table name.
    pub table: String,
    /// Execution timing (BEFORE or AFTER).
    pub timing: TriggerTiming,
    /// DML event (INSERT, UPDATE, or DELETE).
    pub event: TriggerEvent,
    /// Optional WHEN boolean guard SQL string.
    #[serde(default)]
    pub when_sql: Option<String>,
    /// Trigger body SQL statements between BEGIN and END.
    pub body_sql: String,
}

/// Column definition within a table
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDef {
    /// Name of column
    pub name: String,
    /// Data type of column
    pub data_type: DataType,
    /// Whether column is the Primary Key
    pub primary_key: bool,
    /// Whether column requires non-null values
    pub not_null: bool,
    /// Whether column enforces uniqueness across non-NULL values
    #[serde(default)]
    pub unique: bool,
    /// Optional DEFAULT value applied when column is omitted on INSERT
    #[serde(default)]
    pub default_value: Option<DefaultValue>,
    /// Optional column-level CHECK SQL constraint expression
    #[serde(default)]
    pub check_expr: Option<String>,
    /// Optional inline FOREIGN KEY reference specification
    #[serde(default)]
    pub references: Option<ForeignKeyRef>,
}

impl ColumnDef {
    /// Create a new ColumnDef
    pub fn new(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            data_type,
            primary_key: false,
            not_null: false,
            unique: false,
            default_value: None,
            check_expr: None,
            references: None,
        }
    }

    /// Mark this column as a Primary Key
    pub fn primary_key(mut self) -> Self {
        self.primary_key = true;
        self
    }

    /// Mark this column as NOT NULL
    pub fn not_null(mut self) -> Self {
        self.not_null = true;
        self
    }

    /// Mark this column as UNIQUE
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Attach a DEFAULT value specification to this column
    pub fn with_default(mut self, default: DefaultValue) -> Self {
        self.default_value = Some(default);
        self
    }

    /// Attach a CHECK constraint SQL expression to this column
    pub fn with_check(mut self, expr_sql: impl Into<String>) -> Self {
        self.check_expr = Some(expr_sql.into());
        self
    }

    /// Attach a FOREIGN KEY reference to this column
    pub fn with_references(mut self, fk: ForeignKeyRef) -> Self {
        self.references = Some(fk);
        self
    }
}

/// Complete definition of a table stored in TapirusDB
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableDef {
    /// Name of table (case-insensitive)
    pub name: String,
    /// Root page ID of table's B+Tree
    pub root_page: PageId,
    /// Ordered list of column definitions
    pub columns: Vec<ColumnDef>,
    /// Auto-increment counter for primary key generation
    pub next_row_id: u64,
    /// Whether historical time-travel snapshots are tracked for this table
    #[serde(default)]
    pub system_versioning: bool,
    /// Table-level or composite FOREIGN KEY constraints
    #[serde(default)]
    pub foreign_keys: Vec<TableForeignKey>,
    /// Table-level CHECK constraints
    #[serde(default)]
    pub check_constraints: Vec<CheckConstraint>,
    /// Table-level multi-column UNIQUE constraints
    #[serde(default)]
    pub unique_constraints: Vec<Vec<String>>,
    /// Composite PRIMARY KEY column names (if more than one column)
    #[serde(default)]
    pub primary_key_columns: Vec<String>,
    /// Optional FTS5 virtual table configuration
    #[serde(default)]
    pub fts_config: Option<FtsTableConfig>,
}

impl TableDef {
    /// Create a new table definition
    pub fn new(name: String, root_page: PageId, columns: Vec<ColumnDef>) -> Self {
        Self {
            name,
            root_page,
            columns,
            next_row_id: 1,
            system_versioning: false,
            foreign_keys: Vec::new(),
            check_constraints: Vec::new(),
            unique_constraints: Vec::new(),
            primary_key_columns: Vec::new(),
            fts_config: None,
        }
    }

    /// Enable or disable system versioning for time-travel queries
    pub fn with_system_versioning(mut self, enabled: bool) -> Self {
        self.system_versioning = enabled;
        self
    }

    /// Find column index by name (case-insensitive)
    pub fn column_index(&self, col_name: &str) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(col_name))
    }

    /// Column names
    pub fn column_names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    /// Find primary key column index if defined
    pub fn primary_key_index(&self) -> Option<usize> {
        self.columns.iter().position(|c| c.primary_key)
    }

    /// Find first vector column if any: returns (column_index, dimensions)
    pub fn vector_column(&self) -> Option<(usize, usize)> {
        for (idx, col) in self.columns.iter().enumerate() {
            if let DataType::Vector(dims) = col.data_type {
                return Some((idx, dims));
            }
        }
        None
    }
}

/// View definition stored in the schema catalog
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewDef {
    /// Name of view
    pub name: String,
    /// SQL query string defining the view
    pub query_sql: String,
}

impl ViewDef {
    /// Create a new ViewDef
    pub fn new(name: impl Into<String>, query_sql: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            query_sql: query_sql.into(),
        }
    }
}

/// Secondary index definition stored in the schema catalog
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDef {
    /// Name of index
    pub name: String,
    /// Target table name
    pub table: String,
    /// Primary/leading column name (kept for single-column compatibility)
    pub column: String,
    /// Ordered columns covered by this index (supports composite multi-column indexes)
    #[serde(default)]
    pub columns: Vec<String>,
    /// Root page ID of the index B+Tree
    pub root_page: PageId,
    /// Whether index enforces uniqueness
    pub unique: bool,
}

impl IndexDef {
    /// Create a new single-column IndexDef
    pub fn new(
        name: impl Into<String>,
        table: impl Into<String>,
        column: impl Into<String>,
        root_page: PageId,
    ) -> Self {
        let col = column.into();
        Self {
            name: name.into(),
            table: table.into(),
            column: col.clone(),
            columns: vec![col],
            root_page,
            unique: false,
        }
    }

    /// Create a new multi-column composite IndexDef
    pub fn composite(
        name: impl Into<String>,
        table: impl Into<String>,
        columns: Vec<String>,
        root_page: PageId,
        unique: bool,
    ) -> Self {
        let leading = columns.first().cloned().unwrap_or_default();
        Self {
            name: name.into(),
            table: table.into(),
            column: leading,
            columns,
            root_page,
            unique,
        }
    }

    /// Ordered list of indexed column names (handles legacy single-column records)
    pub fn target_columns(&self) -> Vec<&str> {
        if self.columns.is_empty() {
            vec![self.column.as_str()]
        } else {
            self.columns.iter().map(|s| s.as_str()).collect()
        }
    }
}

/// Master schema catalog holding all table, view, index, and trigger definitions
#[derive(Debug, Default, Clone)]
pub struct Catalog {
    tables: HashMap<String, TableDef>,
    views: HashMap<String, ViewDef>,
    indexes: HashMap<String, IndexDef>,
    triggers: HashMap<String, TriggerDef>,
    stats: HashMap<String, crate::sql::planner::TableStats>,
    next_table_id: u64,
}

impl Catalog {
    /// Initialize an empty Catalog
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
            views: HashMap::new(),
            indexes: HashMap::new(),
            triggers: HashMap::new(),
            stats: HashMap::new(),
            next_table_id: 1,
        }
    }

    /// Load catalog from Page 1 of the database file
    pub fn load(pager: &mut Pager) -> Result<Self> {
        let mut catalog = Self::new();
        let page1_buf = pager.read_page(1)?;
        let header = engine::read_page_header(&page1_buf, 1);

        // If page 1 header hasn't been initialized as a B+Tree leaf, initialize it unless read-only
        if header.is_err() {
            if pager.is_read_only() {
                return Ok(catalog);
            }
            let mut page1_init = page1_buf.clone();
            engine::init_leaf_page(&mut page1_init, 1);
            pager.write_page(1, &page1_init)?;
            return Ok(catalog);
        }

        let btree = BTreeStorage::new();
        let cells = btree.scan(pager, 1)?;

        for cell in cells {
            if let Ok(table_def) = serde_json::from_slice::<TableDef>(&cell.payload) {
                if cell.row_id >= catalog.next_table_id {
                    catalog.next_table_id = cell.row_id + 1;
                }
                catalog
                    .tables
                    .insert(table_def.name.to_lowercase(), table_def);
            } else if let Ok(trigger_def) = serde_json::from_slice::<TriggerDef>(&cell.payload) {
                if cell.row_id >= catalog.next_table_id {
                    catalog.next_table_id = cell.row_id + 1;
                }
                catalog
                    .triggers
                    .insert(trigger_def.name.to_lowercase(), trigger_def);
            } else if let Ok(index_def) = serde_json::from_slice::<IndexDef>(&cell.payload) {
                if cell.row_id >= catalog.next_table_id {
                    catalog.next_table_id = cell.row_id + 1;
                }
                catalog
                    .indexes
                    .insert(index_def.name.to_lowercase(), index_def);
            } else if let Ok(view_def) = serde_json::from_slice::<ViewDef>(&cell.payload) {
                if cell.row_id >= catalog.next_table_id {
                    catalog.next_table_id = cell.row_id + 1;
                }
                catalog.views.insert(view_def.name.to_lowercase(), view_def);
            }
        }

        Ok(catalog)
    }

    /// Check if a table exists
    pub fn table_exists(&self, name: &str) -> bool {
        self.tables.contains_key(&name.to_lowercase())
    }

    /// Get all table definitions
    pub fn tables(&self) -> Vec<TableDef> {
        self.tables.values().cloned().collect()
    }

    /// Get table definition
    pub fn get_table(&self, name: &str) -> Option<&TableDef> {
        if let Some(t) = self.tables.get(name) {
            return Some(t);
        }
        self.tables.get(&name.to_lowercase())
    }

    /// Get mutable table definition
    pub fn get_table_mut(&mut self, name: &str) -> Option<&mut TableDef> {
        if self.tables.contains_key(name) {
            return self.tables.get_mut(name);
        }
        self.tables.get_mut(&name.to_lowercase())
    }

    /// Register a new table and persist its definition to Page 1
    pub fn create_table(&mut self, pager: &mut Pager, table_def: TableDef) -> Result<()> {
        let name_lower = table_def.name.to_lowercase();
        if self.tables.contains_key(&name_lower) {
            return Err(Error::TableExists(table_def.name));
        }

        let table_id = self.next_table_id;
        self.next_table_id += 1;

        // Serialize table metadata
        let payload = serde_json::to_vec(&table_def)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize table def: {e}")))?;

        // Insert into Page 1 Master Schema B+Tree
        let mut btree = BTreeStorage::new();
        btree.insert(pager, 1, table_id, &payload)?;

        // Update database header schema_cookie
        pager.header_mut().schema_cookie += 1;
        let header_bytes = pager.header().to_bytes();
        let mut page1_buf = pager.read_page(1)?;
        page1_buf[..crate::pager::DATABASE_HEADER_SIZE].copy_from_slice(&header_bytes);
        pager.write_page(1, &page1_buf)?;

        self.tables.insert(name_lower, table_def);
        Ok(())
    }

    /// Add a column to an existing table (ALTER TABLE ADD COLUMN) and persist update to Page 1
    pub fn add_column(
        &mut self,
        pager: &mut Pager,
        table_name: &str,
        col_def: ColumnDef,
    ) -> Result<()> {
        let name_lower = table_name.to_lowercase();
        let table_def = self
            .tables
            .get_mut(&name_lower)
            .ok_or_else(|| Error::TableNotFound(table_name.to_string()))?;

        if table_def.column_index(&col_def.name).is_some() {
            return Err(Error::SqlSyntax(format!(
                "Column '{}' already exists in table '{}'",
                col_def.name, table_name
            )));
        }

        table_def.columns.push(col_def);
        let updated_def = table_def.clone();

        // Update Page 1 B+Tree master schema entry
        let mut btree = BTreeStorage::new();
        let cells = btree.scan(pager, 1)?;
        for cell in cells {
            if let Ok(td) = serde_json::from_slice::<TableDef>(&cell.payload)
                && td.name.eq_ignore_ascii_case(table_name)
            {
                let payload = serde_json::to_vec(&updated_def).map_err(|e| {
                    Error::Corrupted(format!("Failed to serialize updated table def: {e}"))
                })?;
                btree.delete(pager, 1, cell.row_id)?;
                btree.insert(pager, 1, cell.row_id, &payload)?;
                break;
            }
        }

        // Increment schema cookie
        pager.header_mut().schema_cookie += 1;
        let header_bytes = pager.header().to_bytes();
        let mut page1_buf = pager.read_page(1)?;
        page1_buf[..crate::pager::DATABASE_HEADER_SIZE].copy_from_slice(&header_bytes);
        pager.write_page(1, &page1_buf)?;

        Ok(())
    }

    /// Rename an existing table (ALTER TABLE RENAME TO) and persist update to Page 1
    pub fn rename_table(
        &mut self,
        pager: &mut Pager,
        old_name: &str,
        new_name: &str,
    ) -> Result<()> {
        let old_lower = old_name.to_lowercase();
        let new_lower = new_name.to_lowercase();

        if !self.tables.contains_key(&old_lower) {
            return Err(Error::TableNotFound(old_name.to_string()));
        }
        if self.tables.contains_key(&new_lower) || self.views.contains_key(&new_lower) {
            return Err(Error::TableExists(format!(
                "Table or view '{new_name}' already exists"
            )));
        }

        let mut table_def = self.tables.remove(&old_lower).unwrap();
        table_def.name = new_name.to_string();
        let updated_def = table_def.clone();

        // Update Page 1 B+Tree master schema entry
        let mut btree = BTreeStorage::new();
        let cells = btree.scan(pager, 1)?;
        for cell in cells {
            if let Ok(td) = serde_json::from_slice::<TableDef>(&cell.payload)
                && td.name.eq_ignore_ascii_case(old_name)
            {
                let payload = serde_json::to_vec(&updated_def).map_err(|e| {
                    Error::Corrupted(format!("Failed to serialize updated table def: {e}"))
                })?;
                btree.delete(pager, 1, cell.row_id)?;
                btree.insert(pager, 1, cell.row_id, &payload)?;
                break;
            }
        }

        // Increment schema cookie
        pager.header_mut().schema_cookie += 1;
        let header_bytes = pager.header().to_bytes();
        let mut page1_buf = pager.read_page(1)?;
        page1_buf[..crate::pager::DATABASE_HEADER_SIZE].copy_from_slice(&header_bytes);
        pager.write_page(1, &page1_buf)?;

        self.tables.insert(new_lower, table_def);
        Ok(())
    }

    /// Rename a column in an existing table (ALTER TABLE RENAME COLUMN) and persist update to Page 1
    pub fn rename_column(
        &mut self,
        pager: &mut Pager,
        table_name: &str,
        old_col: &str,
        new_col: &str,
    ) -> Result<()> {
        let name_lower = table_name.to_lowercase();
        let table_def = self
            .tables
            .get_mut(&name_lower)
            .ok_or_else(|| Error::TableNotFound(table_name.to_string()))?;

        if table_def.column_index(new_col).is_some() {
            return Err(Error::SqlSyntax(format!(
                "Column '{new_col}' already exists in table '{table_name}'"
            )));
        }

        let col_idx = table_def
            .column_index(old_col)
            .ok_or_else(|| Error::ColumnNotFound(old_col.to_string()))?;

        table_def.columns[col_idx].name = new_col.to_string();
        let updated_def = table_def.clone();

        // Update Page 1 B+Tree master schema entry
        let mut btree = BTreeStorage::new();
        let cells = btree.scan(pager, 1)?;
        for cell in cells {
            if let Ok(td) = serde_json::from_slice::<TableDef>(&cell.payload)
                && td.name.eq_ignore_ascii_case(table_name)
            {
                let payload = serde_json::to_vec(&updated_def).map_err(|e| {
                    Error::Corrupted(format!("Failed to serialize updated table def: {e}"))
                })?;
                btree.delete(pager, 1, cell.row_id)?;
                btree.insert(pager, 1, cell.row_id, &payload)?;
                break;
            }
        }

        // Increment schema cookie
        pager.header_mut().schema_cookie += 1;
        let header_bytes = pager.header().to_bytes();
        let mut page1_buf = pager.read_page(1)?;
        page1_buf[..crate::pager::DATABASE_HEADER_SIZE].copy_from_slice(&header_bytes);
        pager.write_page(1, &page1_buf)?;

        Ok(())
    }

    /// Drop a column from an existing table (ALTER TABLE DROP COLUMN) and persist update to Page 1
    pub fn drop_column(
        &mut self,
        pager: &mut Pager,
        table_name: &str,
        col_name: &str,
    ) -> Result<()> {
        let name_lower = table_name.to_lowercase();
        let table_def = self
            .tables
            .get_mut(&name_lower)
            .ok_or_else(|| Error::TableNotFound(table_name.to_string()))?;

        let col_idx = table_def
            .column_index(col_name)
            .ok_or_else(|| Error::ColumnNotFound(col_name.to_string()))?;

        if table_def.columns[col_idx].primary_key {
            return Err(Error::ConstraintViolation(format!(
                "Cannot drop PRIMARY KEY column '{col_name}' from table '{table_name}'"
            )));
        }

        if table_def.columns.len() <= 1 {
            return Err(Error::ConstraintViolation(format!(
                "Cannot drop column '{col_name}': table must have at least one column"
            )));
        }

        let old_def = table_def.clone();
        table_def.columns.remove(col_idx);
        let updated_def = table_def.clone();
        let root_page = table_def.root_page;

        // Rewrite rows in table to physically remove the dropped column
        let mut btree = BTreeStorage::new();
        let col_names = old_def.column_names();
        let cells = btree.scan(pager, root_page)?;
        for cell in cells {
            if let Ok(row) = crate::sql::codec::decode_row(&cell.payload, &col_names)
                && col_idx < row.values().len()
            {
                let mut vals = row.values().to_vec();
                vals.remove(col_idx);
                let new_payload = crate::sql::codec::encode_row(&vals);
                btree.insert(pager, root_page, cell.row_id, &new_payload)?;
            }
        }

        // Update Page 1 B+Tree master schema entry
        let cells = btree.scan(pager, 1)?;
        for cell in cells {
            if let Ok(td) = serde_json::from_slice::<TableDef>(&cell.payload)
                && td.name.eq_ignore_ascii_case(table_name)
            {
                let payload = serde_json::to_vec(&updated_def).map_err(|e| {
                    Error::Corrupted(format!("Failed to serialize updated table def: {e}"))
                })?;
                btree.delete(pager, 1, cell.row_id)?;
                btree.insert(pager, 1, cell.row_id, &payload)?;
                break;
            }
        }

        // Increment schema cookie
        pager.header_mut().schema_cookie += 1;
        let header_bytes = pager.header().to_bytes();
        let mut page1_buf = pager.read_page(1)?;
        page1_buf[..crate::pager::DATABASE_HEADER_SIZE].copy_from_slice(&header_bytes);
        pager.write_page(1, &page1_buf)?;

        Ok(())
    }

    /// Drop a table from the catalog and delete its entry on Page 1
    pub fn drop_table(&mut self, pager: &mut Pager, table_name: &str) -> Result<bool> {
        let name_lower = table_name.to_lowercase();
        if let Some(table_def) = self.tables.remove(&name_lower) {
            // Find and delete the cell from Page 1 Master Schema B+Tree
            let mut btree = BTreeStorage::new();
            let cells = btree.scan(pager, 1)?;
            for cell in cells {
                if let Ok(td) = serde_json::from_slice::<TableDef>(&cell.payload)
                    && td.name.eq_ignore_ascii_case(&table_def.name)
                {
                    let _ = btree.delete(pager, 1, cell.row_id);
                    break;
                }
            }

            // Update database header schema_cookie
            pager.header_mut().schema_cookie += 1;
            let header_bytes = pager.header().to_bytes();
            let mut page1_buf = pager.read_page(1)?;
            page1_buf[..crate::pager::DATABASE_HEADER_SIZE].copy_from_slice(&header_bytes);
            pager.write_page(1, &page1_buf)?;

            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Register a new view and persist its definition to Page 1
    pub fn create_view(&mut self, pager: &mut Pager, view_def: ViewDef) -> Result<()> {
        let name_lower = view_def.name.to_lowercase();
        if self.tables.contains_key(&name_lower) {
            return Err(Error::TableExists(format!(
                "Table with name '{}' already exists",
                view_def.name
            )));
        }
        if self.views.contains_key(&name_lower) {
            return Err(Error::TableExists(format!(
                "View '{}' already exists",
                view_def.name
            )));
        }

        let entry_id = self.next_table_id;
        self.next_table_id += 1;

        let payload = serde_json::to_vec(&view_def)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize view def: {e}")))?;

        let mut btree = BTreeStorage::new();
        btree.insert(pager, 1, entry_id, &payload)?;

        // Update database header schema_cookie
        pager.header_mut().schema_cookie += 1;
        let header_bytes = pager.header().to_bytes();
        let mut page1_buf = pager.read_page(1)?;
        page1_buf[..crate::pager::DATABASE_HEADER_SIZE].copy_from_slice(&header_bytes);
        pager.write_page(1, &page1_buf)?;

        self.views.insert(name_lower, view_def);
        Ok(())
    }

    /// Drop a view from the catalog and delete its entry on Page 1
    pub fn drop_view(&mut self, pager: &mut Pager, view_name: &str) -> Result<bool> {
        let name_lower = view_name.to_lowercase();
        if self.views.remove(&name_lower).is_some() {
            let mut btree = BTreeStorage::new();
            let cells = btree.scan(pager, 1)?;
            for cell in cells {
                if let Ok(vd) = serde_json::from_slice::<ViewDef>(&cell.payload)
                    && vd.name.eq_ignore_ascii_case(view_name)
                {
                    let _ = btree.delete(pager, 1, cell.row_id);
                    break;
                }
            }

            // Update database header schema_cookie
            pager.header_mut().schema_cookie += 1;
            let header_bytes = pager.header().to_bytes();
            let mut page1_buf = pager.read_page(1)?;
            page1_buf[..crate::pager::DATABASE_HEADER_SIZE].copy_from_slice(&header_bytes);
            pager.write_page(1, &page1_buf)?;

            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Check if a view exists
    pub fn view_exists(&self, name: &str) -> bool {
        self.views.contains_key(&name.to_lowercase())
    }

    /// Get view definition
    pub fn get_view(&self, name: &str) -> Option<&ViewDef> {
        self.views.get(&name.to_lowercase())
    }

    /// Get all view definitions
    pub fn views(&self) -> Vec<ViewDef> {
        self.views.values().cloned().collect()
    }

    /// Check if an index exists
    pub fn index_exists(&self, name: &str) -> bool {
        self.indexes.contains_key(&name.to_lowercase())
    }

    /// Get index definition
    pub fn get_index(&self, name: &str) -> Option<&IndexDef> {
        self.indexes.get(&name.to_lowercase())
    }

    /// Find an index on a specific table and column
    pub fn find_index_for_column(&self, table: &str, column: &str) -> Option<&IndexDef> {
        self.indexes.values().find(|idx| {
            idx.table.eq_ignore_ascii_case(table) && idx.column.eq_ignore_ascii_case(column)
        })
    }

    /// Check whether a table has any secondary indexes without allocating
    pub fn has_table_indexes(&self, table: &str) -> bool {
        self.indexes
            .values()
            .any(|idx| idx.table.eq_ignore_ascii_case(table))
    }

    /// Get all index definitions
    pub fn indexes(&self) -> Vec<IndexDef> {
        self.indexes.values().cloned().collect()
    }

    /// Retrieve statistical profile for a table if analyzed
    pub fn get_table_stats(&self, table: &str) -> Option<&crate::sql::planner::TableStats> {
        self.stats.get(&table.to_lowercase())
    }

    /// Store statistical profile for a table
    pub fn set_table_stats(&mut self, table: &str, stats: crate::sql::planner::TableStats) {
        self.stats.insert(table.to_lowercase(), stats);
    }

    /// Register a new secondary index and persist its definition to Page 1
    pub fn create_index(&mut self, pager: &mut Pager, index_def: IndexDef) -> Result<()> {
        let name_lower = index_def.name.to_lowercase();
        if self.indexes.contains_key(&name_lower) {
            return Err(Error::TableExists(format!(
                "Index '{}' already exists",
                index_def.name
            )));
        }

        let entry_id = self.next_table_id;
        self.next_table_id += 1;

        let payload = serde_json::to_vec(&index_def)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize index def: {e}")))?;

        let mut btree = BTreeStorage::new();
        btree.insert(pager, 1, entry_id, &payload)?;

        // Update database header schema_cookie
        pager.header_mut().schema_cookie += 1;
        let header_bytes = pager.header().to_bytes();
        let mut page1_buf = pager.read_page(1)?;
        page1_buf[..crate::pager::DATABASE_HEADER_SIZE].copy_from_slice(&header_bytes);
        pager.write_page(1, &page1_buf)?;

        self.indexes.insert(name_lower, index_def);
        Ok(())
    }

    /// Drop an index from the catalog and delete its entry on Page 1
    pub fn drop_index(&mut self, pager: &mut Pager, index_name: &str) -> Result<bool> {
        let name_lower = index_name.to_lowercase();
        if self.indexes.remove(&name_lower).is_some() {
            let mut btree = BTreeStorage::new();
            let cells = btree.scan(pager, 1)?;
            for cell in cells {
                if let Ok(id) = serde_json::from_slice::<IndexDef>(&cell.payload)
                    && id.name.eq_ignore_ascii_case(index_name)
                {
                    let _ = btree.delete(pager, 1, cell.row_id);
                    break;
                }
            }

            // Update database header schema_cookie
            pager.header_mut().schema_cookie += 1;
            let header_bytes = pager.header().to_bytes();
            let mut page1_buf = pager.read_page(1)?;
            page1_buf[..crate::pager::DATABASE_HEADER_SIZE].copy_from_slice(&header_bytes);
            pager.write_page(1, &page1_buf)?;

            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Find an index on a specific table matching an exact ordered column list
    pub fn find_index_for_columns(&self, table: &str, columns: &[&str]) -> Option<&IndexDef> {
        self.indexes.values().find(|idx| {
            if !idx.table.eq_ignore_ascii_case(table) {
                return false;
            }
            let target = idx.target_columns();
            target.len() == columns.len()
                && target
                    .iter()
                    .zip(columns.iter())
                    .all(|(a, b)| a.eq_ignore_ascii_case(b))
        })
    }

    /// Check if a trigger exists
    pub fn trigger_exists(&self, name: &str) -> bool {
        self.triggers.contains_key(&name.to_lowercase())
    }

    /// Return all triggers attached to a table for a specific timing and DML event
    pub fn triggers_for(
        &self,
        table: &str,
        timing: TriggerTiming,
        event: TriggerEvent,
    ) -> Vec<TriggerDef> {
        let mut matched: Vec<TriggerDef> = self
            .triggers
            .values()
            .filter(|t| {
                t.table.eq_ignore_ascii_case(table) && t.timing == timing && t.event == event
            })
            .cloned()
            .collect();
        matched.sort_by(|a, b| a.name.cmp(&b.name));
        matched
    }

    /// Return all registered triggers
    pub fn triggers(&self) -> Vec<TriggerDef> {
        self.triggers.values().cloned().collect()
    }

    /// Register a new trigger and persist it to Page 1
    pub fn create_trigger(&mut self, pager: &mut Pager, trigger: TriggerDef) -> Result<()> {
        let name_lower = trigger.name.to_lowercase();
        if self.triggers.contains_key(&name_lower) {
            return Err(Error::TableExists(format!(
                "Trigger '{}' already exists",
                trigger.name
            )));
        }

        let entry_id = self.next_table_id;
        self.next_table_id += 1;

        let payload = serde_json::to_vec(&trigger)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize trigger def: {e}")))?;

        let mut btree = BTreeStorage::new();
        btree.insert(pager, 1, entry_id, &payload)?;

        pager.header_mut().schema_cookie += 1;
        let header_bytes = pager.header().to_bytes();
        let mut page1_buf = pager.read_page(1)?;
        page1_buf[..crate::pager::DATABASE_HEADER_SIZE].copy_from_slice(&header_bytes);
        pager.write_page(1, &page1_buf)?;

        self.triggers.insert(name_lower, trigger);
        Ok(())
    }

    /// Drop a trigger from the catalog and delete its entry on Page 1
    pub fn drop_trigger(&mut self, pager: &mut Pager, trigger_name: &str) -> Result<bool> {
        let name_lower = trigger_name.to_lowercase();
        if self.triggers.remove(&name_lower).is_some() {
            let mut btree = BTreeStorage::new();
            let cells = btree.scan(pager, 1)?;
            for cell in cells {
                if let Ok(tr) = serde_json::from_slice::<TriggerDef>(&cell.payload)
                    && tr.name.eq_ignore_ascii_case(trigger_name)
                {
                    let _ = btree.delete(pager, 1, cell.row_id);
                    break;
                }
            }

            pager.header_mut().schema_cookie += 1;
            let header_bytes = pager.header().to_bytes();
            let mut page1_buf = pager.read_page(1)?;
            page1_buf[..crate::pager::DATABASE_HEADER_SIZE].copy_from_slice(&header_bytes);
            pager.write_page(1, &page1_buf)?;

            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_create_and_load() {
        let mut pager = Pager::open_in_memory(4096, 128).expect("Pager open");
        let mut catalog = Catalog::load(&mut pager).expect("Load catalog");

        let cols = vec![
            ColumnDef::new("id", DataType::Integer)
                .primary_key()
                .not_null(),
            ColumnDef::new("title", DataType::Text),
            ColumnDef::new("embedding", DataType::Vector(128)),
        ];

        let table_root = pager.allocate_page().expect("Alloc table root");
        let mut root_buf = vec![0u8; 4096];
        engine::init_leaf_page(&mut root_buf, table_root);
        pager
            .write_page(table_root, &root_buf)
            .expect("Init table root");

        let table_def = TableDef::new("documents".to_string(), table_root, cols);
        catalog
            .create_table(&mut pager, table_def.clone())
            .expect("Create table");

        assert!(catalog.table_exists("DOCUMENTS"));
        let retrieved = catalog.get_table("documents").unwrap();
        assert_eq!(retrieved.root_page, table_root);
        assert_eq!(retrieved.columns.len(), 3);
        assert_eq!(retrieved.vector_column(), Some((2, 128)));

        // Load catalog anew from Page 1 to verify persistence
        let reloaded = Catalog::load(&mut pager).expect("Reload catalog");
        assert!(reloaded.table_exists("documents"));
        let t = reloaded.get_table("documents").unwrap();
        assert_eq!(t.name, "documents");
        assert_eq!(t.root_page, table_root);
    }
}
