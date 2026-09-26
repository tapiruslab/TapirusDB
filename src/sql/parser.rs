//! SQL and Graph statement parser.
//!
//! Converts token streams into an executable `Statement` AST.

use crate::error::{Error, Result};
use crate::sql::catalog::{ColumnDef, DataType};
use crate::sql::lexer::{tokenize, Token};
use crate::traits::Value;

/// Relational JOIN types supported in TapirusDB
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    /// Standard INNER JOIN (only matching rows)
    Inner,
    /// LEFT [OUTER] JOIN (all left rows + matching right rows)
    Left,
    /// RIGHT [OUTER] JOIN (all right rows + matching left rows)
    Right,
    /// FULL [OUTER] JOIN (all left and right rows, with NULLs for unmatched sides)
    Full,
}

/// Specification for table JOIN
#[derive(Debug, Clone, PartialEq)]
pub struct JoinClause {
    /// Right/joined table name
    pub table: String,
    /// Left column expression (e.g. "users.id" or "id")
    pub left_col: String,
    /// Right column expression (e.g. "orders.user_id" or "user_id")
    pub right_col: String,
    /// Whether this is a LEFT JOIN (for backward compatibility)
    pub is_left: bool,
    /// The relational join type (Inner, Left, Right, Full)
    pub join_type: JoinType,
}

impl JoinClause {
    /// Helper to create a new JoinClause
    pub fn new(table: impl Into<String>, left_col: impl Into<String>, right_col: impl Into<String>, join_type: JoinType) -> Self {
        Self {
            table: table.into(),
            left_col: left_col.into(),
            right_col: right_col.into(),
            is_left: join_type == JoinType::Left,
            join_type,
        }
    }
}

/// Specification for ORDER BY sorting
#[derive(Debug, Clone, PartialEq)]
pub struct OrderByClause {
    /// Column name to sort by
    pub column: String,
    /// True for ASC, False for DESC
    pub is_ascending: bool,
}

/// Specification for a Common Table Expression (CTE)
#[derive(Debug, Clone, PartialEq)]
pub struct CteClause {
    /// Alias name of the CTE table
    pub name: String,
    /// Optional explicit projected column names
    pub columns: Option<Vec<String>>,
    /// Inner subquery statement defining the CTE
    pub query: Box<Statement>,
}

/// Comparison and matching operators supported in WHERE clauses
#[derive(Debug, Clone, PartialEq)]
pub enum BinaryOp {
    /// =
    Equals,
    /// != or <>
    NotEquals,
    /// >
    GreaterThan,
    /// <
    LessThan,
    /// >=
    GreaterOrEqual,
    /// <=
    LessOrEqual,
    /// LIKE '%pattern%'
    Like,
    /// NOT LIKE '%pattern%'
    NotLike,
    /// MATCH 'search terms' (BM25 lexical search)
    Match,
    /// IS NULL
    IsNull,
    /// IS NOT NULL
    IsNotNull,
}

/// An individual filter condition in a WHERE clause: `column <op> value`
#[derive(Debug, Clone, PartialEq)]
pub struct WhereCondition {
    /// Column name
    pub column: String,
    /// Comparison operator
    pub op: BinaryOp,
    /// Expected literal value
    pub value: Value,
}

impl WhereCondition {
    /// Create an equality filter condition helper
    pub fn eq(column: impl Into<String>, value: Value) -> Self {
        Self {
            column: column.into(),
            op: BinaryOp::Equals,
            value,
        }
    }
}

/// Abstract Boolean Expression Tree representing flexible WHERE / HAVING clauses
#[derive(Debug, Clone, PartialEq)]
pub enum WhereExpr {
    /// Basic binary comparison `col <op> val`
    Condition(WhereCondition),
    /// Logical AND conjunction
    And(Box<WhereExpr>, Box<WhereExpr>),
    /// Logical OR disjunction
    Or(Box<WhereExpr>, Box<WhereExpr>),
    /// `col [NOT] IN (val1, val2, ...)`
    InList {
        /// Target column name
        column: String,
        /// Literal values in list
        values: Vec<Value>,
        /// Whether the condition is negated (`NOT IN`)
        negated: bool,
    },
    /// `col [NOT] IN (SELECT ...)`
    InSubquery {
        /// Target column name
        column: String,
        /// Inner SELECT subquery
        subquery: Box<Statement>,
        /// Whether the condition is negated (`NOT IN`)
        negated: bool,
    },
}

impl WhereExpr {
    /// Helper to create a single condition
    pub fn cond(column: impl Into<String>, op: BinaryOp, value: Value) -> Self {
        WhereExpr::Condition(WhereCondition {
            column: column.into(),
            op,
            value,
        })
    }

    /// Helper to create an equality condition
    pub fn eq(column: impl Into<String>, value: Value) -> Self {
        WhereExpr::Condition(WhereCondition::eq(column, value))
    }
}

/// Abstract Syntax Tree (AST) representing an executable statement in TapirusDB
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// CREATE TABLE [IF NOT EXISTS] name (columns...)
    CreateTable {
        /// Table name
        name: String,
        /// IF NOT EXISTS flag
        if_not_exists: bool,
        /// Column definitions
        columns: Vec<ColumnDef>,
    },
    /// INSERT INTO table [(columns...)] VALUES (values...)
    Insert {
        /// Target table
        table: String,
        /// Optional explicit column list
        columns: Option<Vec<String>>,
        /// Value expressions to insert
        values: Vec<Value>,
    },
    /// SELECT [DISTINCT] columns... FROM table [JOIN ...] [WHERE ...] [GROUP BY ...] [HAVING ...] [ORDER BY col [ASC|DESC]] [LIMIT n]
    Select {
        /// Whether DISTINCT was requested to filter out duplicate rows
        distinct: bool,
        /// Projected column names (empty means `*`)
        columns: Vec<String>,
        /// Source table
        table: String,
        /// Optional table JOIN
        join: Option<JoinClause>,
        /// Optional WHERE expression (supports AND, OR, IN, Subqueries)
        where_clause: Option<WhereExpr>,
        /// Optional GROUP BY columns
        group_by: Option<Vec<String>>,
        /// Optional HAVING filter expression
        having: Option<WhereExpr>,
        /// Optional ORDER BY clause
        order_by: Option<OrderByClause>,
        /// Optional row limit
        limit: Option<usize>,
        /// Optional row offset for pagination
        offset: Option<usize>,
        /// Optional point-in-time timestamp for time-travel queries
        as_of_timestamp: Option<u64>,
    },
    /// SELECT columns... FROM table VECTOR NEAR col = [vectors...] TOP k [WHERE ...]
    VectorSearch {
        /// Projected column names
        columns: Vec<String>,
        /// Source table
        table: String,
        /// Vector column name
        vector_col: String,
        /// Query vector embedding
        query_vector: Vec<f32>,
        /// Number of top nearest neighbors to retrieve
        top_k: usize,
        /// Optional SQL pre-filtering WHERE expression
        where_clause: Option<WhereExpr>,
    },
    /// GRAPH INSERT NODE id LABEL label PROPERTIES properties
    GraphInsertNode {
        /// Node ID
        id: u64,
        /// Node category label
        label: String,
        /// Properties
        properties: String,
    },
    /// GRAPH INSERT EDGE from_id -> to_id LABEL label [WEIGHT w] [PROPERTIES p]
    GraphInsertEdge {
        /// Source Node ID
        from_id: u64,
        /// Target Node ID
        to_id: u64,
        /// Edge label
        label: String,
        /// Edge weight
        weight: f32,
        /// Properties
        properties: String,
    },
    /// GRAPH TRAVERSE FROM start_id [OUTGOING|INCOMING|BOTH] [LABEL label] [MAX_DEPTH depth] [WHERE ...]
    GraphTraverse {
        /// Starting node ID
        start_id: u64,
        /// Traversal direction
        direction: crate::graph::Direction,
        /// Optional edge label filter
        label: Option<String>,
        /// Max depth of traversal hops
        max_depth: usize,
        /// Optional WHERE filter expression for target node properties and edge attributes
        where_clause: Option<WhereExpr>,
    },
    /// WITH cte AS (SELECT ...) [, ...] SELECT ...
    WithCte {
        /// List of Common Table Expressions defined in the WITH clause
        ctes: Vec<CteClause>,
        /// Main outer query executed with CTE results available
        main_query: Box<Statement>,
    },
    /// GRAPH SHORTEST_PATH FROM start_id TO end_id
    GraphShortestPath {
        /// Starting node ID
        start_id: u64,
        /// Destination node ID
        end_id: u64,
    },
    /// GRAPH MATCH (source[:Label])-[rel[:EdgeLabel]]->(target[:Label]) [WHERE ...] [RETURN ...]
    GraphMatch {
        /// Source node variable name (e.g. "a")
        source_var: String,
        /// Source node label filter (optional)
        source_label: Option<String>,
        /// Relationship edge variable name (optional)
        rel_var: Option<String>,
        /// Relationship edge label filter (optional)
        rel_label: Option<String>,
        /// Target node variable name (e.g. "b")
        target_var: String,
        /// Target node label filter (optional)
        target_label: Option<String>,
        /// Optional WHERE filter clause
        where_clause: Option<WhereExpr>,
        /// Return projections (e.g. `["a", "r", "b"]` or `["*"]`)
        return_items: Vec<String>,
    },
    /// UPDATE table SET col1 = val1, col2 = val2 [WHERE ...]
    Update {
        /// Target table
        table: String,
        /// Column assignments
        assignments: Vec<(String, Value)>,
        /// Optional WHERE expression
        where_clause: Option<WhereExpr>,
    },
    /// DELETE FROM table [WHERE ...]
    Delete {
        /// Target table
        table: String,
        /// Optional WHERE expression
        where_clause: Option<WhereExpr>,
    },
    /// BEGIN \[TRANSACTION\]
    BeginTransaction,
    /// COMMIT \[TRANSACTION\]
    CommitTransaction,
    /// ROLLBACK \[TRANSACTION\]
    RollbackTransaction,
    /// CREATE INDEX \[IF NOT EXISTS\] index_name ON table (column)
    CreateIndex {
        /// Index name
        name: String,
        /// IF NOT EXISTS flag
        if_not_exists: bool,
        /// Target table
        table: String,
        /// Column to index
        column: String,
    },
    /// EXPLAIN \[QUERY PLAN\] statement
    Explain {
        /// Statement to explain
        statement: Box<Statement>,
        /// Flag indicating whether QUERY PLAN was specified
        query_plan: bool,
    },
    /// VACUUM \[INTO 'backup.tapir'\]
    Vacuum {
        /// Optional target backup path
        into: Option<String>,
    },
    /// DROP TABLE \[IF EXISTS\] table
    DropTable {
        /// Target table name
        table: String,
        /// Flag indicating whether IF EXISTS was specified
        if_exists: bool,
    },
    /// ALTER TABLE \<table> ADD \[COLUMN\] \<column_def\>
    AlterTableAddColumn {
        /// Target table name
        table: String,
        /// New column definition
        column: ColumnDef,
    },
    /// CREATE VIEW \[IF NOT EXISTS\] \<name\> AS \<query\>
    CreateView {
        /// View name
        name: String,
        /// IF NOT EXISTS flag
        if_not_exists: bool,
        /// Reconstructed SQL query defining the view
        query_sql: String,
        /// View query AST
        query: Box<Statement>,
    },
    /// DROP VIEW [IF EXISTS] <name>
    DropView {
        /// Target view name
        name: String,
        /// Flag indicating whether IF EXISTS was specified
        if_exists: bool,
    },
    /// DROP INDEX [IF EXISTS] <name>
    DropIndex {
        /// Target index name
        name: String,
        /// Flag indicating whether IF EXISTS was specified
        if_exists: bool,
    },
    /// ANALYZE [table]
    Analyze {
        /// Optional target table name to analyze (None means all tables)
        table: Option<String>,
    },
    /// GRAPH ALGORITHM <name> [options...]
    GraphAlgorithm {
        /// Name of the graph algorithm: "PAGERANK", "CONNECTED_COMPONENTS", "BETWEENNESS", "LOUVAIN"
        algorithm: String,
        /// Optional configuration options (e.g. "damping", "iterations", "normalized")
        options: std::collections::HashMap<String, String>,
    },
}

