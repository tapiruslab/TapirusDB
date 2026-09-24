//! SQL Tokenizer and Lexer for TapirusDB.

use crate::error::{Error, Result};

/// Tokens produced by the SQL Lexer
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub enum Token {
    // Keywords
    Create,
    Table,
    If,
    Not,
    Exists,
    Primary,
    Key,
    Integer,
    Real,
    Text,
    Blob,
    Vector,
    Insert,
    Into,
    Values,
    Select,
    From,
    Where,
    Limit,
    Offset,
    Near,
    Top,
    Graph,
    Node,
    Edge,
    Match,
    Null,
    Update,
    Set,
    Delete,
    Drop,
    Alter,
    Add,
    Column,
    View,
    Begin,
    Commit,
    Rollback,
    Transaction,
    Index,
    On,
    Join,
    Inner,
    Left,
    Right,
    Full,
    Outer,
    Distinct,
    Order,
    By,
    Asc,
    Desc,
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Explain,
    Plan,
    Vacuum,
    Like,
    And,
    Or,
    Group,
    Having,
    In,
    Is,
    Between,
    As,
    Of,
    Timestamp,
    With,
    Return,
    Analyze,

    // Literals & Identifiers
    Ident(String),
    StringLit(String),
    IntLit(i64),
    FloatLit(f64),
    BlobLit(Vec<u8>),
    VectorLit(Vec<f32>),

    // Symbols & Punctuation
    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    Comma,
    Semicolon,
    Equals,
    NotEquals,
    GreaterThan,
    LessThan,
    GreaterOrEqual,
    LessOrEqual,
    QuestionMark,
    Asterisk,
    Dot,
    Colon,
    Dash,
    Arrow,
}

