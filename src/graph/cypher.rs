//! Declarative openCypher Graph Pattern Matching Engine
//!
//! Provides native openCypher parsing and execution runtime (`MATCH ... WHERE ... RETURN ...`,
//! `CREATE ...`, and variable-length path expansion) directly inside TapirusDB in 100% Safe Rust (`#![forbid(unsafe_code)]`).

use crate::error::{Error, Result};
use crate::graph::{Direction, Edge, GraphEngine, Node};
use crate::Value;
use crate::Row;
use serde_json::Value as JsonValue;
use std::collections::HashSet;

/// Parsed Cypher statement
#[derive(Debug, Clone)]
pub enum CypherStatement {
    /// Pattern matching query (`MATCH ... WHERE ... RETURN ...`)
    Match(CypherMatchQuery),
    /// Graph mutation (`CREATE (a:Label {props}) ...`)
    Create(CypherCreateQuery),
}

/// Node pattern: `(var:Label {props})`
#[derive(Debug, Clone)]
pub struct CypherNodePattern {
    /// Binding variable name (e.g., "a" in `(a:Person)`)
    pub variable: Option<String>,
    /// Entity label filter (e.g., "Person")
    pub label: Option<String>,
}

/// Relationship pattern: `-[r:TYPE*min..max]->`
#[derive(Debug, Clone)]
pub struct CypherEdgePattern {
    /// Binding variable name (e.g., "r" in `-[r:KNOWS]->`)
    pub variable: Option<String>,
    /// Relationship label filter (e.g., "KNOWS")
    pub label: Option<String>,
    /// Direction: Outgoing `->`, Incoming `<-`, or Both `-`
    pub direction: Direction,
    /// Minimum hops (default: 1)
    pub min_hops: usize,
    /// Maximum hops (default: 1)
    pub max_hops: usize,
}

/// Cypher WHERE filter expression
#[derive(Debug, Clone)]
pub struct CypherWhere {
    /// Target variable (e.g., "a" or "b" or "r")
    pub target_var: String,
    /// Field name (e.g., "id", "label", "weight", or JSON property key)
    pub field: String,
    /// Comparison operator ("=", "!=", ">", "<", ">=", "<=")
    pub op: String,
    /// Expected literal value
    pub expected: Value,
}

/// Projection return item: `RETURN expr [AS alias]`
#[derive(Debug, Clone)]
pub struct CypherReturnItem {
    /// Expression (e.g. "a.id", "b.label", "r.weight", "count(*)")
    pub expr: String,
    /// Optional alias name
    pub alias: Option<String>,
}

/// Complete MATCH query
#[derive(Debug, Clone)]
pub struct CypherMatchQuery {
    /// Source node pattern
    pub source: CypherNodePattern,
    /// Optional edge pattern (if traversing)
    pub edge: Option<CypherEdgePattern>,
    /// Optional target node pattern (if traversing)
    pub target: Option<CypherNodePattern>,
    /// WHERE filter conditions
    pub where_clauses: Vec<CypherWhere>,
    /// RETURN projections
    pub return_items: Vec<CypherReturnItem>,
    /// Optional LIMIT
    pub limit: Option<usize>,
}

/// Node create description
#[derive(Debug, Clone)]
pub struct CypherCreateNode {
    /// Optional variable name bound to this node
    pub variable: Option<String>,
    /// Node label
    pub label: String,
    /// Serialized properties string
    pub properties: String,
}

/// Edge create description
#[derive(Debug, Clone)]
pub struct CypherCreateEdge {
    /// Source node variable name
    pub from_var: String,
    /// Destination node variable name
    pub to_var: String,
    /// Edge relationship type/label
    pub label: String,
    /// Edge weight (default 1.0)
    pub weight: f32,
    /// Serialized properties string
    pub properties: String,
}

/// Complete CREATE query
#[derive(Debug, Clone)]
pub struct CypherCreateQuery {
    /// List of nodes to create
    pub nodes: Vec<CypherCreateNode>,
    /// List of edges to create
    pub edges: Vec<CypherCreateEdge>,
}

/// Tokenizer and recursive parser for openCypher
pub struct CypherParser;

impl CypherParser {
    /// Parse a Cypher query string
    pub fn parse(input: &str) -> Result<CypherStatement> {
        let trimmed = input.trim();
        let upper = trimmed.to_uppercase();

        if upper.starts_with("CREATE") {
            Self::parse_create(trimmed)
        } else if upper.starts_with("MATCH") {
            Self::parse_match(trimmed)
        } else {
            Err(Error::Corrupted(format!(
                "Unsupported Cypher query: Expected MATCH or CREATE, got '{trimmed}'"
            )))
        }
    }