/// Substitute '?' positional placeholders in a token stream with bound Values
pub fn bind_parameters(tokens: &[Token], params: &[Value]) -> Result<Vec<Token>> {
    let mut bound = Vec::with_capacity(tokens.len());
    let mut param_idx = 0;

    for token in tokens {
        if let Token::QuestionMark = token {
            if param_idx >= params.len() {
                return Err(Error::SqlSyntax(format!(
                    "Parameter index out of bounds: expected at least {} parameters, provided {}",
                    param_idx + 1,
                    params.len()
                )));
            }
            let val = &params[param_idx];
            param_idx += 1;
            let tok = match val {
                Value::Null => Token::Null,
                Value::Integer(i) => Token::IntLit(*i),
                Value::Real(f) => Token::FloatLit(*f),
                Value::Text(s) => Token::StringLit(s.clone()),
                Value::Blob(b) => Token::BlobLit(b.clone()),
                Value::Vector(v) => Token::VectorLit(v.clone()),
            };
            bound.push(tok);
        } else {
            bound.push(token.clone());
        }
    }

    if param_idx < params.len() {
        return Err(Error::SqlSyntax(format!(
            "Too many parameters supplied: expected {} parameters, provided {}",
            param_idx,
            params.len()
        )));
    }

    Ok(bound)
}

/// Convert a token stream slice back into SQL text representation
pub fn tokens_to_sql(tokens: &[Token]) -> String {
    let mut s = String::new();
    for (i, tok) in tokens.iter().enumerate() {
        if i > 0 {
            match tok {
                Token::Comma | Token::Semicolon | Token::CloseParen | Token::CloseBracket | Token::Dot => {}
                _ => {
                    if let Some(prev) = tokens.get(i - 1) {
                        if !matches!(prev, Token::OpenParen | Token::OpenBracket | Token::Dot) {
                            s.push(' ');
                        }
                    }
                }
            }
        }
        match tok {
            Token::Create => s.push_str("CREATE"),
            Token::Table => s.push_str("TABLE"),
            Token::If => s.push_str("IF"),
            Token::Not => s.push_str("NOT"),
            Token::Exists => s.push_str("EXISTS"),
            Token::Primary => s.push_str("PRIMARY"),
            Token::Key => s.push_str("KEY"),
            Token::Integer => s.push_str("INTEGER"),
            Token::Real => s.push_str("REAL"),
            Token::Text => s.push_str("TEXT"),
            Token::Blob => s.push_str("BLOB"),
            Token::Vector => s.push_str("VECTOR"),
            Token::Insert => s.push_str("INSERT"),
            Token::Into => s.push_str("INTO"),
            Token::Values => s.push_str("VALUES"),
            Token::Select => s.push_str("SELECT"),
            Token::From => s.push_str("FROM"),
            Token::Where => s.push_str("WHERE"),
            Token::Limit => s.push_str("LIMIT"),
            Token::Offset => s.push_str("OFFSET"),
            Token::Near => s.push_str("NEAR"),
            Token::Top => s.push_str("TOP"),
            Token::Graph => s.push_str("GRAPH"),
            Token::Node => s.push_str("NODE"),
            Token::Edge => s.push_str("EDGE"),
            Token::Match => s.push_str("MATCH"),
            Token::Null => s.push_str("NULL"),
            Token::Update => s.push_str("UPDATE"),
            Token::Set => s.push_str("SET"),
            Token::Delete => s.push_str("DELETE"),
            Token::Drop => s.push_str("DROP"),
            Token::Alter => s.push_str("ALTER"),
            Token::Add => s.push_str("ADD"),
            Token::Column => s.push_str("COLUMN"),
            Token::View => s.push_str("VIEW"),
            Token::Begin => s.push_str("BEGIN"),
            Token::Commit => s.push_str("COMMIT"),
            Token::Rollback => s.push_str("ROLLBACK"),
            Token::Transaction => s.push_str("TRANSACTION"),
            Token::Index => s.push_str("INDEX"),
            Token::On => s.push_str("ON"),
            Token::Join => s.push_str("JOIN"),
            Token::Inner => s.push_str("INNER"),
            Token::Left => s.push_str("LEFT"),
            Token::Right => s.push_str("RIGHT"),
            Token::Full => s.push_str("FULL"),
            Token::Outer => s.push_str("OUTER"),
            Token::Distinct => s.push_str("DISTINCT"),
            Token::Order => s.push_str("ORDER"),
            Token::By => s.push_str("BY"),
            Token::Asc => s.push_str("ASC"),
            Token::Desc => s.push_str("DESC"),
            Token::Count => s.push_str("COUNT"),
            Token::Sum => s.push_str("SUM"),
            Token::Avg => s.push_str("AVG"),
            Token::Min => s.push_str("MIN"),
            Token::Max => s.push_str("MAX"),
            Token::Explain => s.push_str("EXPLAIN"),
            Token::Plan => s.push_str("PLAN"),
            Token::Vacuum => s.push_str("VACUUM"),
            Token::Analyze => s.push_str("ANALYZE"),
            Token::Like => s.push_str("LIKE"),
            Token::And => s.push_str("AND"),
            Token::Or => s.push_str("OR"),
            Token::Group => s.push_str("GROUP"),
            Token::Having => s.push_str("HAVING"),
            Token::In => s.push_str("IN"),
            Token::Is => s.push_str("IS"),
            Token::Between => s.push_str("BETWEEN"),
            Token::As => s.push_str("AS"),
            Token::Of => s.push_str("OF"),
            Token::Timestamp => s.push_str("TIMESTAMP"),
            Token::Ident(id) => s.push_str(id),
            Token::StringLit(lit) => {
                s.push('\'');
                s.push_str(&lit.replace('\'', "''"));
                s.push('\'');
            }
            Token::IntLit(val) => s.push_str(&val.to_string()),
            Token::FloatLit(val) => s.push_str(&val.to_string()),
            Token::BlobLit(b) => s.push_str(&format!("X'{:02X?}'", b)),
            Token::VectorLit(v) => s.push_str(&format!("{v:?}")),
            Token::OpenParen => s.push('('),
            Token::CloseParen => s.push(')'),
            Token::OpenBracket => s.push('['),
            Token::CloseBracket => s.push(']'),
            Token::Comma => s.push(','),
            Token::Semicolon => s.push(';'),
            Token::Equals => s.push('='),
            Token::NotEquals => s.push_str("!="),
            Token::GreaterThan => s.push('>'),
            Token::LessThan => s.push('<'),
            Token::GreaterOrEqual => s.push_str(">="),
            Token::LessOrEqual => s.push_str("<="),
            Token::QuestionMark => s.push('?'),
            Token::Asterisk => s.push('*'),
            Token::Dot => s.push('.'),
            Token::With => s.push_str("WITH"),
            Token::Return => s.push_str("RETURN"),
            Token::Colon => s.push(':'),
            Token::Dash => s.push('-'),
            Token::Arrow => s.push_str("->"),
            Token::Over => s.push_str("OVER"),
            Token::Partition => s.push_str("PARTITION"),
            Token::RowNumber => s.push_str("ROW_NUMBER"),
            Token::Rank => s.push_str("RANK"),
            Token::DenseRank => s.push_str("DENSE_RANK"),
            Token::Ntile => s.push_str("NTILE"),
            Token::Lag => s.push_str("LAG"),
            Token::Lead => s.push_str("LEAD"),
            Token::Rows => s.push_str("ROWS"),
            Token::Unbounded => s.push_str("UNBOUNDED"),
            Token::Preceding => s.push_str("PRECEDING"),
            Token::Following => s.push_str("FOLLOWING"),
            Token::Current => s.push_str("CURRENT"),
            Token::Row => s.push_str("ROW"),
            Token::Algorithm => s.push_str("ALGORITHM"),
        }
    }
    s
}

/// Parse a pre-tokenized SQL token stream into an AST Statement
pub fn parse_tokens(tokens: &[Token]) -> Result<Statement> {
    if tokens.is_empty() {
        return Err(Error::SqlSyntax("Empty SQL statement".into()));
    }

    let mut cursor = 0;
    match &tokens[cursor] {
        Token::Create => {
            if cursor + 1 < tokens.len() && tokens[cursor + 1] == Token::Index {
                parse_create_index(tokens, &mut cursor)
            } else if cursor + 1 < tokens.len() && tokens[cursor + 1] == Token::View {
                parse_create_view(tokens, &mut cursor)
            } else {
                parse_create_table(tokens, &mut cursor)
            }
        }
        Token::Alter => parse_alter(tokens, &mut cursor),
        Token::Insert => parse_insert(tokens, &mut cursor),
        Token::Select => parse_select(tokens, &mut cursor),
        Token::Update => parse_update(tokens, &mut cursor),
        Token::Delete => parse_delete(tokens, &mut cursor),
        Token::Drop => parse_drop(tokens, &mut cursor),
        Token::Begin => parse_begin(tokens, &mut cursor),
        Token::Commit => parse_commit(tokens, &mut cursor),
        Token::Rollback => parse_rollback(tokens, &mut cursor),
        Token::Graph => parse_graph(tokens, &mut cursor),
        Token::Match => parse_graph_match(tokens, &mut cursor),
        Token::Explain => parse_explain(tokens, &mut cursor),
        Token::Vacuum => parse_vacuum(tokens, &mut cursor),
        Token::Analyze => parse_analyze(tokens, &mut cursor),
        Token::With => parse_with_cte(tokens, &mut cursor),
        other => Err(Error::SqlSyntax(format!(
            "Unexpected statement starting with {other:?}"
        ))),
    }
}

