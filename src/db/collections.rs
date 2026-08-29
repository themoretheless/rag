//! Durable collections, ordered outline membership, and prerequisite links.

use std::collections::{HashMap, HashSet};

use duckdb::{params, Connection};

use super::Store;
use crate::error::{AppError, Result};
use crate::models::{Collection, CollectionDetail, CollectionEntry};

impl Store {
    pub fn create_collection(
        &self,
        collection: &Collection,
        entries: &[CollectionEntry],
    ) -> Result<CollectionDetail> {
        validate_collection(collection, entries)?;
        let mut conn = self.lock()?;
        validate_entries(&conn, entries)?;
        let tx = conn.transaction()?;
        tx.execute(
            r#"INSERT INTO collections
               (id, name, description, metadata_json, created_at, updated_at)
               VALUES (?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))"#,
            params![
                collection.id,
                collection.name,
                collection.description,
                collection.metadata_json,
                collection.created_at,
                collection.updated_at,
            ],
        )?;
        replace_entries(&tx, &collection.id, entries)?;
        tx.commit()?;
        drop(conn);
        self.get_collection(&collection.id)?.ok_or_else(|| {
            AppError::not_found(format!("collection not found after create: {}", collection.id))
        })
    }

    pub fn list_collections(&self) -> Result<Vec<Collection>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"SELECT id, name, description, metadata_json,
                      CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR)
               FROM collections ORDER BY updated_at DESC, name ASC"#,
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_collection(row)?);
        }
        Ok(out)
    }

    pub fn get_collection(&self, id: &str) -> Result<Option<CollectionDetail>> {
        let conn = self.lock()?;
        let collection = conn
            .query_row(
                r#"SELECT id, name, description, metadata_json,
                          CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR)
                   FROM collections WHERE id = ?"#,
                params![id],
                row_to_collection,
            )
            .ok();
        let Some(collection) = collection else { return Ok(None) };

        let mut stmt = conn.prepare(
            r#"SELECT document_id, position, parent_document_id
               FROM collection_entries WHERE collection_id = ?
               ORDER BY position ASC, document_id ASC"#,
        )?;
        let mut rows = stmt.query(params![id])?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next()? {
            let document_id: String = row.get(0)?;
            let mut deps_stmt = conn.prepare(
                r#"SELECT depends_on_document_id FROM collection_dependencies
                   WHERE collection_id = ? AND document_id = ?
                   ORDER BY depends_on_document_id ASC"#,
            )?;
            let deps = deps_stmt
                .query_map(params![id, document_id], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            entries.push(CollectionEntry {
                document_id,
                position: row.get(1)?,
                parent_document_id: row.get(2)?,
                depends_on: deps,
            });
        }
        let (dependency_order, dependency_cycle_members) = dependency_order(&entries);
        Ok(Some(CollectionDetail {
            collection,
            entries,
            dependency_order,
            dependency_cycle_members,
        }))
    }

    pub fn update_collection(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<Option<&str>>,
        metadata_json: Option<&str>,
        entries: Option<&[CollectionEntry]>,
    ) -> Result<CollectionDetail> {
        let current = self
            .get_collection(id)?
            .ok_or_else(|| AppError::not_found(format!("collection not found: {id}")))?;
        let next_name = name.unwrap_or(&current.collection.name).trim();
        if next_name.is_empty() {
            return Err(AppError::config("collection name must be non-empty"));
        }
        let next_metadata = metadata_json.unwrap_or(&current.collection.metadata_json);
        validate_metadata(next_metadata)?;
        if let Some(entries) = entries {
            let conn = self.lock()?;
            validate_entries(&conn, entries)?;
        }

        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let next_description = match description {
            Some(value) => value,
            None => current.collection.description.as_deref(),
        };
        tx.execute(
            r#"UPDATE collections SET name = ?, description = ?, metadata_json = ?,
                      updated_at = CURRENT_TIMESTAMP WHERE id = ?"#,
            params![next_name, next_description, next_metadata, id],
        )?;
        if let Some(entries) = entries {
            replace_entries(&tx, id, entries)?;
        }
        tx.commit()?;
        drop(conn);
        self.get_collection(id)?.ok_or_else(|| {
            AppError::not_found(format!("collection not found after update: {id}"))
        })
    }
}

fn validate_collection(collection: &Collection, entries: &[CollectionEntry]) -> Result<()> {
    if collection.id.trim().is_empty() || collection.name.trim().is_empty() {
        return Err(AppError::config("collection id and name must be non-empty"));
    }
    validate_metadata(&collection.metadata_json)?;
    let _ = entries;
    Ok(())
}