/// Tokenize a raw SQL query string into a vector of Tokens
pub fn tokenize(input: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        // Whitespace
        if ch.is_whitespace() {
            i += 1;
            continue;
        }

        // Single-line comment: -- comment...
        if ch == '-' && i + 1 < chars.len() && chars[i + 1] == '-' {
            i += 2;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        // Arrow: ->
        if ch == '-' && i + 1 < chars.len() && chars[i + 1] == '>' {
            tokens.push(Token::Arrow);
            i += 2;
            continue;
        }

        // Dash: - (when not part of a negative number)
        if ch == '-' && !(i + 1 < chars.len() && chars[i + 1].is_ascii_digit()) {
            tokens.push(Token::Dash);
            i += 1;
            continue;
        }

        // Multi-line block comment: /* comment... */
        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            if i + 1 < chars.len() {
                i += 2;
            } else {
                i = chars.len();
            }
            continue;
        }

        // Multi-character comparison symbols: !=, <>, <=, >=, <, >
        if ch == '!' && i + 1 < chars.len() && chars[i + 1] == '=' {
            tokens.push(Token::NotEquals);
            i += 2;
            continue;
        }
        if ch == '<' {
            if i + 1 < chars.len() && chars[i + 1] == '>' {
                tokens.push(Token::NotEquals);
                i += 2;
                continue;
            }
            if i + 1 < chars.len() && chars[i + 1] == '=' {
                tokens.push(Token::LessOrEqual);
                i += 2;
                continue;
            }
            tokens.push(Token::LessThan);
            i += 1;
            continue;
        }
        if ch == '>' {
            if i + 1 < chars.len() && chars[i + 1] == '=' {
                tokens.push(Token::GreaterOrEqual);
                i += 2;
                continue;
            }
            tokens.push(Token::GreaterThan);
            i += 1;
            continue;
        }

        // Single-character symbols
        match ch {
            '(' => {
                tokens.push(Token::OpenParen);
                i += 1;
                continue;
            }
            ')' => {
                tokens.push(Token::CloseParen);
                i += 1;
                continue;
            }
            '[' => {
                tokens.push(Token::OpenBracket);
                i += 1;
                continue;
            }
            ']' => {
                tokens.push(Token::CloseBracket);
                i += 1;
                continue;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
                continue;
            }
            ';' => {
                tokens.push(Token::Semicolon);
                i += 1;
                continue;
            }
            '=' => {
                tokens.push(Token::Equals);
                i += 1;
                continue;
            }
            '?' => {
                tokens.push(Token::QuestionMark);
                i += 1;
                continue;
            }
            '*' => {
                tokens.push(Token::Asterisk);
                i += 1;
                continue;
            }
            '.' => {
                tokens.push(Token::Dot);
                i += 1;
                continue;
            }
            ':' => {
                tokens.push(Token::Colon);
                i += 1;
                continue;
            }
            _ => {}
        }

        // String literals: 'example' or "example"
        if ch == '\'' || ch == '"' {
            let quote = ch;
            i += 1;
            let mut s = String::new();
            while i < chars.len() {
                if chars[i] == quote {
                    if i + 1 < chars.len() && chars[i + 1] == quote {
                        s.push(quote);
                        i += 2;
                        continue;
                    } else {
                        break;
                    }
                }
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    match chars[i] {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        '0' => s.push('\0'),
                        '\\' => s.push('\\'),
                        c if c == quote => s.push(quote),
                        other => {
                            s.push('\\');
                            s.push(other);
                        }
                    }
                } else {
                    s.push(chars[i]);
                }
                i += 1;
            }
            if i >= chars.len() {
                return Err(Error::SqlSyntax("Unterminated string literal".into()));
            }
            i += 1; // Consume closing quote
            tokens.push(Token::StringLit(s));
            continue;
        }

        // Numbers (Integer or Float, including negative)
        if ch.is_ascii_digit() || (ch == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit()) {
            let start = i;
            if ch == '-' {
                i += 1;
            }
            let mut is_float = false;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                if chars[i] == '.' {
                    is_float = true;
                }
                i += 1;
            }
            let num_str: String = chars[start..i].iter().collect();
            if is_float {
                let f = num_str.parse::<f64>().map_err(|_| {
                    Error::SqlSyntax(format!("Invalid float literal: {num_str}"))
                })?;
                tokens.push(Token::FloatLit(f));
            } else {
                let val = num_str.parse::<i64>().map_err(|_| {
                    Error::SqlSyntax(format!("Invalid integer literal: {num_str}"))
                })?;
                tokens.push(Token::IntLit(val));
            }
            continue;
        }

        // Identifiers and Keywords
        if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let upper = word.to_uppercase();

            let tok = match upper.as_str() {
                "CREATE" => Token::Create,
                "TABLE" => Token::Table,
                "IF" => Token::If,
                "NOT" => Token::Not,
                "EXISTS" => Token::Exists,
                "PRIMARY" => Token::Primary,
                "KEY" => Token::Key,
                "INTEGER" | "INT" => Token::Integer,
                "REAL" | "FLOAT" | "DOUBLE" => Token::Real,
                "TEXT" | "VARCHAR" | "STRING" => Token::Text,
                "BLOB" => Token::Blob,
                "VECTOR" => Token::Vector,
                "INSERT" => Token::Insert,
                "INTO" => Token::Into,
                "VALUES" => Token::Values,
                "SELECT" => Token::Select,
                "FROM" => Token::From,
                "WHERE" => Token::Where,
                "LIMIT" => Token::Limit,
                "OFFSET" => Token::Offset,
                "NEAR" => Token::Near,
                "TOP" => Token::Top,
                "GRAPH" => Token::Graph,
                "NODE" => Token::Node,
                "EDGE" => Token::Edge,
                "MATCH" => Token::Match,
                "NULL" => Token::Null,
                "UPDATE" => Token::Update,
                "SET" => Token::Set,
                "DELETE" => Token::Delete,
                "DROP" => Token::Drop,
                "ALTER" => Token::Alter,
                "ADD" => Token::Add,
                "COLUMN" => Token::Column,
                "VIEW" => Token::View,
                "BEGIN" => Token::Begin,
                "COMMIT" => Token::Commit,
                "ROLLBACK" => Token::Rollback,
                "TRANSACTION" => Token::Transaction,
                "INDEX" => Token::Index,
                "ON" => Token::On,
                "JOIN" => Token::Join,
                "INNER" => Token::Inner,
                "LEFT" => Token::Left,
                "RIGHT" => Token::Right,
                "FULL" => Token::Full,
                "OUTER" => Token::Outer,
                "DISTINCT" => Token::Distinct,
                "ORDER" => Token::Order,
                "BY" => Token::By,
                "ASC" => Token::Asc,
                "DESC" => Token::Desc,
                "COUNT" => Token::Count,
                "SUM" => Token::Sum,
                "AVG" => Token::Avg,
                "MIN" => Token::Min,
                "MAX" => Token::Max,
                "EXPLAIN" => Token::Explain,
                "PLAN" => Token::Plan,
                "VACUUM" => Token::Vacuum,
                "LIKE" => Token::Like,
                "AND" => Token::And,
                "OR" => Token::Or,
                "GROUP" => Token::Group,
                "HAVING" => Token::Having,
                "IN" => Token::In,
                "IS" => Token::Is,
                "BETWEEN" => Token::Between,
                "AS" => Token::As,
                "OF" => Token::Of,
                "TIMESTAMP" => Token::Timestamp,
                "WITH" => Token::With,
                "RETURN" => Token::Return,
                "ANALYZE" => Token::Analyze,
                _ => Token::Ident(word),
            };
            tokens.push(tok);
            continue;
        }

        return Err(Error::SqlSyntax(format!(
            "Unexpected character in SQL input: '{ch}'"
        )));
    }

    Ok(tokens)
}