/// Parse a raw SQL query string into an AST Statement
pub fn parse_sql(sql: &str) -> Result<Statement> {
    let tokens = tokenize(sql)?;
    parse_tokens(&tokens)
}

fn parse_with_cte(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    expect_token(tokens, cursor, &Token::With)?;
    let mut ctes = Vec::new();

    loop {
        let name = match get_token(tokens, cursor)? {
            Token::Ident(s) => s.clone(),
            other => return Err(Error::SqlSyntax(format!("Expected CTE name, got {other:?}"))),
        };

        let mut columns = None;
        if check_token(tokens, *cursor, &Token::OpenParen) {
            *cursor += 1;
            let mut cols = Vec::new();
            while *cursor < tokens.len() {
                cols.push(parse_identifier_or_keyword(tokens, cursor)?);
                if check_token(tokens, *cursor, &Token::Comma) {
                    *cursor += 1;
                } else {
                    break;
                }
            }
            expect_token(tokens, cursor, &Token::CloseParen)?;
            columns = Some(cols);
        }

        expect_token(tokens, cursor, &Token::As)?;
        expect_token(tokens, cursor, &Token::OpenParen)?;

        // Find matching CloseParen for the inner query
        let start_inner = *cursor;
        let mut depth = 1;
        while *cursor < tokens.len() && depth > 0 {
            match &tokens[*cursor] {
                Token::OpenParen => depth += 1,
                Token::CloseParen => depth -= 1,
                _ => {}
            }
            *cursor += 1;
        }

        if depth != 0 {
            return Err(Error::SqlSyntax("Unclosed parenthesis in CTE query".into()));
        }

        let inner_tokens = &tokens[start_inner..*cursor - 1];
        let query_stmt = parse_tokens(inner_tokens)?;

        ctes.push(CteClause {
            name,
            columns,
            query: Box::new(query_stmt),
        });

        if check_token(tokens, *cursor, &Token::Comma) {
            *cursor += 1;
        } else {
            break;
        }
    }

    // Now parse the main query statement
    let main_tokens = &tokens[*cursor..];
    let main_query = parse_tokens(main_tokens)?;
    *cursor = tokens.len();

    Ok(Statement::WithCte {
        ctes,
        main_query: Box::new(main_query),
    })
}

fn parse_identifier_or_keyword(tokens: &[Token], cursor: &mut usize) -> Result<String> {
    match get_token(tokens, cursor)? {
        Token::Ident(s) => Ok(s.clone()),
        Token::Key => Ok("key".to_string()),
        Token::Plan => Ok("plan".to_string()),
        Token::Order => Ok("order".to_string()),
        Token::Match => Ok("match".to_string()),
        Token::Like => Ok("like".to_string()),
        Token::Vacuum => Ok("vacuum".to_string()),
        Token::Analyze => Ok("analyze".to_string()),
        Token::Column => Ok("column".to_string()),
        Token::View => Ok("view".to_string()),
        Token::Node => Ok("node".to_string()),
        Token::Edge => Ok("edge".to_string()),
        Token::Graph => Ok("graph".to_string()),
        other => Err(Error::SqlSyntax(format!("Expected identifier or column name, got {other:?}"))),
    }
}

fn parse_column_def(tokens: &[Token], cursor: &mut usize) -> Result<ColumnDef> {
    let col_name = parse_identifier_or_keyword(tokens, cursor)?;

    let data_type = match get_token(tokens, cursor)? {
        Token::Integer => DataType::Integer,
        Token::Real => DataType::Real,
        Token::Text => DataType::Text,
        Token::Blob => DataType::Blob,
        Token::Vector => {
            expect_token(tokens, cursor, &Token::OpenParen)?;
            let dims = match get_token(tokens, cursor)? {
                Token::IntLit(n) => *n as usize,
                other => return Err(Error::SqlSyntax(format!("Expected vector dimensions, got {other:?}"))),
            };
            expect_token(tokens, cursor, &Token::CloseParen)?;
            DataType::Vector(dims)
        }
        other => return Err(Error::SqlSyntax(format!("Expected data type, got {other:?}"))),
    };

    let mut primary_key = false;
    let mut not_null = false;

    // Optional modifiers: PRIMARY KEY, NOT NULL
    while *cursor < tokens.len() {
        if check_token(tokens, *cursor, &Token::Primary) {
            *cursor += 1;
            expect_token(tokens, cursor, &Token::Key)?;
            primary_key = true;
        } else if check_token(tokens, *cursor, &Token::Not) {
            *cursor += 1;
            expect_token(tokens, cursor, &Token::Null)?;
            not_null = true;
        } else {
            break;
        }
    }

    Ok(ColumnDef {
        name: col_name,
        data_type,
        primary_key,
        not_null,
    })
}

fn parse_create_table(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume CREATE
    expect_token(tokens, cursor, &Token::Table)?;

    let mut if_not_exists = false;
    if check_token(tokens, *cursor, &Token::If) {
        *cursor += 1;
        expect_token(tokens, cursor, &Token::Not)?;
        expect_token(tokens, cursor, &Token::Exists)?;
        if_not_exists = true;
    }

    let table_name = match get_token(tokens, cursor)? {
        Token::Ident(s) => s.clone(),
        other => return Err(Error::SqlSyntax(format!("Expected table name, got {other:?}"))),
    };

    expect_token(tokens, cursor, &Token::OpenParen)?;

    let mut columns = Vec::new();

    while *cursor < tokens.len() {
        columns.push(parse_column_def(tokens, cursor)?);

        if check_token(tokens, *cursor, &Token::Comma) {
            *cursor += 1;
        } else if check_token(tokens, *cursor, &Token::CloseParen) {
            *cursor += 1;
            break;
        } else {
            return Err(Error::SqlSyntax("Expected ',' or ')' in CREATE TABLE".into()));
        }
    }

    Ok(Statement::CreateTable {
        name: table_name,
        if_not_exists,
        columns,
    })
}

fn parse_insert(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume INSERT
    expect_token(tokens, cursor, &Token::Into)?;

    let table_name = match get_token(tokens, cursor)? {
        Token::Ident(s) => s.clone(),
        other => return Err(Error::SqlSyntax(format!("Expected table name, got {other:?}"))),
    };

    let mut columns = None;
    if check_token(tokens, *cursor, &Token::OpenParen) {
        *cursor += 1;
        let mut cols = Vec::new();
        while *cursor < tokens.len() {
            let col = parse_identifier_or_keyword(tokens, cursor)?;
            cols.push(col);
            if check_token(tokens, *cursor, &Token::Comma) {
                *cursor += 1;
            } else if check_token(tokens, *cursor, &Token::CloseParen) {
                *cursor += 1;
                break;
            }
        }
        columns = Some(cols);
    }

    expect_token(tokens, cursor, &Token::Values)?;
    expect_token(tokens, cursor, &Token::OpenParen)?;

    let mut values = Vec::new();
    while *cursor < tokens.len() {
        let val = parse_value_literal(tokens, cursor)?;
        values.push(val);

        if check_token(tokens, *cursor, &Token::Comma) {
            *cursor += 1;
        } else if check_token(tokens, *cursor, &Token::CloseParen) {
            *cursor += 1;
            break;
        } else {
            return Err(Error::SqlSyntax("Expected ',' or ')' in VALUES clause".into()));
        }
    }

    Ok(Statement::Insert {
        table: table_name,
        columns,
        values,
    })
}

fn parse_window_spec(tokens: &[Token], cursor: &mut usize) -> Result<String> {
    expect_token(tokens, cursor, &Token::Over)?;
    expect_token(tokens, cursor, &Token::OpenParen)?;
    let mut parts = Vec::new();

    // Check for PARTITION BY
    if check_token(tokens, *cursor, &Token::Partition) {
        *cursor += 1;
        expect_token(tokens, cursor, &Token::By)?;
        let mut p_cols = Vec::new();
        while *cursor < tokens.len() {
            let col = parse_column_ident(tokens, cursor)?;
            p_cols.push(col);
            if check_token(tokens, *cursor, &Token::Comma) {
                *cursor += 1;
            } else {
                break;
            }
        }
        parts.push(format!("PARTITION BY {}", p_cols.join(", ")));
    }

    // Check for ORDER BY
    if check_token(tokens, *cursor, &Token::Order) {
        *cursor += 1;
        expect_token(tokens, cursor, &Token::By)?;
        let col = parse_column_ident(tokens, cursor)?;
        let mut dir = "ASC";
        if check_token(tokens, *cursor, &Token::Desc) {
            *cursor += 1;
            dir = "DESC";
        } else if check_token(tokens, *cursor, &Token::Asc) {
            *cursor += 1;
            dir = "ASC";
        }
        parts.push(format!("ORDER BY {col} {dir}"));
    }

    // Optional ROWS frame
    if check_token(tokens, *cursor, &Token::Rows) {
        *cursor += 1;
        let mut frame_words = Vec::new();
        while *cursor < tokens.len() && !check_token(tokens, *cursor, &Token::CloseParen) {
            let tok = get_token(tokens, cursor)?;
            match tok {
                Token::Between => frame_words.push("BETWEEN".to_string()),
                Token::Unbounded => frame_words.push("UNBOUNDED".to_string()),
                Token::Preceding => frame_words.push("PRECEDING".to_string()),
                Token::Following => frame_words.push("FOLLOWING".to_string()),
                Token::Current => frame_words.push("CURRENT".to_string()),
                Token::Row => frame_words.push("ROW".to_string()),
                Token::And => frame_words.push("AND".to_string()),
                Token::IntLit(n) => frame_words.push(n.to_string()),
                other => frame_words.push(format!("{other:?}")),
            }
        }
        parts.push(format!("ROWS {}", frame_words.join(" ")));
    }

    expect_token(tokens, cursor, &Token::CloseParen)?;
    Ok(format!("OVER ({})", parts.join(" ")))
}

