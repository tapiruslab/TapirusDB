//! Native JSON Document Database Engine for TapirusDB.
//!
//! Provides flexible, schema-less MongoDB-style document storage and retrieval
//! seamlessly stored inside the same single `.tapir` database file.

use crate::error::{Error, Result};
use crate::traits::Value;
use crate::Connection;
use serde_json::Value as JsonValue;

/// A schema-less JSON Document Collection (analogous to a MongoDB collection)
pub struct Collection<'a> {
    table_name: String,
    conn: &'a Connection,
}

impl<'a> Collection<'a> {
    /// Open or create a Document collection by name
    pub fn open(name: &str, conn: &'a Connection) -> Result<Self> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::SqlSyntax(format!(
                "Invalid collection name '{name}': must contain only alphanumeric characters and underscores"
            )));
        }
        let table_name = format!("__doc_{name}");
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {table_name} (id INTEGER PRIMARY KEY, doc TEXT);"
        );
        conn.execute(&sql)?;
        Ok(Self { table_name, conn })
    }

    /// Insert a JSON document with an auto-incremented primary key atomically via RETURNING
    pub fn insert_one(&self, doc: &JsonValue) -> Result<u64> {
        let json_str = serde_json::to_string(doc)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize JSON document: {e}")))?;
        let sql = format!(
            "INSERT INTO {} (doc) VALUES (?) RETURNING id;",
            self.table_name
        );
        let rows = self.conn.query_with_params(
            &sql,
            &[Value::Text(json_str)],
        )?;
        if let Some(first) = rows.first() {
            let id: i64 = first.get("id")?;
            Ok(id as u64)
        } else {
            Err(Error::Internal("Failed to retrieve auto-generated ID from document insert".into()))
        }
    }

    /// Insert a JSON document with an explicit document ID
    pub fn insert_with_id(&self, id: u64, doc: &JsonValue) -> Result<()> {
        let json_str = serde_json::to_string(doc)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize JSON document: {e}")))?;
        let sql = format!(
            "INSERT INTO {} (id, doc) VALUES (?, ?);",
            self.table_name
        );
        self.conn.execute_with_params(
            &sql,
            &[Value::Integer(id as i64), Value::Text(json_str)],
        )?;
        Ok(())
    }

    /// Find a JSON document by its ID
    pub fn find_by_id(&self, id: u64) -> Result<Option<JsonValue>> {
        let sql = format!("SELECT doc FROM {} WHERE id = ?;", self.table_name);
        let rows = self.conn.query_with_params(
            &sql,
            &[Value::Integer(id as i64)],
        )?;
        if rows.is_empty() {
            return Ok(None);
        }
        let doc_str: String = rows[0].get("doc")?;
        let val: JsonValue = serde_json::from_str(&doc_str)
            .map_err(|e| Error::Corrupted(format!("Failed to parse stored JSON: {e}")))?;
        Ok(Some(val))
    }

    /// Retrieve all JSON documents in this collection
    pub fn find_all(&self) -> Result<Vec<(u64, JsonValue)>> {
        let sql = format!("SELECT id, doc FROM {};", self.table_name);
        let rows = self.conn.query(&sql)?;
        let mut docs = Vec::with_capacity(rows.len());

        for row in rows {
            let id: i64 = row.get("id")?;
            let doc_str: String = row.get("doc")?;
            let val: JsonValue = serde_json::from_str(&doc_str)
                .map_err(|e| Error::Corrupted(format!("Failed to parse stored JSON: {e}")))?;
            docs.push((id as u64, val));
        }

        Ok(docs)
    }

    /// Delete a JSON document by its ID
    pub fn delete(&self, id: u64) -> Result<bool> {
        let sql = format!("DELETE FROM {} WHERE id = ?;", self.table_name);
        let rows_affected = self.conn.execute_with_params(
            &sql,
            &[Value::Integer(id as i64)],
        )?;
        Ok(rows_affected > 0)
    }

    /// Update a JSON document by its ID
    pub fn update_by_id(&self, id: u64, doc: &JsonValue) -> Result<bool> {
        let serialized = serde_json::to_string(doc)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize document: {e}")))?;
        let sql = format!("UPDATE {} SET doc = ? WHERE id = ?;", self.table_name);
        let rows_affected = self.conn.execute_with_params(
            &sql,
            &[Value::Text(serialized), Value::Integer(id as i64)],
        )?;
        Ok(rows_affected > 0)
    }

    /// Count total documents in collection
    pub fn count(&self) -> Result<usize> {
        let sql = format!("SELECT COUNT(*) FROM {};", self.table_name);
        let rows = self.conn.query(&sql)?;
        if let Some(first) = rows.first() {
            let cnt: i64 = first.get("COUNT(*)")?;
            Ok(cnt.max(0) as usize)
        } else {
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mongodb_style_document_collection() {
        let conn = Connection::open_in_memory().expect("Open in-memory");
        let collection = conn.collection("users").expect("Open collection");

        let doc1 = json!({
            "name": "Alex",
            "role": "Chief Architect",
            "skills": ["Rust", "AI", "Databases"],
            "active": true
        });

        let doc2 = json!({
            "name": "Marcus Vance",
            "role": "Staff Systems Engineer",
            "skills": ["C++", "Distributed Systems", "Raft"],
            "active": true
        });

        let id1 = collection.insert_one(&doc1).expect("Insert doc1");
        let id2 = collection.insert_one(&doc2).expect("Insert doc2");

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(collection.count().unwrap(), 2);

        // Find by ID
        let fetched = collection.find_by_id(1).expect("Find doc1");
        assert!(fetched.is_some());
        let val = fetched.unwrap();
        assert_eq!(val["name"], "Alex");
        assert_eq!(val["role"], "Chief Architect");
        assert_eq!(val["skills"][0], "Rust");

        // Find all
        let all = collection.find_all().expect("Find all");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_document_deletion_and_no_collision() {
        let conn = Connection::open_in_memory().expect("Open in-memory");
        let collection = conn.collection("items").expect("Open collection");

        let doc = json!({"item": "A"});
        let id1 = collection.insert_one(&doc).expect("Insert 1");
        let id2 = collection.insert_one(&doc).expect("Insert 2");
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);

        // Delete id 1. Count drops to 1.
        let deleted = collection.delete(1).expect("Delete id 1");
        assert!(deleted);
        assert_eq!(collection.count().unwrap(), 1);

        // Insert new document: MUST get id 3 without colliding with id 2!
        let id3 = collection.insert_one(&doc).expect("Insert 3 without collision");
        assert_eq!(id3, 3);
        assert_eq!(collection.count().unwrap(), 2);

        let doc3 = collection.find_by_id(3).expect("Find doc 3");
        assert!(doc3.is_some());
    }

    #[test]
    fn test_collection_name_sanitization_rejects_sql_injection() {
        let conn = Connection::open_in_memory().expect("Open in-memory");

        // Malicious names with spaces, semicolons, quotes, comments
        assert!(conn.collection("users; DROP TABLE users; --").is_err());
        assert!(conn.collection("users' OR 1=1 --").is_err());
        assert!(conn.collection("my collection").is_err());
        assert!(conn.collection("").is_err());
        assert!(conn.collection("coll-name").is_err());

        // Valid names
        assert!(conn.collection("valid_name_123").is_ok());
        assert!(conn.collection("items").is_ok());
    }
}
