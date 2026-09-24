//! Embedded Real-Time Reactive Query Subscriptions and Change Data Capture (CDC).
//!
//! Provides an ultra-lightweight, in-memory event bus in 100% Pure Safe Rust,
//! allowing application UIs and background tasks to react instantly to table mutations
//! without continuous polling.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Type of database mutation operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeOp {
    /// Row inserted
    Insert,
    /// Row updated
    Update,
    /// Row deleted
    Delete,
}

impl std::fmt::Display for ChangeOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeOp::Insert => write!(f, "INSERT"),
            ChangeOp::Update => write!(f, "UPDATE"),
            ChangeOp::Delete => write!(f, "DELETE"),
        }
    }
}

/// A Change Data Capture (CDC) event emitted on table mutation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeEvent {
    /// Operation type (INSERT, UPDATE, DELETE)
    pub op: ChangeOp,
    /// Name of affected table
    pub table: String,
    /// Unique Row ID
    pub row_id: u64,
    /// Mutation timestamp (Unix epoch seconds)
    pub timestamp: u64,
    /// Row payload or modified fields
    pub data: serde_json::Value,
}

type Callback = Arc<dyn Fn(&ChangeEvent) + Send + Sync>;

/// In-memory Realtime Event Bus for Table Subscriptions & Live Queries
pub struct RealtimeBus {
    next_sub_id: AtomicU64,
    table_subscribers: RwLock<HashMap<String, Vec<(u64, Callback)>>>,
    global_subscribers: RwLock<Vec<(u64, Callback)>>,
}

impl Default for RealtimeBus {
    fn default() -> Self {
        Self::new()
    }
}

impl RealtimeBus {
    /// Create a new empty RealtimeBus
    pub fn new() -> Self {
        Self {
            next_sub_id: AtomicU64::new(1),
            table_subscribers: RwLock::new(HashMap::new()),
            global_subscribers: RwLock::new(Vec::new()),
        }
    }

    /// Subscribe to changes on a specific table or all tables ('*')
    pub fn subscribe<F>(&self, table: &str, callback: F) -> u64
    where
        F: Fn(&ChangeEvent) + Send + Sync + 'static,
    {
        let id = self.next_sub_id.fetch_add(1, Ordering::SeqCst);
        let cb: Callback = Arc::new(callback);

        if table == "*" {
            let mut global = self.global_subscribers.write();
            global.push((id, cb));
        } else {
            let mut map = self.table_subscribers.write();
            map.entry(table.to_lowercase()).or_default().push((id, cb));
        }

        id
    }

    /// Unsubscribe an active subscription by ID
    pub fn unsubscribe(&self, sub_id: u64) {
        {
            let mut global = self.global_subscribers.write();
            global.retain(|(id, _)| *id != sub_id);
        }
        {
            let mut map = self.table_subscribers.write();
            for subscribers in map.values_mut() {
                subscribers.retain(|(id, _)| *id != sub_id);
            }
        }
    }

    /// Publish a change event to all relevant table and global subscribers
    pub fn publish(&self, event: &ChangeEvent) {
        // 1. Notify table-specific subscribers
        {
            let map = self.table_subscribers.read();
            if let Some(subscribers) = map.get(&event.table.to_lowercase()) {
                for (_, cb) in subscribers {
                    cb(event);
                }
            }
        }

        // 2. Notify global subscribers
        {
            let global = self.global_subscribers.read();
            for (_, cb) in global.iter() {
                cb(event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn test_realtime_table_subscription() {
        let bus = RealtimeBus::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let counter_clone = counter.clone();
        let sub_id = bus.subscribe("users", move |ev| {
            assert_eq!(ev.table, "users");
            assert_eq!(ev.op, ChangeOp::Insert);
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        // Trigger event on "users"
        bus.publish(&ChangeEvent {
            op: ChangeOp::Insert,
            table: "users".to_string(),
            row_id: 1,
            timestamp: 1700000000,
            data: serde_json::json!({"name": "Faiz"}),
        });

        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Trigger event on different table (should not increment)
        bus.publish(&ChangeEvent {
            op: ChangeOp::Insert,
            table: "orders".to_string(),
            row_id: 100,
            timestamp: 1700000001,
            data: serde_json::json!({"total": 50}),
        });

        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Unsubscribe
        bus.unsubscribe(sub_id);

        bus.publish(&ChangeEvent {
            op: ChangeOp::Insert,
            table: "users".to_string(),
            row_id: 2,
            timestamp: 1700000002,
            data: serde_json::json!({"name": "Ahmad"}),
        });

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