fn parse_column_expression(tokens: &[Token], cursor: &mut usize) -> Result<String> {
    if *cursor >= tokens.len() {
        return Err(Error::SqlSyntax("Unexpected end of tokens in column list".into()));
    }
    let mut expr = match &tokens[*cursor] {
        Token::RowNumber | Token::Rank | Token::DenseRank => {
            let func_name = match &tokens[*cursor] {
                Token::RowNumber => "ROW_NUMBER",
                Token::Rank => "RANK",
                Token::DenseRank => "DENSE_RANK",
                _ => unreachable!(),
            };
            *cursor += 1;
            expect_token(tokens, cursor, &Token::OpenParen)?;
            expect_token(tokens, cursor, &Token::CloseParen)?;
            let window_spec = parse_window_spec(tokens, cursor)?;
            format!("{func_name}() {window_spec}")
        }
        Token::Ntile => {
            *cursor += 1;
            expect_token(tokens, cursor, &Token::OpenParen)?;
            let n = match get_token(tokens, cursor)? {
                Token::IntLit(val) => *val,
                other => return Err(Error::SqlSyntax(format!("Expected integer in NTILE, got {other:?}"))),
            };
            expect_token(tokens, cursor, &Token::CloseParen)?;
            let window_spec = parse_window_spec(tokens, cursor)?;
            format!("NTILE({n}) {window_spec}")
        }
        Token::Lag | Token::Lead => {
            let func_name = match &tokens[*cursor] {
                Token::Lag => "LAG",
                Token::Lead => "LEAD",
                _ => unreachable!(),
            };
            *cursor += 1;
            expect_token(tokens, cursor, &Token::OpenParen)?;
            let col = parse_column_ident(tokens, cursor)?;
            let mut offset = 1;
            if check_token(tokens, *cursor, &Token::Comma) {
                *cursor += 1;
                match get_token(tokens, cursor)? {
                    Token::IntLit(val) => offset = *val,
                    other => return Err(Error::SqlSyntax(format!("Expected integer offset in {func_name}, got {other:?}"))),
                }
            }
            expect_token(tokens, cursor, &Token::CloseParen)?;
            let window_spec = parse_window_spec(tokens, cursor)?;
            format!("{func_name}({col}, {offset}) {window_spec}")
        }
        Token::Count | Token::Sum | Token::Avg | Token::Min | Token::Max => {
            let func_name = match &tokens[*cursor] {
                Token::Count => "COUNT",
                Token::Sum => "SUM",
                Token::Avg => "AVG",
                Token::Min => "MIN",
                Token::Max => "MAX",
                _ => unreachable!(),
            };
            *cursor += 1;
            expect_token(tokens, cursor, &Token::OpenParen)?;
            let arg = if check_token(tokens, *cursor, &Token::Asterisk) {
                *cursor += 1;
                "*".to_string()
            } else if let Ok(Token::IntLit(n)) = get_token_peek(tokens, *cursor) {
                let s = n.to_string();
                *cursor += 1;
                s
            } else {
                parse_column_ident(tokens, cursor)?
            };
            expect_token(tokens, cursor, &Token::CloseParen)?;
            if check_token(tokens, *cursor, &Token::Over) {
                let window_spec = parse_window_spec(tokens, cursor)?;
                format!("{func_name}({arg}) {window_spec}")
            } else {
                format!("{func_name}({arg})")
            }
        }
        Token::Ident(id) if id.eq_ignore_ascii_case("JSON_EXTRACT") => {
            *cursor += 1;
            expect_token(tokens, cursor, &Token::OpenParen)?;
            let col = parse_column_ident(tokens, cursor)?;
            expect_token(tokens, cursor, &Token::Comma)?;
            let path = match get_token(tokens, cursor)? {
                Token::StringLit(p) => p.clone(),
                other => return Err(Error::SqlSyntax(format!("Expected string path in JSON_EXTRACT, got {other:?}"))),
            };
            expect_token(tokens, cursor, &Token::CloseParen)?;
            format!("JSON_EXTRACT({col}, '{path}')")
        }
        _ => parse_column_ident(tokens, cursor)?,
    };

    // Check optional AS alias
    if check_token(tokens, *cursor, &Token::As) {
        *cursor += 1;
        let alias = parse_identifier_or_keyword(tokens, cursor)?;
        expr = format!("{expr} AS {alias}");
    }

    Ok(expr)
}

fn parse_column_ident(tokens: &[Token], cursor: &mut usize) -> Result<String> {
    let mut base = parse_identifier_or_keyword(tokens, cursor)?;
    while check_token(tokens, *cursor, &Token::Dot) {
        *cursor += 1;
        let sub = parse_identifier_or_keyword(tokens, cursor)?;
        base.push('.');
        base.push_str(&sub);
    }
    Ok(base)
}

fn parse_select(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume SELECT

    let distinct = if check_token(tokens, *cursor, &Token::Distinct) {
        *cursor += 1;
        true
    } else {
        false
    };

    let mut columns = Vec::new();
    if check_token(tokens, *cursor, &Token::Asterisk) {
        *cursor += 1;
    } else {
        while *cursor < tokens.len() {
            let col_expr = parse_column_expression(tokens, cursor)?;
            columns.push(col_expr);

            if check_token(tokens, *cursor, &Token::Comma) {
                *cursor += 1;
            } else if check_token(tokens, *cursor, &Token::From) {
                break;
            }
        }
    }

    expect_token(tokens, cursor, &Token::From)?;

    let table_name = match get_token(tokens, cursor)? {
        Token::Ident(s) => s.clone(),
        other => return Err(Error::SqlSyntax(format!("Expected table name, got {other:?}"))),
    };

    // Optional JOIN clause: [INNER|LEFT|RIGHT] [OUTER] JOIN other_table ON left_col = right_col
    let mut join = None;
    if check_token(tokens, *cursor, &Token::Left)
        || check_token(tokens, *cursor, &Token::Right)
        || check_token(tokens, *cursor, &Token::Full)
        || check_token(tokens, *cursor, &Token::Inner)
        || check_token(tokens, *cursor, &Token::Join)
    {
        let join_type = if check_token(tokens, *cursor, &Token::Left) {
            *cursor += 1;
            if check_token(tokens, *cursor, &Token::Outer) {
                *cursor += 1;
            }
            expect_token(tokens, cursor, &Token::Join)?;
            JoinType::Left
        } else if check_token(tokens, *cursor, &Token::Right) {
            *cursor += 1;
            if check_token(tokens, *cursor, &Token::Outer) {
                *cursor += 1;
            }
            expect_token(tokens, cursor, &Token::Join)?;
            JoinType::Right
        } else if check_token(tokens, *cursor, &Token::Full) {
            *cursor += 1;
            if check_token(tokens, *cursor, &Token::Outer) {
                *cursor += 1;
            }
            expect_token(tokens, cursor, &Token::Join)?;
            JoinType::Full
        } else {
            if check_token(tokens, *cursor, &Token::Inner) {
                *cursor += 1;
            }
            if check_token(tokens, *cursor, &Token::Outer) {
                *cursor += 1;
            }
            expect_token(tokens, cursor, &Token::Join)?;
            JoinType::Inner
        };

        let right_table = match get_token(tokens, cursor)? {
            Token::Ident(s) => s.clone(),
            other => return Err(Error::SqlSyntax(format!("Expected table name after JOIN, got {other:?}"))),
        };

        expect_token(tokens, cursor, &Token::On)?;
        let left_col = parse_column_ident(tokens, cursor)?;
        expect_token(tokens, cursor, &Token::Equals)?;
        let right_col = parse_column_ident(tokens, cursor)?;

        join = Some(JoinClause {
            table: right_table,
            left_col,
            right_col,
            is_left: join_type == JoinType::Left,
            join_type,
        });
    }

    // Check for VECTOR NEAR ... TOP k
    if check_token(tokens, *cursor, &Token::Vector) {
        *cursor += 1;
        expect_token(tokens, cursor, &Token::Near)?;

        let vector_col = match get_token(tokens, cursor)? {
            Token::Ident(s) => s.clone(),
            other => return Err(Error::SqlSyntax(format!("Expected vector column, got {other:?}"))),
        };

        expect_token(tokens, cursor, &Token::Equals)?;

        let query_val = parse_value_literal(tokens, cursor)?;
        let query_vector = match query_val {
            Value::Vector(v) => v,
            other => return Err(Error::SqlSyntax(format!("Expected vector literal, got {other:?}"))),
        };

        expect_token(tokens, cursor, &Token::Top)?;

        let top_k = match get_token(tokens, cursor)? {
            Token::IntLit(n) => *n as usize,
            other => return Err(Error::SqlSyntax(format!("Expected integer TOP k, got {other:?}"))),
        };

        let where_clause = parse_optional_where_clause(tokens, cursor)?;

        return Ok(Statement::VectorSearch {
            columns,
            table: table_name,
            vector_col,
            query_vector,
            top_k,
            where_clause,
        });
    }

    // Optional AS OF TIMESTAMP clause: AS OF TIMESTAMP <ts>
    let mut as_of_timestamp = None;
    if check_token(tokens, *cursor, &Token::As) {
        if *cursor + 2 < tokens.len()
            && tokens[*cursor + 1] == Token::Of
            && tokens[*cursor + 2] == Token::Timestamp
        {
            *cursor += 3;
            let ts = match get_token(tokens, cursor)? {
                Token::IntLit(n) => *n as u64,
                other => return Err(Error::SqlSyntax(format!("Expected integer timestamp for AS OF TIMESTAMP, got {other:?}"))),
            };
            as_of_timestamp = Some(ts);
        }
    }

    // Optional WHERE clause: WHERE ...
    let where_clause = parse_optional_where_clause(tokens, cursor)?;

    // Optional GROUP BY clause: GROUP BY col1, col2
    let mut group_by = None;
    if check_token(tokens, *cursor, &Token::Group) {
        *cursor += 1;
        expect_token(tokens, cursor, &Token::By)?;
        let mut cols = Vec::new();
        loop {
            cols.push(parse_column_ident(tokens, cursor)?);
            if check_token(tokens, *cursor, &Token::Comma) {
                *cursor += 1;
            } else {
                break;
            }
        }
        group_by = Some(cols);
    }

    // Optional HAVING clause: HAVING <expr>
    let mut having = None;
    if check_token(tokens, *cursor, &Token::Having) {
        *cursor += 1;
        having = Some(parse_where_expr(tokens, cursor)?);
    }

    // Optional ORDER BY clause: ORDER BY col [ASC|DESC] or pgvector ORDER BY col <-> '[...]' LIMIT k
    let mut order_by = None;
    if check_token(tokens, *cursor, &Token::Order) {
        *cursor += 1;
        expect_token(tokens, cursor, &Token::By)?;
        let col = parse_column_ident(tokens, cursor)?;

        // Support pgvector Euclidean/Cosine distance syntax: ORDER BY col <-> '[...]' [LIMIT k]
        let is_vector_dist = if check_token(tokens, *cursor, &Token::LessThan)
            && *cursor + 1 < tokens.len()
            && tokens[*cursor + 1] == Token::Arrow
        {
            *cursor += 2;
            true
        } else if check_token(tokens, *cursor, &Token::LessThan)
            && *cursor + 2 < tokens.len()
            && tokens[*cursor + 1] == Token::Dash
            && tokens[*cursor + 2] == Token::GreaterThan
        {
            *cursor += 3;
            true
        } else {
            false
        };

        if is_vector_dist {
            let query_val = parse_value_literal(tokens, cursor)?;
            let query_vector = match query_val {
                Value::Vector(v) => v,
                Value::Text(ref s) => {
                    let trimmed = s.trim();
                    if trimmed.starts_with('[') && trimmed.ends_with(']') {
                        let inner = &trimmed[1..trimmed.len() - 1];
                        inner
                            .split(',')
                            .map(|part| part.trim().parse::<f32>().unwrap_or(0.0))
                            .collect()
                    } else {
                        return Err(Error::SqlSyntax("Expected vector literal after <->".into()));
                    }
                }
                other => return Err(Error::SqlSyntax(format!("Expected vector literal after <->, got {other:?}"))),
            };

            let mut top_k = 10;
            if check_token(tokens, *cursor, &Token::Limit) {
                *cursor += 1;
                top_k = match get_token(tokens, cursor)? {
                    Token::IntLit(n) => *n as usize,
                    other => return Err(Error::SqlSyntax(format!("Expected integer for LIMIT, got {other:?}"))),
                };
            }

            return Ok(Statement::VectorSearch {
                columns,
                table: table_name,
                vector_col: col,
                query_vector,
                top_k,
                where_clause,
            });
        }

        let is_ascending = if check_token(tokens, *cursor, &Token::Desc) {
            *cursor += 1;
            false
        } else {
            if check_token(tokens, *cursor, &Token::Asc) {
                *cursor += 1;
            }
            true
        };
        order_by = Some(OrderByClause {
            column: col,
            is_ascending,
        });
    }

    // Optional LIMIT clause: LIMIT n
    let mut limit = None;
    if check_token(tokens, *cursor, &Token::Limit) {
        *cursor += 1;
        let n = match get_token(tokens, cursor)? {
            Token::IntLit(n) => *n as usize,
            other => return Err(Error::SqlSyntax(format!("Expected integer for LIMIT, got {other:?}"))),
        };
        limit = Some(n);
    }

    // Optional OFFSET clause: OFFSET n
    let mut offset = None;
    if check_token(tokens, *cursor, &Token::Offset) {
        *cursor += 1;
        let n = match get_token(tokens, cursor)? {
            Token::IntLit(n) => *n as usize,
            other => return Err(Error::SqlSyntax(format!("Expected integer for OFFSET, got {other:?}"))),
        };
        offset = Some(n);
    }

    Ok(Statement::Select {
        distinct,
        columns,
        table: table_name,
        join,
        where_clause,
        group_by,
        having,
        order_by,
        limit,
        offset,
        as_of_timestamp,
    })
}