fn validate_metadata(raw: &str) -> Result<()> {
    serde_json::from_str::<serde_json::Value>(raw)
        .map(|_| ())
        .map_err(|e| AppError::config(format!("metadata_json is not valid JSON: {e}")))
}

fn validate_entries(conn: &Connection, entries: &[CollectionEntry]) -> Result<()> {
    let ids: HashSet<&str> = entries.iter().map(|e| e.document_id.trim()).collect();
    if ids.len() != entries.len() || ids.contains("") {
        return Err(AppError::config("collection document ids must be non-empty and unique"));
    }
    for entry in entries {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM documents WHERE id = ?",
            params![entry.document_id],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Err(AppError::not_found(format!("document not found: {}", entry.document_id)));
        }
        if let Some(parent) = entry.parent_document_id.as_deref() {
            if parent == entry.document_id || !ids.contains(parent) {
                return Err(AppError::config(format!(
                    "parent_document_id for {} must name another collection member",
                    entry.document_id
                )));
            }
        }
        let mut seen = HashSet::new();
        for dep in &entry.depends_on {
            if dep == &entry.document_id || !ids.contains(dep.as_str()) || !seen.insert(dep) {
                return Err(AppError::config(format!(
                    "depends_on for {} must contain unique other collection members",
                    entry.document_id
                )));
            }
        }
    }
    for entry in entries {
        let mut cursor = entry.parent_document_id.as_deref();
        let mut visited = HashSet::new();
        while let Some(parent) = cursor {
            if !visited.insert(parent) || parent == entry.document_id {
                return Err(AppError::config(format!(
                    "outline parent cycle includes {}",
                    entry.document_id
                )));
            }
            cursor = entries
                .iter()
                .find(|candidate| candidate.document_id == parent)
                .and_then(|candidate| candidate.parent_document_id.as_deref());
        }
    }
    Ok(())
}

fn dependency_order(entries: &[CollectionEntry]) -> (Vec<String>, Vec<String>) {
    let mut indegree: HashMap<&str, usize> = entries
        .iter()
        .map(|entry| (entry.document_id.as_str(), entry.depends_on.len()))
        .collect();
    let mut ordered = Vec::with_capacity(entries.len());
    let mut emitted = HashSet::new();
    loop {
        let next = entries.iter().find(|entry| {
            !emitted.contains(entry.document_id.as_str())
                && indegree.get(entry.document_id.as_str()).copied().unwrap_or(0) == 0
        });
        let Some(next) = next else { break };
        emitted.insert(next.document_id.as_str());
        ordered.push(next.document_id.clone());
        for candidate in entries {
            if candidate.depends_on.iter().any(|id| id == &next.document_id) {
                if let Some(value) = indegree.get_mut(candidate.document_id.as_str()) {
                    *value = value.saturating_sub(1);
                }
            }
        }
    }
    let cycle_members = entries
        .iter()
        .filter(|entry| !emitted.contains(entry.document_id.as_str()))
        .map(|entry| entry.document_id.clone())
        .collect::<Vec<_>>();
    if cycle_members.is_empty() {
        (ordered, cycle_members)
    } else {
        (Vec::new(), cycle_members)
    }
}

fn replace_entries(conn: &Connection, collection_id: &str, entries: &[CollectionEntry]) -> Result<()> {
    conn.execute("DELETE FROM collection_dependencies WHERE collection_id = ?", params![collection_id])?;
    conn.execute("DELETE FROM collection_entries WHERE collection_id = ?", params![collection_id])?;
    for (position, entry) in entries.iter().enumerate() {
        conn.execute(
            r#"INSERT INTO collection_entries
               (collection_id, document_id, position, parent_document_id) VALUES (?, ?, ?, ?)"#,
            params![collection_id, entry.document_id, position as i32, entry.parent_document_id],
        )?;
        for dependency in &entry.depends_on {
            conn.execute(
                r#"INSERT INTO collection_dependencies
                   (collection_id, document_id, depends_on_document_id) VALUES (?, ?, ?)"#,
                params![collection_id, entry.document_id, dependency],
            )?;
        }
    }
    Ok(())
}

fn row_to_collection(row: &duckdb::Row<'_>) -> duckdb::Result<Collection> {
    let created: String = row.get(4)?;
    let updated: String = row.get(5)?;
    Ok(Collection {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        metadata_json: row.get(3)?,
        created_at: created,
        updated_at: updated,
    })
}
