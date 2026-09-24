//! Cost-Based Query Optimizer (CBO) and Execution Plan Cost Estimator.
//!
//! Evaluates candidate physical execution plans (Sequential Scan, B+Tree Index Scan,
//! Primary Key Point Lookup, HNSW Vector Scan, and Join strategies) using I/O page cost,
//! CPU instruction cost, and table cardinality statistics to pick the optimal execution path.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Statistical profile of a database table used for cost-based optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableStats {
    /// Name of the table
    pub table_name: String,
    /// Total row count in table
    pub total_rows: usize,
    /// Total pages allocated / occupied by the table
    pub page_count: usize,
    /// Estimated cardinality (distinct values count) per column
    pub column_distinct: HashMap<String, usize>,
    /// Null count per column
    pub column_nulls: HashMap<String, usize>,
    /// Average row size in bytes
    pub avg_row_size: usize,
    /// Timestamp of last statistical analysis
    pub last_analyzed_epoch_secs: u64,
}

impl TableStats {
    /// Create a new statistical profile with default assumptions
    pub fn new(table_name: &str, total_rows: usize, page_count: usize) -> Self {
        let avg_size = if total_rows > 0 {
            (page_count * 4096) / total_rows
        } else {
            128
        };
        Self {
            table_name: table_name.to_string(),
            total_rows,
            page_count: page_count.max(1),
            column_distinct: HashMap::new(),
            column_nulls: HashMap::new(),
            avg_row_size: avg_size.max(16),
            last_analyzed_epoch_secs: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }

    /// Estimate predicate selectivity for a given column (fraction of rows matching condition)
    pub fn estimate_selectivity(&self, column: &str, is_equality: bool) -> f64 {
        if self.total_rows == 0 {
            return 1.0;
        }

        if let Some(&distinct) = self.column_distinct.get(column) {
            if distinct > 0 {
                if is_equality {
                    return (1.0 / distinct as f64).min(1.0);
                } else {
                    // Range query default selectivity ~ 33%
                    return 0.33;
                }
            }
        }

        // Default heuristic if no column statistics collected yet
        if is_equality {
            0.1 // 10% default for unindexed equality
        } else {
            0.33
        }
    }
}

/// Physical plan type considered by the Cost-Based Optimizer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanType {
    /// Point lookup directly via Primary Key row_id
    PkLookup,
    /// Secondary B+Tree inverted index scan
    IndexScan,
    /// Sequential Table Scan
    SeqScan,
    /// HNSW graph vector search
    HnswVectorScan,
    /// Brute-force sequential vector scan
    BruteForceVectorScan,
    /// Hash Join between two relations
    HashJoin,
    /// Nested Loop Join
    NestedLoopJoin,
}

/// Cost metrics calculated for an execution plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    /// Cost incurred before the first row can be emitted (e.g. index traversal, sorting)
    pub startup_cost: f64,
    /// Total cumulative cost to emit all candidate rows
    pub total_cost: f64,
    /// Estimated number of rows output by this plan
    pub estimated_rows: usize,
    /// Plan operator description
    pub plan_detail: String,
}

impl std::fmt::Display for CostEstimate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "(cost={:.2}..{:.2} rows={})",
            self.startup_cost, self.total_cost, self.estimated_rows
        )
    }
}

/// Cost-Based Query Optimizer evaluating execution plans
pub struct CostOptimizer {
    /// Standard random page I/O cost (baseline = 1.0)
    pub page_io_cost: f64,
    /// Sequential page I/O cost (often faster due to OS prefetching = 0.8)
    pub seq_page_cost: f64,
    /// CPU cost to evaluate a single WHERE filter expression per row
    pub cpu_eval_cost: f64,
    /// CPU cost to process index entry
    pub index_cpu_cost: f64,
    /// CPU cost to calculate a 32-bit float vector cosine/L2 distance per chunk
    pub vector_chunk_cost: f64,
}

impl Default for CostOptimizer {
    fn default() -> Self {
        Self {
            page_io_cost: 1.0,
            seq_page_cost: 0.8,
            cpu_eval_cost: 0.01,
            index_cpu_cost: 0.005,
            vector_chunk_cost: 0.02,
        }
    }
}

impl CostOptimizer {
    /// Create a new CostOptimizer with default cost parameters
    pub fn new() -> Self {
        Self::default()
    }