fn parse_update(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume UPDATE
    let table_name = match get_token(tokens, cursor)? {
        Token::Ident(s) => s.clone(),
        other => return Err(Error::SqlSyntax(format!("Expected table name, got {other:?}"))),
    };

    expect_token(tokens, cursor, &Token::Set)?;

    let mut assignments = Vec::new();
    while *cursor < tokens.len() {
        let col_name = parse_identifier_or_keyword(tokens, cursor)?;
        expect_token(tokens, cursor, &Token::Equals)?;
        let val = parse_value_literal(tokens, cursor)?;
        assignments.push((col_name, val));

        if check_token(tokens, *cursor, &Token::Comma) {
            *cursor += 1;
        } else {
            break;
        }
    }

    if assignments.is_empty() {
        return Err(Error::SqlSyntax("UPDATE statement missing SET assignments".into()));
    }

    let where_clause = parse_optional_where_clause(tokens, cursor)?;

    Ok(Statement::Update {
        table: table_name,
        assignments,
        where_clause,
    })
}

fn parse_delete(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume DELETE
    expect_token(tokens, cursor, &Token::From)?;
    let table_name = match get_token(tokens, cursor)? {
        Token::Ident(s) => s.clone(),
        other => return Err(Error::SqlSyntax(format!("Expected table name in DELETE FROM, got {other:?}"))),
    };

    let where_clause = parse_optional_where_clause(tokens, cursor)?;

    Ok(Statement::Delete {
        table: table_name,
        where_clause,
    })
}

fn parse_alter(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume ALTER
    expect_token(tokens, cursor, &Token::Table)?;
    let table_name = parse_identifier_or_keyword(tokens, cursor)?;
    expect_token(tokens, cursor, &Token::Add)?;
    if check_token(tokens, *cursor, &Token::Column) {
        *cursor += 1; // optional COLUMN keyword
    }
    let col = parse_column_def(tokens, cursor)?;
    Ok(Statement::AlterTableAddColumn {
        table: table_name,
        column: col,
    })
}

fn parse_create_view(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume CREATE
    expect_token(tokens, cursor, &Token::View)?;

    let mut if_not_exists = false;
    if check_token(tokens, *cursor, &Token::If) {
        *cursor += 1;
        expect_token(tokens, cursor, &Token::Not)?;
        expect_token(tokens, cursor, &Token::Exists)?;
        if_not_exists = true;
    }

    let view_name = parse_identifier_or_keyword(tokens, cursor)?;
    expect_token(tokens, cursor, &Token::As)?;

    let query_start = *cursor;
    let query_stmt = parse_select(tokens, cursor)?;
    let query_sql = tokens_to_sql(&tokens[query_start..*cursor]);

    Ok(Statement::CreateView {
        name: view_name,
        if_not_exists,
        query_sql,
        query: Box::new(query_stmt),
    })
}

fn parse_drop(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume DROP
    if check_token(tokens, *cursor, &Token::View) {
        *cursor += 1; // consume VIEW
        let mut if_exists = false;
        if check_token(tokens, *cursor, &Token::If) {
            *cursor += 1;
            expect_token(tokens, cursor, &Token::Exists)?;
            if_exists = true;
        }
        let view_name = parse_identifier_or_keyword(tokens, cursor)?;
        return Ok(Statement::DropView {
            name: view_name,
            if_exists,
        });
    }

    if check_token(tokens, *cursor, &Token::Index) {
        *cursor += 1; // consume INDEX
        let mut if_exists = false;
        if check_token(tokens, *cursor, &Token::If) {
            *cursor += 1;
            expect_token(tokens, cursor, &Token::Exists)?;
            if_exists = true;
        }
        let index_name = parse_identifier_or_keyword(tokens, cursor)?;
        return Ok(Statement::DropIndex {
            name: index_name,
            if_exists,
        });
    }

    expect_token(tokens, cursor, &Token::Table)?;

    let mut if_exists = false;
    if check_token(tokens, *cursor, &Token::If) {
        *cursor += 1;
        expect_token(tokens, cursor, &Token::Exists)?;
        if_exists = true;
    }

    let table_name = parse_identifier_or_keyword(tokens, cursor)?;

    Ok(Statement::DropTable {
        table: table_name,
        if_exists,
    })
}

fn parse_begin(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume BEGIN
    if check_token(tokens, *cursor, &Token::Transaction) {
        *cursor += 1;
    }
    Ok(Statement::BeginTransaction)
}

fn parse_commit(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume COMMIT
    if check_token(tokens, *cursor, &Token::Transaction) {
        *cursor += 1;
    }
    Ok(Statement::CommitTransaction)
}

fn parse_rollback(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume ROLLBACK
    if check_token(tokens, *cursor, &Token::Transaction) {
        *cursor += 1;
    }
    Ok(Statement::RollbackTransaction)
}

fn parse_create_index(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume CREATE
    expect_token(tokens, cursor, &Token::Index)?;

    let mut if_not_exists = false;
    if check_token(tokens, *cursor, &Token::If) {
        *cursor += 1;
        expect_token(tokens, cursor, &Token::Not)?;
        expect_token(tokens, cursor, &Token::Exists)?;
        if_not_exists = true;
    }

    let index_name = match get_token(tokens, cursor)? {
        Token::Ident(s) => s.clone(),
        other => return Err(Error::SqlSyntax(format!("Expected index name, got {other:?}"))),
    };

    expect_token(tokens, cursor, &Token::On)?;

    let table_name = match get_token(tokens, cursor)? {
        Token::Ident(s) => s.clone(),
        other => return Err(Error::SqlSyntax(format!("Expected table name in CREATE INDEX ON, got {other:?}"))),
    };

    expect_token(tokens, cursor, &Token::OpenParen)?;
    let column_name = parse_column_ident(tokens, cursor)?;
    expect_token(tokens, cursor, &Token::CloseParen)?;

    Ok(Statement::CreateIndex {
        name: index_name,
        if_not_exists,
        table: table_name,
        column: column_name,
    })
}

fn parse_where_expr(tokens: &[Token], cursor: &mut usize) -> Result<WhereExpr> {
    parse_or_expr(tokens, cursor)
}

fn parse_or_expr(tokens: &[Token], cursor: &mut usize) -> Result<WhereExpr> {
    let mut expr = parse_and_expr(tokens, cursor)?;
    while check_token(tokens, *cursor, &Token::Or) {
        *cursor += 1;
        let right = parse_and_expr(tokens, cursor)?;
        expr = WhereExpr::Or(Box::new(expr), Box::new(right));
    }
    Ok(expr)
}

fn parse_and_expr(tokens: &[Token], cursor: &mut usize) -> Result<WhereExpr> {
    let mut expr = parse_primary_where_expr(tokens, cursor)?;
    while check_token(tokens, *cursor, &Token::And) {
        *cursor += 1;
        let right = parse_primary_where_expr(tokens, cursor)?;
        expr = WhereExpr::And(Box::new(expr), Box::new(right));
    }
    Ok(expr)
}