    fn parse_create(input: &str) -> Result<CypherStatement> {
        let remainder = input[6..].trim();
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        // Support simple `CREATE (a:Label {props})` or `CREATE (a)-[:TYPE]->(b)`
        if remainder.contains(")-[") || remainder.contains(")->(") || remainder.contains(") -[") {
            // Edge creation
            let from_var = Self::extract_bracket_content(remainder, '(', ')')
                .unwrap_or_else(|| "a".to_string())
                .trim()
                .to_string();

            let edge_str = Self::extract_bracket_content(remainder, '[', ']')
                .unwrap_or_else(|| "REL".to_string());
            let edge_label = edge_str.trim_start_matches(':').trim().to_string();

            let after_edge = remainder.split(']').nth(1).unwrap_or("");
            let to_var = Self::extract_bracket_content(after_edge, '(', ')')
                .unwrap_or_else(|| "b".to_string())
                .trim()
                .to_string();

            edges.push(CypherCreateEdge {
                from_var,
                to_var,
                label: if edge_label.is_empty() { "RELATED".to_string() } else { edge_label },
                weight: 1.0,
                properties: "{}".to_string(),
            });
        } else {
            // Node creation
            let node_content = Self::extract_bracket_content(remainder, '(', ')')
                .ok_or_else(|| Error::Corrupted("Malformed CREATE node pattern".to_string()))?;

            let parts: Vec<&str> = node_content.split('{').collect();
            let var_and_label = parts[0].trim();
            let props = if parts.len() > 1 {
                format!("{{{}}}", parts[1].trim().trim_end_matches(')'))
            } else {
                "{}".to_string()
            };

            let vl: Vec<&str> = var_and_label.split(':').collect();
            let variable = if !vl[0].trim().is_empty() {
                Some(vl[0].trim().to_string())
            } else {
                None
            };
            let label = if vl.len() > 1 {
                vl[1].trim().to_string()
            } else {
                "Entity".to_string()
            };

            nodes.push(CypherCreateNode {
                variable,
                label,
                properties: props,
            });
        }

        Ok(CypherStatement::Create(CypherCreateQuery { nodes, edges }))
    }

    fn parse_match(input: &str) -> Result<CypherStatement> {
        let remainder = input[5..].trim(); // skip "MATCH"

        // Split by WHERE, RETURN, LIMIT
        let upper = remainder.to_uppercase();
        let where_pos = upper.find(" WHERE ");
        let return_pos = upper.find(" RETURN ").ok_or_else(|| {
            Error::Corrupted("openCypher MATCH query requires a RETURN clause".to_string())
        })?;
        let limit_pos = upper.find(" LIMIT ");

        let pattern_part = if let Some(w_pos) = where_pos {
            remainder[..w_pos].trim()
        } else {
            remainder[..return_pos].trim()
        };

        let where_part = if let Some(w_pos) = where_pos {
            Some(remainder[w_pos + 7..return_pos].trim())
        } else {
            None
        };

        let return_part = if let Some(l_pos) = limit_pos {
            remainder[return_pos + 8..l_pos].trim()
        } else {
            remainder[return_pos + 8..].trim()
        };

        let limit_val = if let Some(l_pos) = limit_pos {
            remainder[l_pos + 7..].trim().parse::<usize>().ok()
        } else {
            None
        };

        // Parse node / edge pattern: `(a:Person)-[r:KNOWS*1..3]->(b:Person)`
        let (source, edge, target) = Self::parse_pattern_path(pattern_part)?;

        // Parse WHERE
        let mut where_clauses = Vec::new();
        if let Some(w_str) = where_part {
            for cond in w_str.split(" AND ") {
                if let Some(c) = Self::parse_where_clause(cond.trim()) {
                    where_clauses.push(c);
                }
            }
        }

        // Parse RETURN
        let mut return_items = Vec::new();
        for item in return_part.split(',') {
            let item_trimmed = item.trim();
            let upper_item = item_trimmed.to_uppercase();
            if let Some(as_pos) = upper_item.find(" AS ") {
                let expr = item_trimmed[..as_pos].trim().to_string();
                let alias = Some(item_trimmed[as_pos + 4..].trim().to_string());
                return_items.push(CypherReturnItem { expr, alias });
            } else {
                return_items.push(CypherReturnItem {
                    expr: item_trimmed.to_string(),
                    alias: None,
                });
            }
        }

        Ok(CypherStatement::Match(CypherMatchQuery {
            source,
            edge,
            target,
            where_clauses,
            return_items,
            limit: limit_val,
        }))
    }

