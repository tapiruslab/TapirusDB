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
    Savepoint,
    Release,
    Rename,
    To,
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
    Returning,
    Replace,
    Conflict,
    Do,
    Nothing,
    Ignore,
    Analyze,
    Pragma,
    Case,
    When,
    Then,
    Else,
    End,
    Cast,

    // Window Function Keywords
    Over,
    Partition,
    RowNumber,
    Rank,
    DenseRank,
    Ntile,
    Lag,
    Lead,
    Rows,
    Unbounded,
    Preceding,
    Following,
    Current,
    Row,

    // Graph Algorithm
    Algorithm,

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
    Plus,
    Slash,
    Percent,
    Concat,
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

        // Concat operator: ||
        if ch == '|' && i + 1 < chars.len() && chars[i + 1] == '|' {
            tokens.push(Token::Concat);
            i += 2;
            continue;
        }

        // Single-character symbols
        match ch {
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
                continue;
            }
            '/' => {
                tokens.push(Token::Slash);
                i += 1;
                continue;
            }
            '%' => {
                tokens.push(Token::Percent);
                i += 1;
                continue;
            }
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
        if ch.is_ascii_digit()
            || (ch == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
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
                let f = num_str
                    .parse::<f64>()
                    .map_err(|_| Error::SqlSyntax(format!("Invalid float literal: {num_str}")))?;
                tokens.push(Token::FloatLit(f));
            } else {
                let val = num_str
                    .parse::<i64>()
                    .map_err(|_| Error::SqlSyntax(format!("Invalid integer literal: {num_str}")))?;
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
                "RETURNING" => Token::Returning,
                "REPLACE" => Token::Replace,
                "CONFLICT" => Token::Conflict,
                "DO" => Token::Do,
                "NOTHING" => Token::Nothing,
                "IGNORE" => Token::Ignore,
                "ANALYZE" => Token::Analyze,
                "PRAGMA" => Token::Pragma,
                "CASE" => Token::Case,
                "WHEN" => Token::When,
                "THEN" => Token::Then,
                "ELSE" => Token::Else,
                "END" => Token::End,
                "CAST" => Token::Cast,
                "SAVEPOINT" => Token::Savepoint,
                "RELEASE" => Token::Release,
                "RENAME" => Token::Rename,
                "TO" => Token::To,
                // Window function keywords
                "OVER" => Token::Over,
                "PARTITION" => Token::Partition,
                "ROW_NUMBER" => Token::RowNumber,
                "RANK" => Token::Rank,
                "DENSE_RANK" => Token::DenseRank,
                "NTILE" => Token::Ntile,
                "LAG" => Token::Lag,
                "LEAD" => Token::Lead,
                "ROWS" => Token::Rows,
                "UNBOUNDED" => Token::Unbounded,
                "PRECEDING" => Token::Preceding,
                "FOLLOWING" => Token::Following,
                "CURRENT" => Token::Current,
                "ROW" => Token::Row,
                // Graph algorithm keyword
                "ALGORITHM" => Token::Algorithm,
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

/// Reconstruct SQL text from a validated token slice (e.g. parenthesized scalar subqueries).
pub(crate) fn tokens_to_sql(tokens: &[Token]) -> String {
    fn tok_str(tok: &Token) -> String {
        match tok {
            Token::Create => "CREATE".to_string(),
            Token::Table => "TABLE".to_string(),
            Token::If => "IF".to_string(),
            Token::Not => "NOT".to_string(),
            Token::Exists => "EXISTS".to_string(),
            Token::Primary => "PRIMARY".to_string(),
            Token::Key => "KEY".to_string(),
            Token::Integer => "INTEGER".to_string(),
            Token::Real => "REAL".to_string(),
            Token::Text => "TEXT".to_string(),
            Token::Blob => "BLOB".to_string(),
            Token::Vector => "VECTOR".to_string(),
            Token::Insert => "INSERT".to_string(),
            Token::Into => "INTO".to_string(),
            Token::Values => "VALUES".to_string(),
            Token::Select => "SELECT".to_string(),
            Token::From => "FROM".to_string(),
            Token::Where => "WHERE".to_string(),
            Token::Limit => "LIMIT".to_string(),
            Token::Offset => "OFFSET".to_string(),
            Token::Near => "NEAR".to_string(),
            Token::Top => "TOP".to_string(),
            Token::Graph => "GRAPH".to_string(),
            Token::Node => "NODE".to_string(),
            Token::Edge => "EDGE".to_string(),
            Token::Match => "MATCH".to_string(),
            Token::Null => "NULL".to_string(),
            Token::Update => "UPDATE".to_string(),
            Token::Set => "SET".to_string(),
            Token::Delete => "DELETE".to_string(),
            Token::Drop => "DROP".to_string(),
            Token::Alter => "ALTER".to_string(),
            Token::Add => "ADD".to_string(),
            Token::Column => "COLUMN".to_string(),
            Token::View => "VIEW".to_string(),
            Token::Begin => "BEGIN".to_string(),
            Token::Commit => "COMMIT".to_string(),
            Token::Rollback => "ROLLBACK".to_string(),
            Token::Transaction => "TRANSACTION".to_string(),
            Token::Savepoint => "SAVEPOINT".to_string(),
            Token::Release => "RELEASE".to_string(),
            Token::Rename => "RENAME".to_string(),
            Token::To => "TO".to_string(),
            Token::Index => "INDEX".to_string(),
            Token::On => "ON".to_string(),
            Token::Join => "JOIN".to_string(),
            Token::Inner => "INNER".to_string(),
            Token::Left => "LEFT".to_string(),
            Token::Right => "RIGHT".to_string(),
            Token::Full => "FULL".to_string(),
            Token::Outer => "OUTER".to_string(),
            Token::Distinct => "DISTINCT".to_string(),
            Token::Order => "ORDER".to_string(),
            Token::By => "BY".to_string(),
            Token::Asc => "ASC".to_string(),
            Token::Desc => "DESC".to_string(),
            Token::Count => "COUNT".to_string(),
            Token::Sum => "SUM".to_string(),
            Token::Avg => "AVG".to_string(),
            Token::Min => "MIN".to_string(),
            Token::Max => "MAX".to_string(),
            Token::Explain => "EXPLAIN".to_string(),
            Token::Plan => "PLAN".to_string(),
            Token::Vacuum => "VACUUM".to_string(),
            Token::Like => "LIKE".to_string(),
            Token::And => "AND".to_string(),
            Token::Or => "OR".to_string(),
            Token::Group => "GROUP".to_string(),
            Token::Having => "HAVING".to_string(),
            Token::In => "IN".to_string(),
            Token::Is => "IS".to_string(),
            Token::Between => "BETWEEN".to_string(),
            Token::As => "AS".to_string(),
            Token::Of => "OF".to_string(),
            Token::Timestamp => "TIMESTAMP".to_string(),
            Token::With => "WITH".to_string(),
            Token::Return => "RETURN".to_string(),
            Token::Returning => "RETURNING".to_string(),
            Token::Replace => "REPLACE".to_string(),
            Token::Conflict => "CONFLICT".to_string(),
            Token::Do => "DO".to_string(),
            Token::Nothing => "NOTHING".to_string(),
            Token::Ignore => "IGNORE".to_string(),
            Token::Analyze => "ANALYZE".to_string(),
            Token::Pragma => "PRAGMA".to_string(),
            Token::Case => "CASE".to_string(),
            Token::When => "WHEN".to_string(),
            Token::Then => "THEN".to_string(),
            Token::Else => "ELSE".to_string(),
            Token::End => "END".to_string(),
            Token::Cast => "CAST".to_string(),
            Token::Over => "OVER".to_string(),
            Token::Partition => "PARTITION".to_string(),
            Token::RowNumber => "ROW_NUMBER".to_string(),
            Token::Rank => "RANK".to_string(),
            Token::DenseRank => "DENSE_RANK".to_string(),
            Token::Ntile => "NTILE".to_string(),
            Token::Lag => "LAG".to_string(),
            Token::Lead => "LEAD".to_string(),
            Token::Rows => "ROWS".to_string(),
            Token::Unbounded => "UNBOUNDED".to_string(),
            Token::Preceding => "PRECEDING".to_string(),
            Token::Following => "FOLLOWING".to_string(),
            Token::Current => "CURRENT".to_string(),
            Token::Row => "ROW".to_string(),
            Token::Algorithm => "ALGORITHM".to_string(),
            Token::Ident(s) => s.clone(),
            Token::StringLit(s) => format!("'{}'", s.replace('\'', "''")),
            Token::IntLit(i) => i.to_string(),
            Token::FloatLit(f) => f.to_string(),
            Token::BlobLit(bytes) => {
                let mut hex = String::with_capacity(bytes.len() * 2 + 3);
                hex.push_str("X'");
                for b in bytes {
                    hex.push_str(&format!("{b:02X}"));
                }
                hex.push('\'');
                hex
            }
            Token::VectorLit(vec) => {
                let items: Vec<String> = vec.iter().map(|v| v.to_string()).collect();
                format!("[{}]", items.join(", "))
            }
            Token::OpenParen => "(".to_string(),
            Token::CloseParen => ")".to_string(),
            Token::OpenBracket => "[".to_string(),
            Token::CloseBracket => "]".to_string(),
            Token::Comma => ",".to_string(),
            Token::Semicolon => ";".to_string(),
            Token::Equals => "=".to_string(),
            Token::NotEquals => "!=".to_string(),
            Token::GreaterThan => ">".to_string(),
            Token::LessThan => "<".to_string(),
            Token::GreaterOrEqual => ">=".to_string(),
            Token::LessOrEqual => "<=".to_string(),
            Token::QuestionMark => "?".to_string(),
            Token::Asterisk => "*".to_string(),
            Token::Dot => ".".to_string(),
            Token::Colon => ":".to_string(),
            Token::Dash => "-".to_string(),
            Token::Arrow => "->".to_string(),
            Token::Plus => "+".to_string(),
            Token::Slash => "/".to_string(),
            Token::Percent => "%".to_string(),
            Token::Concat => "||".to_string(),
        }
    }

    let mut out = String::new();
    let mut prev: Option<&Token> = None;
    for tok in tokens {
        let no_space_before = matches!(
            tok,
            Token::Comma | Token::CloseParen | Token::CloseBracket | Token::Semicolon | Token::Dot
        ) || (matches!(tok, Token::OpenParen)
            && matches!(
                prev,
                Some(
                    Token::Count
                        | Token::Sum
                        | Token::Avg
                        | Token::Min
                        | Token::Max
                        | Token::RowNumber
                        | Token::Rank
                        | Token::DenseRank
                        | Token::Ntile
                        | Token::Lag
                        | Token::Lead
                        | Token::Cast
                        | Token::Ident(_)
                )
            ));
        let no_space_after_prev = matches!(
            prev,
            Some(Token::OpenParen | Token::OpenBracket | Token::Dot)
        );
        if !out.is_empty() && !no_space_before && !no_space_after_prev {
            out.push(' ');
        }
        out.push_str(&tok_str(tok));
        prev = Some(tok);
    }
    out
}