fn parse_primary_where_expr(tokens: &[Token], cursor: &mut usize) -> Result<WhereExpr> {
    if check_token(tokens, *cursor, &Token::OpenParen) {
        *cursor += 1;
        let inner = parse_or_expr(tokens, cursor)?;
        expect_token(tokens, cursor, &Token::CloseParen)?;
        return Ok(inner);
    }

    let col_name = parse_column_expression(tokens, cursor)?;

    if check_token(tokens, *cursor, &Token::Is) {
        *cursor += 1;
        let is_not = if check_token(tokens, *cursor, &Token::Not) {
            *cursor += 1;
            true
        } else {
            false
        };
        expect_token(tokens, cursor, &Token::Null)?;
        return Ok(WhereExpr::Condition(WhereCondition {
            column: col_name,
            op: if is_not { BinaryOp::IsNotNull } else { BinaryOp::IsNull },
            value: Value::Null,
        }));
    }

    if check_token(tokens, *cursor, &Token::Between) {
        *cursor += 1;
        let val1 = parse_value_literal(tokens, cursor)?;
        expect_token(tokens, cursor, &Token::And)?;
        let val2 = parse_value_literal(tokens, cursor)?;
        let ge = WhereExpr::cond(col_name.clone(), BinaryOp::GreaterOrEqual, val1);
        let le = WhereExpr::cond(col_name, BinaryOp::LessOrEqual, val2);
        return Ok(WhereExpr::And(Box::new(ge), Box::new(le)));
    }

    let negated = if check_token(tokens, *cursor, &Token::Not) {
        *cursor += 1;
        true
    } else {
        false
    };

    if check_token(tokens, *cursor, &Token::In) {
        *cursor += 1;
        expect_token(tokens, cursor, &Token::OpenParen)?;
        if check_token(tokens, *cursor, &Token::Select) {
            let subquery = parse_select(tokens, cursor)?;
            expect_token(tokens, cursor, &Token::CloseParen)?;
            return Ok(WhereExpr::InSubquery {
                column: col_name,
                subquery: Box::new(subquery),
                negated,
            });
        } else {
            let mut values = Vec::new();
            while *cursor < tokens.len() {
                values.push(parse_value_literal(tokens, cursor)?);
                if check_token(tokens, *cursor, &Token::Comma) {
                    *cursor += 1;
                } else {
                    break;
                }
            }
            expect_token(tokens, cursor, &Token::CloseParen)?;
            return Ok(WhereExpr::InList {
                column: col_name,
                values,
                negated,
            });
        }
    }

    let raw_op = match get_token(tokens, cursor)? {
        Token::Equals => BinaryOp::Equals,
        Token::NotEquals => BinaryOp::NotEquals,
        Token::GreaterThan => BinaryOp::GreaterThan,
        Token::LessThan => BinaryOp::LessThan,
        Token::GreaterOrEqual => BinaryOp::GreaterOrEqual,
        Token::LessOrEqual => BinaryOp::LessOrEqual,
        Token::Like => BinaryOp::Like,
        Token::Match => BinaryOp::Match,
        other => return Err(Error::SqlSyntax(format!("Expected comparison operator in WHERE, got {other:?}"))),
    };

    let op = if negated {
        match raw_op {
            BinaryOp::Like => BinaryOp::NotLike,
            BinaryOp::Equals => BinaryOp::NotEquals,
            BinaryOp::NotEquals => BinaryOp::Equals,
            BinaryOp::GreaterThan => BinaryOp::LessOrEqual,
            BinaryOp::LessThan => BinaryOp::GreaterOrEqual,
            BinaryOp::GreaterOrEqual => BinaryOp::LessThan,
            BinaryOp::LessOrEqual => BinaryOp::GreaterThan,
            BinaryOp::NotLike => BinaryOp::Like,
            BinaryOp::Match => return Err(Error::SqlSyntax("NOT MATCH is not supported".into())),
            BinaryOp::IsNull => BinaryOp::IsNotNull,
            BinaryOp::IsNotNull => BinaryOp::IsNull,
        }
    } else {
        raw_op
    };

    let val = parse_value_literal(tokens, cursor)?;
    Ok(WhereExpr::Condition(WhereCondition {
        column: col_name,
        op,
        value: val,
    }))
}

fn parse_optional_where_clause(tokens: &[Token], cursor: &mut usize) -> Result<Option<WhereExpr>> {
    if !check_token(tokens, *cursor, &Token::Where) {
        return Ok(None);
    }
    *cursor += 1; // consume WHERE
    let expr = parse_where_expr(tokens, cursor)?;
    Ok(Some(expr))
}

fn parse_explain(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume EXPLAIN
    let mut query_plan = false;
    if check_token(tokens, *cursor, &Token::Plan) {
        *cursor += 1;
        query_plan = true;
    } else if let Ok(Token::Ident(s)) = get_token_peek(tokens, *cursor) {
        if s.eq_ignore_ascii_case("QUERY") {
            *cursor += 1;
            if check_token(tokens, *cursor, &Token::Plan) {
                *cursor += 1;
                query_plan = true;
            } else if let Ok(Token::Ident(p)) = get_token_peek(tokens, *cursor) {
                if p.eq_ignore_ascii_case("PLAN") {
                    *cursor += 1;
                    query_plan = true;
                }
            }
        } else if s.eq_ignore_ascii_case("PLAN") {
            *cursor += 1;
            query_plan = true;
        }
    }

    let inner_stmt = match get_token_peek(tokens, *cursor)? {
        Token::Select => parse_select(tokens, cursor)?,
        Token::Insert => parse_insert(tokens, cursor)?,
        Token::Update => parse_update(tokens, cursor)?,
        Token::Delete => parse_delete(tokens, cursor)?,
        other => return Err(Error::SqlSyntax(format!("EXPLAIN does not support {other:?}"))),
    };

    Ok(Statement::Explain {
        statement: Box::new(inner_stmt),
        query_plan,
    })
}

fn parse_vacuum(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume VACUUM
    let mut into = None;
    if check_token(tokens, *cursor, &Token::Into) {
        *cursor += 1; // consume INTO
        match get_token(tokens, cursor)? {
            Token::StringLit(s) => into = Some(s.clone()),
            other => return Err(Error::SqlSyntax(format!("Expected destination file path string after VACUUM INTO, got {other:?}"))),
        }
    }
    Ok(Statement::Vacuum { into })
}

fn parse_analyze(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume ANALYZE
    let table = if *cursor < tokens.len() {
        if check_token(tokens, *cursor, &Token::Semicolon) {
            None
        } else {
            let t = parse_identifier_or_keyword(tokens, cursor)?;
            Some(t)
        }
    } else {
        None
    };
    Ok(Statement::Analyze { table })
}

