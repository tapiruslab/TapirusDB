//! Restricted, first-party entry points for individual TapirusDB data models.
//!
//! Every edition uses the same `.tapir` storage engine. These types restrict the
//! public API of an edition; they do not provide a security boundary against a
//! caller that deliberately opens the same file with the full `Connection` API.

#[cfg(any(
    feature = "edition-lite",
    feature = "edition-document",
    feature = "edition-graph",
    feature = "edition-vector"
))]
use crate::{Connection, Result};
#[cfg(any(
    feature = "edition-lite",
    feature = "edition-document",
    feature = "edition-graph",
    feature = "edition-vector"
))]
use std::path::Path;

/// SQL-only entry point. The complete engine remains available through `Connection`.
#[cfg(feature = "edition-lite")]
pub struct LiteDatabase {
    connection: Connection,
}

#[cfg(feature = "edition-lite")]
impl LiteDatabase {
    /// Open a SQL-only view of an existing or new `.tapir` file.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Ok(Self {
            connection: Connection::open(path)?,
        })
    }

    /// Open a temporary SQL-only database.
    pub fn open_in_memory() -> Result<Self> {
        Ok(Self {
            connection: Connection::open_in_memory()?,
        })
    }

    /// Execute one relational SQL statement.
    pub fn execute(&self, sql: &str) -> Result<usize> {
        check_lite_sql(sql, &[])?;
        self.connection.execute(sql)
    }

    /// Query one relational SQL statement.
    pub fn query(&self, sql: &str) -> Result<Vec<crate::Row>> {
        check_lite_sql(sql, &[])?;
        self.connection.query(sql)
    }

    /// Execute relational SQL with positional parameters.
    pub fn execute_with_params(&self, sql: &str, params: &[crate::Value]) -> Result<usize> {
        check_lite_sql(sql, params)?;
        self.connection.execute_with_params(sql, params)
    }

    /// Query relational SQL with positional parameters.
    pub fn query_with_params(&self, sql: &str, params: &[crate::Value]) -> Result<Vec<crate::Row>> {
        check_lite_sql(sql, params)?;
        self.connection.query_with_params(sql, params)
    }

    /// Commit/checkpoint and close the connection.
    pub fn close(&self) -> Result<()> {
        self.connection.close()
    }
}

#[cfg(feature = "edition-lite")]
fn check_lite_sql(sql: &str, params: &[crate::Value]) -> Result<()> {
    use crate::sql::lexer::{Token, tokenize};
    use crate::sql::parser::{bind_parameters, parse_tokens};

    let tokens = tokenize(sql)?;
    let tokens = bind_parameters(&tokens, params)?;
    let denied = tokens.iter().enumerate().any(|(index, token)| match token {
        Token::Vector | Token::VectorLit(_) | Token::Graph | Token::Match => true,
        Token::Ident(name) => {
            let upper = name.to_ascii_uppercase();
            let model_function = (upper.starts_with("TAP_")
                || matches!(upper.as_str(), "VECTOR_COSINE" | "VECTOR_L2"))
                && tokens.get(index + 1) == Some(&Token::OpenParen);
            name.starts_with("__") || model_function
        }
        _ => false,
    });
    if denied {
        return Err(crate::Error::SqlSyntax(
            "This statement requires a non-Lite TapirusDB edition".into(),
        ));
    }
    let statement = parse_tokens(&tokens)?;
    if matches!(
        statement,
        crate::Statement::VectorSearch { .. }
            | crate::Statement::GraphInsertNode { .. }
            | crate::Statement::GraphInsertEdge { .. }
            | crate::Statement::GraphTraverse { .. }
            | crate::Statement::GraphShortestPath { .. }
            | crate::Statement::GraphMatch { .. }
            | crate::Statement::GraphAlgorithm { .. }
    ) {
        return Err(crate::Error::SqlSyntax(
            "This statement requires a non-Lite TapirusDB edition".into(),
        ));
    }
    Ok(())
}

/// JSON document-only entry point. SQL is used internally for durable storage.
#[cfg(feature = "edition-document")]
pub struct DocumentDatabase {
    connection: Connection,
}

#[cfg(feature = "edition-document")]
impl DocumentDatabase {
    /// Open a document-only view of an existing or new `.tapir` file.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Ok(Self {
            connection: Connection::open(path)?,
        })
    }

    /// Open a temporary document-only database.
    pub fn open_in_memory() -> Result<Self> {
        Ok(Self {
            connection: Connection::open_in_memory()?,
        })
    }

    /// Open or create a JSON collection.
    pub fn collection(&self, name: &str) -> Result<crate::Collection<'_>> {
        self.connection.collection(name)
    }

    /// Commit/checkpoint and close the connection.
    pub fn close(&self) -> Result<()> {
        self.connection.close()
    }
}

/// Graph-only entry point; graph records use the same native storage file.
#[cfg(feature = "edition-graph")]
pub struct GraphDatabase {
    connection: Connection,
}