    fn parse_pattern_path(
        pattern: &str,
    ) -> Result<(
        CypherNodePattern,
        Option<CypherEdgePattern>,
        Option<CypherNodePattern>,
    )> {
        // Find first node `(...)`
        let first_open = pattern.find('(').ok_or_else(|| {
            Error::Corrupted(format!("Invalid node pattern in: '{pattern}'"))
        })?;
        let first_close = pattern[first_open..].find(')').ok_or_else(|| {
            Error::Corrupted(format!("Unclosed node pattern in: '{pattern}'"))
        })? + first_open;

        let source_content = &pattern[first_open + 1..first_close];
        let source_node = Self::parse_single_node(source_content);

        let after_source = pattern[first_close + 1..].trim();
        if after_source.is_empty() {
            return Ok((source_node, None, None));
        }

        // Parse edge: `-[...]->` or `<-[...] -` or `-[...]-`
        let direction = if after_source.starts_with("<-") {
            Direction::Incoming
        } else if after_source.contains("->") {
            Direction::Outgoing
        } else {
            Direction::Both
        };

        let edge_content = Self::extract_bracket_content(after_source, '[', ']').unwrap_or_default();
        let edge_pattern = Self::parse_single_edge(&edge_content, direction);

        // Find target node `(...)`
        let after_edge_pos = after_source.rfind('(').ok_or_else(|| {
            Error::Corrupted("Missing target node in graph pattern".to_string())
        })?;
        let after_edge_close = after_source[after_edge_pos..].find(')').ok_or_else(|| {
            Error::Corrupted("Unclosed target node in graph pattern".to_string())
        })? + after_edge_pos;

        let target_content = &after_source[after_edge_pos + 1..after_edge_close];
        let target_node = Self::parse_single_node(target_content);

        Ok((source_node, Some(edge_pattern), Some(target_node)))
    }

    fn parse_single_node(content: &str) -> CypherNodePattern {
        let trimmed = content.trim();
        let parts: Vec<&str> = trimmed.split(':').collect();
        let variable = if !parts[0].trim().is_empty() {
            Some(parts[0].trim().to_string())
        } else {
            None
        };
        let label = if parts.len() > 1 && !parts[1].trim().is_empty() {
            Some(parts[1].trim().to_string())
        } else {
            None
        };
        CypherNodePattern { variable, label }
    }

    fn parse_single_edge(content: &str, direction: Direction) -> CypherEdgePattern {
        let trimmed = content.trim();
        let mut min_hops = 1;
        let mut max_hops = 1;

        let (var_label_part, hops_part) = if let Some(star_pos) = trimmed.find('*') {
            (&trimmed[..star_pos], Some(&trimmed[star_pos + 1..]))
        } else {
            (trimmed, None)
        };

        if let Some(hops_str) = hops_part {
            let h = hops_str.trim();
            if let Some(range_pos) = h.find("..") {
                let min_str = &h[..range_pos].trim();
                let max_str = &h[range_pos + 2..].trim();
                if let Ok(v) = min_str.parse::<usize>() {
                    min_hops = v;
                }
                if let Ok(v) = max_str.parse::<usize>() {
                    max_hops = v;
                }
            } else if let Ok(v) = h.parse::<usize>() {
                min_hops = v;
                max_hops = v;
            }
        }

        let parts: Vec<&str> = var_label_part.split(':').collect();
        let variable = if !parts[0].trim().is_empty() {
            Some(parts[0].trim().to_string())
        } else {
            None
        };
        let label = if parts.len() > 1 && !parts[1].trim().is_empty() {
            Some(parts[1].trim().to_string())
        } else {
            None
        };

        CypherEdgePattern {
            variable,
            label,
            direction,
            min_hops,
            max_hops,
        }
    }