fn parse_graph(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    *cursor += 1; // consume GRAPH

    if check_token(tokens, *cursor, &Token::Match) {
        return parse_graph_match(tokens, cursor);
    }
    if let Ok(Token::Ident(s)) = get_token_peek(tokens, *cursor) {
        if s.eq_ignore_ascii_case("MATCH") {
            return parse_graph_match(tokens, cursor);
        }
    }

    if check_token(tokens, *cursor, &Token::Algorithm)
        || matches!(get_token_peek(tokens, *cursor), Ok(Token::Ident(s)) if s.eq_ignore_ascii_case("ALGORITHM"))
    {
        *cursor += 1;
        let algo_name = parse_identifier_or_keyword(tokens, cursor)?.to_uppercase();
        let mut options = std::collections::HashMap::new();

        while *cursor < tokens.len() && !check_token(tokens, *cursor, &Token::Semicolon) {
            let key = parse_identifier_or_keyword(tokens, cursor)?.to_lowercase();
            if check_token(tokens, *cursor, &Token::Equals) {
                *cursor += 1;
            }
            let val = match get_token(tokens, cursor)? {
                Token::FloatLit(f) => f.to_string(),
                Token::IntLit(i) => i.to_string(),
                Token::StringLit(s) => s.clone(),
                Token::Ident(s) => s.clone(),
                other => return Err(Error::SqlSyntax(format!("Unexpected value for option {key}: {other:?}"))),
            };
            options.insert(key, val);
        }

        return Ok(Statement::GraphAlgorithm {
            algorithm: algo_name,
            options,
        });
    }

    if check_token(tokens, *cursor, &Token::Insert) {
        *cursor += 1; // consume INSERT
        if check_token(tokens, *cursor, &Token::Node) {
            *cursor += 1;
            let id = match get_token(tokens, cursor)? {
                Token::IntLit(n) => *n as u64,
                other => return Err(Error::SqlSyntax(format!("Expected node ID integer, got {other:?}"))),
            };

            // Optional LABEL
            let mut label = "Default".to_string();
            if let Ok(Token::Ident(s)) = get_token_peek(tokens, *cursor) {
                if s.eq_ignore_ascii_case("LABEL") {
                    *cursor += 1;
                    match get_token(tokens, cursor)? {
                        Token::Ident(l) | Token::StringLit(l) => label = l.clone(),
                        other => return Err(Error::SqlSyntax(format!("Expected label name, got {other:?}"))),
                    }
                }
            }

            // Optional PROPERTIES
            let mut properties = String::new();
            if let Ok(Token::Ident(s)) = get_token_peek(tokens, *cursor) {
                if s.eq_ignore_ascii_case("PROPERTIES") {
                    *cursor += 1;
                    match get_token(tokens, cursor)? {
                        Token::StringLit(p) => properties = p.clone(),
                        other => return Err(Error::SqlSyntax(format!("Expected properties string, got {other:?}"))),
                    }
                }
            }

            Ok(Statement::GraphInsertNode { id, label, properties })
        } else if check_token(tokens, *cursor, &Token::Edge) {
            *cursor += 1;
            let from_id = match get_token(tokens, cursor)? {
                Token::IntLit(n) => *n as u64,
                other => return Err(Error::SqlSyntax(format!("Expected from_id integer, got {other:?}"))),
            };

            // Optional arrow -> or TO keyword
            if check_token(tokens, *cursor, &Token::Arrow) {
                *cursor += 1;
            } else if check_token(tokens, *cursor, &Token::Dash)
                && *cursor + 1 < tokens.len()
                && tokens[*cursor + 1] == Token::GreaterThan
            {
                *cursor += 2;
            } else if let Ok(Token::Ident(to_kw)) = get_token_peek(tokens, *cursor) {
                if to_kw.eq_ignore_ascii_case("TO") {
                    *cursor += 1;
                }
            }

            let to_id = match get_token(tokens, cursor)? {
                Token::IntLit(n) => *n as u64,
                other => return Err(Error::SqlSyntax(format!("Expected to_id integer, got {other:?}"))),
            };

            let mut label = "CONNECTS".to_string();
            let mut weight = 1.0;
            let mut properties = String::new();

            while *cursor < tokens.len() {
                if let Ok(Token::Ident(s)) = get_token_peek(tokens, *cursor) {
                    if s.eq_ignore_ascii_case("LABEL") {
                        *cursor += 1;
                        match get_token(tokens, cursor)? {
                            Token::Ident(l) | Token::StringLit(l) => label = l.clone(),
                            other => return Err(Error::SqlSyntax(format!("Expected edge label, got {other:?}"))),
                        }
                    } else if s.eq_ignore_ascii_case("WEIGHT") {
                        *cursor += 1;
                        match get_token(tokens, cursor)? {
                            Token::FloatLit(w) => weight = *w as f32,
                            Token::IntLit(w) => weight = *w as f32,
                            other => return Err(Error::SqlSyntax(format!("Expected weight number, got {other:?}"))),
                        }
                    } else if s.eq_ignore_ascii_case("PROPERTIES") {
                        *cursor += 1;
                        match get_token(tokens, cursor)? {
                            Token::StringLit(p) => properties = p.clone(),
                            other => return Err(Error::SqlSyntax(format!("Expected properties string, got {other:?}"))),
                        }
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }

            Ok(Statement::GraphInsertEdge {
                from_id,
                to_id,
                label,
                weight,
                properties,
            })
        } else {
            Err(Error::SqlSyntax("Expected NODE or EDGE after GRAPH INSERT".into()))
        }
    } else if let Ok(Token::Ident(cmd)) = get_token_peek(tokens, *cursor) {
        if cmd.eq_ignore_ascii_case("TRAVERSE") {
            *cursor += 1; // consume TRAVERSE
            expect_token(tokens, cursor, &Token::From)?;
            let start_id = match get_token(tokens, cursor)? {
                Token::IntLit(n) => *n as u64,
                other => return Err(Error::SqlSyntax(format!("Expected integer start_id for GRAPH TRAVERSE, got {other:?}"))),
            };

            let mut direction = crate::graph::Direction::Both;
            let mut label = None;
            let mut max_depth = 1usize;

            while *cursor < tokens.len() {
                if let Ok(Token::Ident(opt)) = get_token_peek(tokens, *cursor) {
                    if opt.eq_ignore_ascii_case("OUTGOING") {
                        *cursor += 1;
                        direction = crate::graph::Direction::Outgoing;
                    } else if opt.eq_ignore_ascii_case("INCOMING") {
                        *cursor += 1;
                        direction = crate::graph::Direction::Incoming;
                    } else if opt.eq_ignore_ascii_case("BOTH") {
                        *cursor += 1;
                        direction = crate::graph::Direction::Both;
                    } else if opt.eq_ignore_ascii_case("LABEL") {
                        *cursor += 1;
                        match get_token(tokens, cursor)? {
                            Token::Ident(l) | Token::StringLit(l) => label = Some(l.clone()),
                            other => return Err(Error::SqlSyntax(format!("Expected label name in GRAPH TRAVERSE, got {other:?}"))),
                        }
                    } else if opt.eq_ignore_ascii_case("MAX_DEPTH") || opt.eq_ignore_ascii_case("DEPTH") {
                        *cursor += 1;
                        match get_token(tokens, cursor)? {
                            Token::IntLit(d) => max_depth = (*d).max(1) as usize,
                            other => return Err(Error::SqlSyntax(format!("Expected integer depth, got {other:?}"))),
                        }
                    } else {
                        break;
                    }
                } else if check_token(tokens, *cursor, &Token::StringLit(Default::default())) {
                    match get_token(tokens, cursor)? {
                        Token::StringLit(l) => label = Some(l.clone()),
                        _ => unreachable!(),
                    }
                } else {
                    break;
                }
            }

            // Optional WHERE clause for predicate filtering on graph traversal
            let where_clause = if check_token(tokens, *cursor, &Token::Where) {
                *cursor += 1;
                Some(parse_where_expr(tokens, cursor)?)
            } else {
                None
            };

            Ok(Statement::GraphTraverse {
                start_id,
                direction,
                label,
                max_depth,
                where_clause,
            })
        } else if cmd.eq_ignore_ascii_case("SHORTEST_PATH") || cmd.eq_ignore_ascii_case("PATH") {
            *cursor += 1; // consume SHORTEST_PATH
            expect_token(tokens, cursor, &Token::From)?;
            let start_id = match get_token(tokens, cursor)? {
                Token::IntLit(n) => *n as u64,
                other => return Err(Error::SqlSyntax(format!("Expected integer start_id for GRAPH SHORTEST_PATH, got {other:?}"))),
            };

            // Optional TO keyword
            if let Ok(Token::Ident(to_kw)) = get_token_peek(tokens, *cursor) {
                if to_kw.eq_ignore_ascii_case("TO") {
                    *cursor += 1;
                }
            } else if check_token(tokens, *cursor, &Token::Into) {
                *cursor += 1;
            }

            let end_id = match get_token(tokens, cursor)? {
                Token::IntLit(n) => *n as u64,
                other => return Err(Error::SqlSyntax(format!("Expected integer end_id for GRAPH SHORTEST_PATH, got {other:?}"))),
            };

            Ok(Statement::GraphShortestPath { start_id, end_id })
        } else {
            Err(Error::SqlSyntax(format!("Expected INSERT, TRAVERSE, SHORTEST_PATH, or MATCH after GRAPH, got {cmd}")))
        }
    } else {
        Err(Error::SqlSyntax("Expected INSERT, TRAVERSE, SHORTEST_PATH, or MATCH after GRAPH".into()))
    }
}

/// Parse a declarative Cypher/GQL-Lite pattern match query:
/// `GRAPH MATCH (a:Person)-[r:FOUNDED]->(b:Project) WHERE a.id = 1 RETURN a.name, b.title`
pub fn parse_graph_match(tokens: &[Token], cursor: &mut usize) -> Result<Statement> {
    if check_token(tokens, *cursor, &Token::Graph) {
        *cursor += 1;
    }
    if check_token(tokens, *cursor, &Token::Match) {
        *cursor += 1;
    } else if let Ok(Token::Ident(s)) = get_token_peek(tokens, *cursor) {
        if s.eq_ignore_ascii_case("MATCH") {
            *cursor += 1;
        }
    }

    // 1. Source node: (var [: label])
    expect_token(tokens, cursor, &Token::OpenParen)?;
    let source_var = parse_identifier_or_keyword(tokens, cursor)?;
    let source_label = if check_token(tokens, *cursor, &Token::Colon) {
        *cursor += 1;
        Some(parse_identifier_or_keyword(tokens, cursor)?)
    } else {
        None
    };
    expect_token(tokens, cursor, &Token::CloseParen)?;

    // 2. Relationship: -> or -[ [var] [: label] ]->
    let mut rel_var = None;
    let mut rel_label = None;

    if check_token(tokens, *cursor, &Token::Arrow) {
        *cursor += 1; // (a)->(b)
    } else if check_token(tokens, *cursor, &Token::Dash) {
        *cursor += 1;
        if check_token(tokens, *cursor, &Token::OpenBracket) {
            *cursor += 1;
            // Variable name (if not colon)
            if !check_token(tokens, *cursor, &Token::Colon) && !check_token(tokens, *cursor, &Token::CloseBracket) {
                let v = parse_identifier_or_keyword(tokens, cursor)?;
                rel_var = Some(v);
            }
            if check_token(tokens, *cursor, &Token::Colon) {
                *cursor += 1;
                let l = parse_identifier_or_keyword(tokens, cursor)?;
                rel_label = Some(l);
            }
            expect_token(tokens, cursor, &Token::CloseBracket)?;
            if check_token(tokens, *cursor, &Token::Arrow) {
                *cursor += 1;
            } else if check_token(tokens, *cursor, &Token::Dash) {
                *cursor += 1;
                if check_token(tokens, *cursor, &Token::GreaterThan) {
                    *cursor += 1;
                }
            }
        } else if check_token(tokens, *cursor, &Token::GreaterThan) {
            *cursor += 1;
        }
    }

    // 3. Target node: (var [: label])
    expect_token(tokens, cursor, &Token::OpenParen)?;
    let target_var = parse_identifier_or_keyword(tokens, cursor)?;
    let target_label = if check_token(tokens, *cursor, &Token::Colon) {
        *cursor += 1;
        Some(parse_identifier_or_keyword(tokens, cursor)?)
    } else {
        None
    };
    expect_token(tokens, cursor, &Token::CloseParen)?;

    // 4. Optional WHERE
    let where_clause = if check_token(tokens, *cursor, &Token::Where) {
        *cursor += 1;
        Some(parse_where_expr(tokens, cursor)?)
    } else {
        None
    };

    // 5. Optional RETURN
    let mut return_items = Vec::new();
    if check_token(tokens, *cursor, &Token::Return) {
        *cursor += 1;
        loop {
            if check_token(tokens, *cursor, &Token::Asterisk) {
                *cursor += 1;
                return_items.push("*".to_string());
            } else {
                let item = parse_identifier_or_keyword(tokens, cursor)?;
                if check_token(tokens, *cursor, &Token::Dot) {
                    *cursor += 1;
                    let sub = parse_identifier_or_keyword(tokens, cursor)?;
                    return_items.push(format!("{item}.{sub}"));
                } else {
                    return_items.push(item);
                }
            }

            if check_token(tokens, *cursor, &Token::Comma) {
                *cursor += 1;
            } else {
                break;
            }
        }
    }

    if return_items.is_empty() {
        return_items = vec![
            source_var.clone(),
            rel_var.clone().unwrap_or_else(|| "edge".to_string()),
            target_var.clone(),
        ];
    }

    Ok(Statement::GraphMatch {
        source_var,
        source_label,
        rel_var,
        rel_label,
        target_var,
        target_label,
        where_clause,
        return_items,
    })
}

fn parse_value_literal(tokens: &[Token], cursor: &mut usize) -> Result<Value> {
    match get_token(tokens, cursor)? {
        Token::IntLit(i) => Ok(Value::Integer(*i)),
        Token::FloatLit(f) => Ok(Value::Real(*f)),
        Token::StringLit(s) => Ok(Value::Text(s.clone())),
        Token::BlobLit(b) => Ok(Value::Blob(b.clone())),
        Token::VectorLit(v) => Ok(Value::Vector(v.clone())),
        Token::Null => Ok(Value::Null),
        Token::Ident(s) if s.eq_ignore_ascii_case("true") => Ok(Value::Integer(1)),
        Token::Ident(s) if s.eq_ignore_ascii_case("false") => Ok(Value::Integer(0)),
        Token::OpenBracket => {
            // Vector literal: [0.1, 0.2, 0.3]
            let mut floats = Vec::new();
            while *cursor < tokens.len() {
                match get_token(tokens, cursor)? {
                    Token::FloatLit(f) => floats.push(*f as f32),
                    Token::IntLit(i) => floats.push(*i as f32),
                    other => return Err(Error::SqlSyntax(format!("Expected float in vector, got {other:?}"))),
                }
                if check_token(tokens, *cursor, &Token::Comma) {
                    *cursor += 1;
                } else if check_token(tokens, *cursor, &Token::CloseBracket) {
                    *cursor += 1;
                    break;
                }
            }
            Ok(Value::Vector(floats))
        }
        other => Err(Error::SqlSyntax(format!("Unexpected value token {other:?}"))),
    }
}

fn expect_token<'a>(tokens: &'a [Token], cursor: &mut usize, expected: &Token) -> Result<&'a Token> {
    if *cursor >= tokens.len() {
        return Err(Error::SqlSyntax(format!("Unexpected end of input, expected {expected:?}")));
    }
    if &tokens[*cursor] == expected {
        let t = &tokens[*cursor];
        *cursor += 1;
        Ok(t)
    } else {
        Err(Error::SqlSyntax(format!(
            "Expected {expected:?}, found {:?}",
            tokens[*cursor]
        )))
    }
}