#[cfg(feature = "edition-graph")]
impl GraphDatabase {
    /// Open a graph-only view of an existing or new `.tapir` file.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Ok(Self {
            connection: Connection::open(path)?,
        })
    }

    /// Open a temporary graph-only database.
    pub fn open_in_memory() -> Result<Self> {
        Ok(Self {
            connection: Connection::open_in_memory()?,
        })
    }

    /// Store a graph node.
    pub fn add_node(&self, id: u64, label: &str, properties: &str) -> Result<()> {
        self.connection.graph_add_node(id, label, properties)
    }

    /// Store a directed graph edge and return its ID.
    pub fn add_edge(
        &self,
        from_id: u64,
        to_id: u64,
        label: &str,
        weight: f32,
        properties: &str,
    ) -> Result<u64> {
        self.connection
            .graph_add_edge(from_id, to_id, label, weight, properties)
    }

    /// Find adjacent nodes and edges.
    pub fn neighbors(
        &self,
        id: u64,
        direction: crate::Direction,
        label: Option<&str>,
    ) -> Vec<(crate::Node, crate::Edge)> {
        self.connection.graph_neighbors(id, direction, label)
    }

    /// Commit/checkpoint and close the connection.
    pub fn close(&self) -> Result<()> {
        self.connection.close()
    }
}

/// Vector-only entry point with durable vector collections.
#[cfg(feature = "edition-vector")]
pub struct VectorDatabase {
    connection: Connection,
}

#[cfg(feature = "edition-vector")]
impl VectorDatabase {
    /// Open a vector-only view of an existing or new `.tapir` file.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Ok(Self {
            connection: Connection::open(path)?,
        })
    }

    /// Open a temporary vector-only database.
    pub fn open_in_memory() -> Result<Self> {
        Ok(Self {
            connection: Connection::open_in_memory()?,
        })
    }

    /// Open or create a durable collection with a fixed vector dimension.
    pub fn collection(&self, name: &str, dimension: usize) -> Result<VectorCollection<'_>> {
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            || dimension == 0
        {
            return Err(crate::Error::ConstraintViolation(
                "Vector collection requires a simple name and positive dimension".into(),
            ));
        }
        let table = format!("__vec_{name}");
        self.connection.execute(&format!(
            "CREATE TABLE IF NOT EXISTS {table} (id INTEGER PRIMARY KEY, embedding VECTOR({dimension}), metadata TEXT)"
        ))?;
        let definition = self
            .connection
            .table(&table)
            .ok_or_else(|| crate::Error::TableNotFound(table.clone()))?;
        if !definition.columns.iter().any(|column| {
            column.name.eq_ignore_ascii_case("embedding")
                && column.data_type == crate::sql::catalog::DataType::Vector(dimension)
        }) {
            return Err(crate::Error::ConstraintViolation(format!(
                "Collection {name} has a different vector dimension or schema"
            )));
        }
        Ok(VectorCollection {
            connection: &self.connection,
            table,
            dimension,
        })
    }

    /// Commit/checkpoint and close the connection.
    pub fn close(&self) -> Result<()> {
        self.connection.close()
    }
}

/// A fixed-dimension vector collection in a native `.tapir` file.
#[cfg(feature = "edition-vector")]
pub struct VectorCollection<'a> {
    connection: &'a Connection,
    table: String,
    dimension: usize,
}

#[cfg(feature = "edition-vector")]
impl VectorCollection<'_> {
    /// Insert one vector with caller-chosen ID and JSON metadata.
    pub fn insert(&self, id: u64, embedding: &[f32], metadata: &serde_json::Value) -> Result<()> {
        self.validate_embedding(embedding)?;
        let id = i64::try_from(id).map_err(|_| {
            crate::Error::ConstraintViolation("Vector ID exceeds signed 64-bit range".into())
        })?;
        self.connection.execute_with_params(
            &format!(
                "INSERT INTO {} (id, embedding, metadata) VALUES (?, ?, ?)",
                self.table
            ),
            &[
                crate::Value::Integer(id),
                crate::Value::Vector(embedding.to_vec()),
                crate::Value::Text(metadata.to_string()),
            ],
        )?;
        Ok(())
    }

    /// Search nearest vectors by cosine distance; rows include `id` and `metadata`.
    pub fn search(&self, embedding: &[f32], limit: usize) -> Result<Vec<crate::Row>> {
        self.validate_embedding(embedding)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let vector = embedding
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        self.connection.query(&format!(
            "SELECT id, metadata FROM {} VECTOR NEAR embedding = [{vector}] TOP {limit}",
            self.table
        ))
    }

    fn validate_embedding(&self, embedding: &[f32]) -> Result<()> {
        if embedding.len() != self.dimension {
            return Err(crate::Error::DimensionMismatch(
                self.dimension,
                embedding.len(),
            ));
        }
        if !embedding.iter().all(|value| value.is_finite()) {
            return Err(crate::Error::ConstraintViolation(
                "Vector components must be finite".into(),
            ));
        }
        Ok(())
    }
}