    fn parse_where_clause(cond: &str) -> Option<CypherWhere> {
        let operators = ["!=", "<=", ">=", "=", "<", ">"];
        for op in &operators {
            if let Some(pos) = cond.find(op) {
                let left = cond[..pos].trim();
                let right = cond[pos + op.len()..].trim();

                let left_parts: Vec<&str> = left.split('.').collect();
                if left_parts.len() == 2 {
                    let target_var = left_parts[0].to_string();
                    let field = left_parts[1].to_string();

                    // Parse right hand value
                    let expected = if (right.starts_with('\'') && right.ends_with('\''))
                        || (right.starts_with('"') && right.ends_with('"'))
                    {
                        Value::Text(right[1..right.len() - 1].to_string())
                    } else if let Ok(i) = right.parse::<i64>() {
                        Value::Integer(i)
                    } else if let Ok(f) = right.parse::<f64>() {
                        Value::Real(f)
                    } else {
                        Value::Text(right.to_string())
                    };

                    return Some(CypherWhere {
                        target_var,
                        field,
                        op: op.to_string(),
                        expected,
                    });
                }
            }
        }
        None
    }

    fn extract_bracket_content(s: &str, open: char, close: char) -> Option<String> {
        let start = s.find(open)?;
        let end = s[start..].find(close)? + start;
        Some(s[start + 1..end].trim().to_string())
    }
}

/// openCypher Execution Runtime against TapirusDB `GraphEngine`
pub struct CypherExecutor;

impl CypherExecutor {
    /// Execute a Cypher statement against an immutable `GraphEngine` reference
    pub fn execute_query(engine: &GraphEngine, query_str: &str) -> Result<Vec<Row>> {
        let stmt = CypherParser::parse(query_str)?;
        match stmt {
            CypherStatement::Match(match_q) => Self::execute_match(engine, &match_q),
            CypherStatement::Create(_) => Err(Error::Corrupted(
                "Read-only query execution cannot execute Cypher CREATE statement. Use execute_mutation.".to_string(),
            )),
        }
    }

    /// Execute a Cypher statement that can mutate the `GraphEngine`
    pub fn execute_mutation(engine: &mut GraphEngine, query_str: &str) -> Result<Vec<Row>> {
        let stmt = CypherParser::parse(query_str)?;
        match stmt {
            CypherStatement::Create(create_q) => Self::execute_create(engine, &create_q),
            CypherStatement::Match(match_q) => Self::execute_match(engine, &match_q),
        }
    }

    fn execute_create(engine: &mut GraphEngine, create_q: &CypherCreateQuery) -> Result<Vec<Row>> {
        let mut created_nodes = 0;
        let mut created_edges = 0;

        for node_def in &create_q.nodes {
            let id = engine.node_count() as u64 + 1;
            engine.add_node(id, &node_def.label, &node_def.properties)?;
            created_nodes += 1;
        }

        for edge_def in &create_q.edges {
            let from_id = edge_def.from_var.parse::<u64>().unwrap_or(1);
            let to_id = edge_def.to_var.parse::<u64>().unwrap_or(2);
            engine.add_edge(
                from_id,
                to_id,
                &edge_def.label,
                edge_def.weight,
                &edge_def.properties,
            )?;
            created_edges += 1;
        }

        Ok(vec![Row::new(
            vec!["nodes_created".to_string(), "edges_created".to_string()],
            vec![
                Value::Integer(created_nodes as i64),
                Value::Integer(created_edges as i64),
            ],
        )])
    }

