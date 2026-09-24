//! SQL Parser, Catalog, Record Codec, and Execution Engine.

pub mod catalog;
pub mod codec;
pub mod executor;
pub mod lexer;
pub mod parser;
pub mod planner;
pub mod vectorized;

pub use catalog::{Catalog, ColumnDef, DataType, IndexDef, TableDef, ViewDef};
pub use codec::{decode_row, decode_row_values, encode_row};
pub use executor::{matches_condition, SQLExecutor};
pub use parser::{bind_parameters, parse_sql, parse_tokens, CteClause, Statement};
pub use planner::{CostEstimate, CostOptimizer, PlanType, TableStats};
pub use vectorized::VectorizedAccumulator;