fn check_token(tokens: &[Token], cursor: usize, expected: &Token) -> bool {
    if cursor < tokens.len() {
        &tokens[cursor] == expected
    } else {
        false
    }
}

fn get_token<'a>(tokens: &'a [Token], cursor: &mut usize) -> Result<&'a Token> {
    if *cursor >= tokens.len() {
        return Err(Error::SqlSyntax("Unexpected end of input".into()));
    }
    let t = &tokens[*cursor];
    *cursor += 1;
    Ok(t)
}

fn get_token_peek(tokens: &[Token], cursor: usize) -> Result<&Token> {
    if cursor >= tokens.len() {
        return Err(Error::SqlSyntax("Unexpected end of input".into()));
    }
    Ok(&tokens[cursor])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_create_table() {
        let sql = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, embedding VECTOR(128));";
        let stmt = parse_sql(sql).expect("Parse CREATE TABLE");
        match stmt {
            Statement::CreateTable { name, columns, .. } => {
                assert_eq!(name, "users");
                assert_eq!(columns.len(), 3);
                assert_eq!(columns[0].name, "id");
                assert!(columns[0].primary_key);
                assert_eq!(columns[2].data_type, DataType::Vector(128));
            }
            _ => panic!("Expected CreateTable"),
        }
    }

    #[test]
    fn test_parse_insert() {
        let sql = "INSERT INTO users (id, name, embedding) VALUES (1, 'Faiz', [0.1, 0.2, 0.3]);";
        let stmt = parse_sql(sql).expect("Parse INSERT");
        match stmt {
            Statement::Insert { table, values, .. } => {
                assert_eq!(table, "users");
                assert_eq!(values.len(), 3);
                assert_eq!(values[0], Value::Integer(1));
                assert_eq!(values[1], Value::Text("Faiz".into()));
                assert_eq!(values[2], Value::Vector(vec![0.1, 0.2, 0.3]));
            }
            _ => panic!("Expected Insert"),
        }
    }

    #[test]
    fn test_parse_vector_search() {
        let sql = "SELECT id, name FROM users VECTOR NEAR embedding = [0.1, 0.2, 0.3] TOP 5;";
        let stmt = parse_sql(sql).expect("Parse VECTOR NEAR");
        match stmt {
            Statement::VectorSearch { table, vector_col, query_vector, top_k, .. } => {
                assert_eq!(table, "users");
                assert_eq!(vector_col, "embedding");
                assert_eq!(query_vector.len(), 3);
                assert_eq!(top_k, 5);
            }
            _ => panic!("Expected VectorSearch"),
        }
    }

    #[test]
    fn test_parse_update_and_delete() {
        let sql = "UPDATE users SET name = 'Ahmad', age = 30 WHERE id = 1;";
        let stmt = parse_sql(sql).expect("Parse UPDATE");
        match stmt {
            Statement::Update { table, assignments, where_clause } => {
                assert_eq!(table, "users");
                assert_eq!(assignments.len(), 2);
                assert_eq!(assignments[0], ("name".to_string(), Value::Text("Ahmad".into())));
                assert_eq!(assignments[1], ("age".to_string(), Value::Integer(30)));
                assert_eq!(where_clause, Some(WhereExpr::eq("id", Value::Integer(1))));
            }
            _ => panic!("Expected Update"),
        }

        let sql_del = "DELETE FROM users WHERE id = 42;";
        let stmt_del = parse_sql(sql_del).expect("Parse DELETE");
        match stmt_del {
            Statement::Delete { table, where_clause } => {
                assert_eq!(table, "users");
                assert_eq!(where_clause, Some(WhereExpr::eq("id", Value::Integer(42))));
            }
            _ => panic!("Expected Delete"),
        }
    }

    #[test]
    fn test_parse_explain_and_vacuum() {
        let sql_exp = "EXPLAIN QUERY PLAN SELECT id FROM items WHERE price > 50 AND price <= 100;";
        let stmt_exp = parse_sql(sql_exp).expect("Parse EXPLAIN");
        match stmt_exp {
            Statement::Explain { statement, query_plan } => {
                assert!(query_plan);
                match *statement {
                    Statement::Select { table, where_clause, .. } => {
                        assert_eq!(table, "items");
                        let expr = where_clause.expect("where clause");
                        match expr {
                            WhereExpr::And(l, r) => {
                                assert_eq!(*l, WhereExpr::cond("price", BinaryOp::GreaterThan, Value::Integer(50)));
                                assert_eq!(*r, WhereExpr::cond("price", BinaryOp::LessOrEqual, Value::Integer(100)));
                            }
                            _ => panic!("Expected And expression"),
                        }
                    }
                    _ => panic!("Expected inner Select"),
                }
            }
            _ => panic!("Expected Explain"),
        }

        let sql_vac = "VACUUM INTO 'backup.tapir';";
        let stmt_vac = parse_sql(sql_vac).expect("Parse VACUUM");
        match stmt_vac {
            Statement::Vacuum { into } => {
                assert_eq!(into, Some("backup.tapir".to_string()));
            }
            _ => panic!("Expected Vacuum"),
        }
    }

    #[test]
    fn test_parse_group_by_having_or_and_subquery() {
        // Test OR and parenthesis
        let sql_or = "SELECT id FROM users WHERE (status = 'active' AND age > 20) OR role = 'admin';";
        let stmt_or = parse_sql(sql_or).expect("Parse OR");
        match stmt_or {
            Statement::Select { where_clause, .. } => {
                assert!(matches!(where_clause, Some(WhereExpr::Or(_, _))));
            }
            _ => panic!("Expected Select"),
        }

        // Test GROUP BY and HAVING
        let sql_gb = "SELECT dept, COUNT(*), AVG(salary) FROM employees GROUP BY dept HAVING COUNT(*) > 5;";
        let stmt_gb = parse_sql(sql_gb).expect("Parse GROUP BY");
        match stmt_gb {
            Statement::Select { group_by, having, .. } => {
                assert_eq!(group_by, Some(vec!["dept".to_string()]));
                assert!(having.is_some());
            }
            _ => panic!("Expected Select with GROUP BY and HAVING"),
        }

        // Test IN list
        let sql_in = "SELECT id FROM items WHERE category IN ('Electronics', 'Books');";
        let stmt_in = parse_sql(sql_in).expect("Parse IN list");
        match stmt_in {
            Statement::Select { where_clause, .. } => {
                match where_clause.unwrap() {
                    WhereExpr::InList { column, values, negated } => {
                        assert_eq!(column, "category");
                        assert_eq!(values.len(), 2);
                        assert!(!negated);
                    }
                    _ => panic!("Expected InList"),
                }
            }
            _ => panic!("Expected Select"),
        }

        // Test IN subquery
        let sql_sub = "SELECT id FROM orders WHERE user_id IN (SELECT id FROM users WHERE active = 1);";
        let stmt_sub = parse_sql(sql_sub).expect("Parse IN subquery");
        match stmt_sub {
            Statement::Select { where_clause, .. } => {
                match where_clause.unwrap() {
                    WhereExpr::InSubquery { column, subquery, negated } => {
                        assert_eq!(column, "user_id");
                        assert!(!negated);
                        assert!(matches!(*subquery, Statement::Select { .. }));
                    }
                    _ => panic!("Expected InSubquery"),
                }
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_parse_transactions_and_index() {
        assert_eq!(parse_sql("BEGIN;").unwrap(), Statement::BeginTransaction);
        assert_eq!(parse_sql("BEGIN TRANSACTION;").unwrap(), Statement::BeginTransaction);
        assert_eq!(parse_sql("COMMIT;").unwrap(), Statement::CommitTransaction);
        assert_eq!(parse_sql("ROLLBACK;").unwrap(), Statement::RollbackTransaction);

        let sql_idx = "CREATE INDEX IF NOT EXISTS idx_name ON users (name);";
        let stmt_idx = parse_sql(sql_idx).expect("Parse CREATE INDEX");
        match stmt_idx {
            Statement::CreateIndex { name, if_not_exists, table, column } => {
                assert_eq!(name, "idx_name");
                assert!(if_not_exists);
                assert_eq!(table, "users");
                assert_eq!(column, "name");
            }
            _ => panic!("Expected CreateIndex"),
        }
    }

    #[test]
    fn test_parse_join_order_by_and_aggregates() {
        let sql = "SELECT users.id, orders.total FROM users INNER JOIN orders ON users.id = orders.user_id ORDER BY orders.total DESC LIMIT 10;";
        let stmt = parse_sql(sql).expect("Parse JOIN and ORDER BY");
        match stmt {
            Statement::Select { columns, table, join, order_by, limit, .. } => {
                assert_eq!(table, "users");
                assert_eq!(columns, vec!["users.id", "orders.total"]);
                assert_eq!(join, Some(JoinClause {
                    table: "orders".to_string(),
                    left_col: "users.id".to_string(),
                    right_col: "orders.user_id".to_string(),
                    is_left: false,
                    join_type: JoinType::Inner,
                }));
                assert_eq!(order_by, Some(OrderByClause {
                    column: "orders.total".to_string(),
                    is_ascending: false,
                }));
                assert_eq!(limit, Some(10));
            }
            _ => panic!("Expected Select with JOIN and ORDER BY"),
        }

        let sql_agg = "SELECT COUNT(*), SUM(price), AVG(price) FROM products;";
        let stmt_agg = parse_sql(sql_agg).expect("Parse Aggregates");
        match stmt_agg {
            Statement::Select { columns, table, .. } => {
                assert_eq!(table, "products");
                assert_eq!(columns, vec!["COUNT(*)", "SUM(price)", "AVG(price)"]);
            }
            _ => panic!("Expected Select with aggregates"),
        }
    }
}