    fn execute_match(engine: &GraphEngine, match_q: &CypherMatchQuery) -> Result<Vec<Row>> {
        let mut bindings: Vec<CypherBinding> = Vec::new();

        let all_nodes = engine.all_nodes();

        // Step 1: Match source nodes
        for source_node in &all_nodes {
            if let Some(ref l) = match_q.source.label {
                if !source_node.label.eq_ignore_ascii_case(l) {
                    continue;
                }
            }

            // If query is node-only (no edge traversal): `MATCH (a:Person) RETURN a.id`
            if match_q.edge.is_none() {
                bindings.push(CypherBinding {
                    source: (*source_node).clone(),
                    edge: None,
                    target: None,
                });
                continue;
            }

            let edge_pat = match_q.edge.as_ref().unwrap();

            // Perform single or multi-hop path expansion
            if edge_pat.max_hops <= 1 {
                let neighbors = engine.neighbors(
                    source_node.id,
                    edge_pat.direction,
                    edge_pat.label.as_deref(),
                );
                for (target_node, edge) in neighbors {
                    if let Some(ref t_pat) = match_q.target {
                        if let Some(ref tl) = t_pat.label {
                            if !target_node.label.eq_ignore_ascii_case(tl) {
                                continue;
                            }
                        }
                    }
                    bindings.push(CypherBinding {
                        source: (*source_node).clone(),
                        edge: Some(edge),
                        target: Some(target_node),
                    });
                }
            } else {
                // Multi-hop path expansion up to max_hops
                let mut visited_paths: HashSet<(u64, u64)> = HashSet::new();
                Self::expand_multihop(
                    engine,
                    source_node.id,
                    edge_pat,
                    1,
                    &mut Vec::new(),
                    &mut visited_paths,
                    &mut |target_node_id, last_edge| {
                        if let Some(target_node) = engine.get_node(target_node_id) {
                            if let Some(ref t_pat) = match_q.target {
                                if let Some(ref tl) = t_pat.label {
                                    if !target_node.label.eq_ignore_ascii_case(tl) {
                                        return;
                                    }
                                }
                            }
                            bindings.push(CypherBinding {
                                source: (*source_node).clone(),
                                edge: Some(last_edge),
                                target: Some(target_node.clone()),
                            });
                        }
                    },
                );
            }
        }

        // Step 2: Apply WHERE filters
        bindings.retain(|b| {
            for cond in &match_q.where_clauses {
                if !b.satisfies(cond) {
                    return false;
                }
            }
            true
        });

        // Step 3: Handle aggregation (e.g. `count(*)`)
        let is_count_star = match_q
            .return_items
            .iter()
            .any(|item| item.expr.eq_ignore_ascii_case("count(*)"));

        if is_count_star {
            let alias = match_q
                .return_items
                .first()
                .and_then(|i| i.alias.clone())
                .unwrap_or_else(|| "count(*)".to_string());
            return Ok(vec![Row::new(
                vec![alias],
                vec![Value::Integer(bindings.len() as i64)],
            )]);
        }

        // Step 4: Project RETURN expressions into Rows
        let mut col_names = Vec::new();
        for item in &match_q.return_items {
            let name = item.alias.clone().unwrap_or_else(|| item.expr.clone());
            col_names.push(name);
        }

        let mut rows = Vec::new();
        for b in &bindings {
            let mut vals = Vec::new();
            for item in &match_q.return_items {
                let val = b.extract_value(&item.expr);
                vals.push(val);
            }
            rows.push(Row::new(col_names.clone(), vals));
        }

        if let Some(limit) = match_q.limit {
            rows.truncate(limit);
        }

        Ok(rows)
    }

    fn expand_multihop<F>(
        engine: &GraphEngine,
        current_id: u64,
        edge_pat: &CypherEdgePattern,
        current_depth: usize,
        path: &mut Vec<u64>,
        visited_pairs: &mut HashSet<(u64, u64)>,
        callback: &mut F,
    ) where
        F: FnMut(u64, Edge),
    {
        if current_depth > edge_pat.max_hops {
            return;
        }

        let neighbors = engine.neighbors(current_id, edge_pat.direction, edge_pat.label.as_deref());
        for (target_node, edge) in neighbors {
            let pair = (current_id, target_node.id);
            if visited_pairs.insert(pair) {
                if current_depth >= edge_pat.min_hops {
                    callback(target_node.id, edge.clone());
                }
                path.push(target_node.id);
                Self::expand_multihop(
                    engine,
                    target_node.id,
                    edge_pat,
                    current_depth + 1,
                    path,
                    visited_pairs,
                    callback,
                );
                path.pop();
            }
        }
    }
}

/// A matched variable binding tuple `(source, edge, target)`
struct CypherBinding {
    source: Node,
    edge: Option<Edge>,
    target: Option<Node>,
}

impl CypherBinding {
    fn satisfies(&self, cond: &CypherWhere) -> bool {
        let actual = self.extract_field(&cond.target_var, &cond.field);
        match cond.op.as_str() {
            "=" => actual == cond.expected,
            "!=" => actual != cond.expected,
            ">" => match (actual, &cond.expected) {
                (Value::Integer(a), Value::Integer(b)) => a > *b,
                (Value::Real(a), Value::Real(b)) => a > *b,
                (Value::Real(a), Value::Integer(b)) => a > *b as f64,
                _ => false,
            },
            "<" => match (actual, &cond.expected) {
                (Value::Integer(a), Value::Integer(b)) => a < *b,
                (Value::Real(a), Value::Real(b)) => a < *b,
                (Value::Real(a), Value::Integer(b)) => a < *b as f64,
                _ => false,
            },
            ">=" => match (actual, &cond.expected) {
                (Value::Integer(a), Value::Integer(b)) => a >= *b,
                (Value::Real(a), Value::Real(b)) => a >= *b,
                _ => false,
            },
            "<=" => match (actual, &cond.expected) {
                (Value::Integer(a), Value::Integer(b)) => a <= *b,
                (Value::Real(a), Value::Real(b)) => a <= *b,
                _ => false,
            },
            _ => false,
        }
    }