    /// Estimate cost of a Sequential Table Scan
    pub fn estimate_seq_scan(&self, stats: &TableStats, selectivity: f64) -> CostEstimate {
        let startup_cost = 0.0;
        let io_cost = (stats.page_count as f64) * self.seq_page_cost;
        let cpu_cost = (stats.total_rows as f64) * self.cpu_eval_cost;
        let total_cost = startup_cost + io_cost + cpu_cost;
        let estimated_rows = ((stats.total_rows as f64) * selectivity).round() as usize;

        CostEstimate {
            startup_cost,
            total_cost,
            estimated_rows: estimated_rows.max(1),
            plan_detail: format!("SeqScan on {} {}", stats.table_name, CostEstimate { startup_cost, total_cost, estimated_rows: estimated_rows.max(1), plan_detail: String::new() }),
        }
    }

    /// Estimate cost of a Primary Key Point Lookup
    pub fn estimate_pk_lookup(&self, stats: &TableStats) -> CostEstimate {
        let tree_depth = (stats.page_count as f64).log2().clamp(1.0, 4.0);
        let startup_cost = tree_depth * self.page_io_cost;
        let total_cost = startup_cost + self.cpu_eval_cost;
        CostEstimate {
            startup_cost,
            total_cost,
            estimated_rows: 1,
            plan_detail: format!("PkLookup on {} (cost={:.2}..{:.2} rows=1)", stats.table_name, startup_cost, total_cost),
        }
    }

    /// Estimate cost of an Index Scan
    pub fn estimate_index_scan(
        &self,
        stats: &TableStats,
        index_name: &str,
        column: &str,
        selectivity: f64,
    ) -> CostEstimate {
        let tree_depth = (stats.page_count as f64).log2().clamp(1.0, 4.0);
        let startup_cost = tree_depth * 0.5; // Index root & internal page lookups
        let estimated_rows = ((stats.total_rows as f64) * selectivity).round().max(1.0) as usize;
        let index_io = (estimated_rows as f64) * self.index_cpu_cost;
        let table_fetch_io = (estimated_rows as f64) * self.page_io_cost;
        let total_cost = startup_cost + index_io + table_fetch_io;

        CostEstimate {
            startup_cost,
            total_cost,
            estimated_rows,
            plan_detail: format!(
                "IndexScan on {} using {} on col '{}' (cost={:.2}..{:.2} rows={})",
                stats.table_name, index_name, column, startup_cost, total_cost, estimated_rows
            ),
        }
    }

    /// Estimate cost of a Vector Search (HNSW vs Brute-Force)
    pub fn estimate_vector_search(
        &self,
        stats: &TableStats,
        top_k: usize,
        dimensions: usize,
        has_hnsw_index: bool,
    ) -> CostEstimate {
        if has_hnsw_index {
            let startup_cost = 0.5;
            let log_n = (stats.total_rows as f64).log2().max(1.0);
            let distance_evals = log_n * 16.0; // ef_search * layers
            let compute_cost = distance_evals * (dimensions as f64 / 8.0) * self.vector_chunk_cost;
            let total_cost = startup_cost + compute_cost;
            CostEstimate {
                startup_cost,
                total_cost,
                estimated_rows: top_k.min(stats.total_rows.max(1)),
                plan_detail: format!(
                    "HnswVectorScan on {} [k={top_k}, d={dimensions}] (cost={:.2}..{:.2} rows={})",
                    stats.table_name, startup_cost, total_cost, top_k.min(stats.total_rows.max(1))
                ),
            }
        } else {
            let startup_cost = 0.0;
            let io_cost = (stats.page_count as f64) * self.seq_page_cost;
            let compute_cost = (stats.total_rows as f64) * (dimensions as f64 / 8.0) * self.vector_chunk_cost;
            let total_cost = startup_cost + io_cost + compute_cost;
            CostEstimate {
                startup_cost,
                total_cost,
                estimated_rows: top_k.min(stats.total_rows.max(1)),
                plan_detail: format!(
                    "BruteForceVectorScan on {} [k={top_k}, d={dimensions}] (cost={:.2}..{:.2} rows={})",
                    stats.table_name, startup_cost, total_cost, top_k.min(stats.total_rows.max(1))
                ),
            }
        }
    }

    /// Choose optimal execution plan between Index Scan and Seq Scan based on cost
    pub fn choose_scan_plan(
        &self,
        stats: &TableStats,
        available_index: Option<(&str, &str)>, // (index_name, column)
        is_pk_candidate: bool,
        selectivity: f64,
    ) -> (PlanType, CostEstimate) {
        if is_pk_candidate {
            let pk_cost = self.estimate_pk_lookup(stats);
            return (PlanType::PkLookup, pk_cost);
        }

        let seq_cost = self.estimate_seq_scan(stats, selectivity);

        if let Some((idx_name, col)) = available_index {
            let idx_cost = self.estimate_index_scan(stats, idx_name, col, selectivity);
            if idx_cost.total_cost < seq_cost.total_cost {
                return (PlanType::IndexScan, idx_cost);
            }
        }

        (PlanType::SeqScan, seq_cost)
    }
}