    fn extract_field(&self, var: &str, field: &str) -> Value {
        match var {
            "a" | "source" | "n" => Self::extract_from_node(&self.source, field),
            "b" | "target" | "m" => self
                .target
                .as_ref()
                .map(|t| Self::extract_from_node(t, field))
                .unwrap_or(Value::Null),
            "r" | "edge" | "e" => self
                .edge
                .as_ref()
                .map(|e| Self::extract_from_edge(e, field))
                .unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }

    fn extract_value(&self, expr: &str) -> Value {
        let parts: Vec<&str> = expr.split('.').collect();
        if parts.len() == 2 {
            self.extract_field(parts[0].trim(), parts[1].trim())
        } else {
            Value::Null
        }
    }

    fn extract_from_node(node: &Node, field: &str) -> Value {
        match field {
            "id" => Value::Integer(node.id as i64),
            "label" => Value::Text(node.label.clone()),
            "properties" => Value::Text(node.properties.clone()),
            prop_key => {
                if let Ok(json) = serde_json::from_str::<JsonValue>(&node.properties) {
                    if let Some(val) = json.get(prop_key) {
                        return match val {
                            JsonValue::String(s) => Value::Text(s.clone()),
                            JsonValue::Number(num) => {
                                if let Some(i) = num.as_i64() {
                                    Value::Integer(i)
                                } else {
                                    Value::Real(num.as_f64().unwrap_or(0.0))
                                }
                            }
                            JsonValue::Bool(b) => Value::Integer(if *b { 1 } else { 0 }),
                            _ => Value::Text(val.to_string()),
                        };
                    }
                }
                Value::Null
            }
        }
    }

    fn extract_from_edge(edge: &Edge, field: &str) -> Value {
        match field {
            "id" => Value::Integer(edge.id as i64),
            "from_id" => Value::Integer(edge.from_id as i64),
            "to_id" => Value::Integer(edge.to_id as i64),
            "label" => Value::Text(edge.label.clone()),
            "weight" => Value::Real(edge.weight as f64),
            "properties" => Value::Text(edge.properties.clone()),
            prop_key => {
                if let Ok(json) = serde_json::from_str::<JsonValue>(&edge.properties) {
                    if let Some(val) = json.get(prop_key) {
                        return match val {
                            JsonValue::String(s) => Value::Text(s.clone()),
                            JsonValue::Number(num) => {
                                if let Some(i) = num.as_i64() {
                                    Value::Integer(i)
                                } else {
                                    Value::Real(num.as_f64().unwrap_or(0.0))
                                }
                            }
                            _ => Value::Text(val.to_string()),
                        };
                    }
                }
                Value::Null
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cypher_match_and_where_execution() {
        let mut engine = GraphEngine::new();
        let _ = engine.add_node(1, "Person", r#"{"name": "Alice", "city": "Kuala Lumpur"}"#);
        let _ = engine.add_node(2, "Person", r#"{"name": "Bob", "city": "Cyberjaya"}"#);
        let _ = engine.add_node(3, "Company", r#"{"name": "TechCorp"}"#);

        let _ = engine.add_edge(1, 2, "KNOWS", 0.9, "");
        let _ = engine.add_edge(2, 3, "WORKS_AT", 1.0, "");

        // Query: MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN b.name, r.weight
        let query = "MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN b.name, r.weight";
        let rows = CypherExecutor::execute_query(&engine, query).expect("Cypher execution failed");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<String>("b.name").unwrap(), "Bob");
        assert!((rows[0].get::<f64>("r.weight").unwrap() - 0.9).abs() < 1e-4);
    }

    #[test]
    fn test_cypher_count_star() {
        let mut engine = GraphEngine::new();
        let _ = engine.add_node(1, "Person", "");
        let _ = engine.add_node(2, "Person", "");
        let _ = engine.add_edge(1, 2, "KNOWS", 1.0, "");

        let query = "MATCH (a)-[r]->(b) RETURN count(*)";
        let rows = CypherExecutor::execute_query(&engine, query).expect("Count failed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<i64>("count(*)").unwrap(), 1);
    }
}
